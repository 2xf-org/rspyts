//! The response envelope (ABI §4) and allocation primitives (ABI §2).
//!
//! Layout of every value returned by a bridged function:
//!
//! ```text
//! offset  size       field
//! 0       1          status   (0 ok, 1 error, 2 panic)
//! 1       3          reserved (zero)
//! 4       4          json_len (u32 LE)
//! 8       4          tail_len (u32 LE)
//! 12      json_len   UTF-8 JSON payload
//! 12+j    tail_len   raw numeric tail (Buf<T> data)
//! ```
//!
//! Envelopes are allocated so capacity == length, which lets the foreign
//! caller free them with `rspyts_free(ptr, 12 + json_len + tail_len)`.

use crate::error::BridgeError;
use serde::Serialize;
use std::cell::RefCell;

pub const HEADER_LEN: usize = 12;

pub const STATUS_OK: u8 = 0;
pub const STATUS_ERROR: u8 = 1;
pub const STATUS_PANIC: u8 = 2;

/// Allocate `len` zeroed bytes for the foreign caller (ABI `rspyts_alloc`).
///
/// Zero-length allocations return a dangling, well-aligned, non-null
/// pointer that must not be dereferenced (mirrored by [`dealloc`]).
pub fn alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::NonNull::<u8>::dangling().as_ptr();
    }
    let boxed: Box<[u8]> = vec![0u8; len].into_boxed_slice();
    Box::into_raw(boxed).cast::<u8>()
}

/// Free a buffer previously produced by [`alloc`] or returned as an
/// envelope (ABI `rspyts_free`).
///
/// # Safety
/// `ptr` must originate from [`alloc`] or a sealed envelope with exactly
/// this `len`,
/// and must not be used afterwards.
pub unsafe fn dealloc(ptr: *mut u8, len: usize) {
    unsafe {
        if len == 0 || ptr.is_null() {
            return;
        }
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
    }
}

/// Assemble an envelope and leak it to the caller (capacity == length).
///
/// Callers must have validated the 4 GiB header limits first (see
/// [`encode_ok`]); this panic is a bug guard, not a reachable path.
fn seal(status: u8, json: &[u8], tail: &[u8]) -> *mut u8 {
    let json_len = u32::try_from(json.len()).expect("rspyts: JSON payload exceeds 4 GiB");
    let tail_len = u32::try_from(tail.len()).expect("rspyts: buffer tail exceeds 4 GiB");
    let total = HEADER_LEN + json.len() + tail.len();
    let mut buf = Vec::with_capacity(total);
    buf.push(status);
    buf.extend_from_slice(&[0u8; 3]);
    buf.extend_from_slice(&json_len.to_le_bytes());
    buf.extend_from_slice(&tail_len.to_le_bytes());
    buf.extend_from_slice(json);
    buf.extend_from_slice(tail);
    debug_assert_eq!(buf.len(), total);
    Box::into_raw(buf.into_boxed_slice()).cast::<u8>()
}

thread_local! {
    /// Collects `Buf<T>` bytes while a return value is being serialized.
    /// `Some` only inside [`encode_ok`]'s serialization scope; `Buf`
    /// serialized outside that scope falls back to a plain JSON array.
    static TAIL: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

/// Append `bytes` to the active tail, zero-padded so the data starts at a
/// multiple of `align`. Returns the byte offset of the data within the
/// tail, or `None` when no envelope serialization is in progress.
pub fn tail_push(bytes: &[u8], align: usize) -> Option<usize> {
    TAIL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let tail = slot.as_mut()?;
        let pad = (align - (tail.len() % align)) % align;
        tail.extend(std::iter::repeat_n(0u8, pad));
        let off = tail.len();
        tail.extend_from_slice(bytes);
        Some(off)
    })
}

/// Encode a successful return value: serialize `value` to JSON with the
/// tail collector active, then seal a status-0 envelope.
pub fn encode_ok<T: Serialize>(value: &T) -> *mut u8 {
    let previous = TAIL.with(|slot| slot.borrow_mut().replace(Vec::new()));
    let json = serde_json::to_vec(value);
    let tail = TAIL.with(|slot| {
        std::mem::replace(&mut *slot.borrow_mut(), previous).expect("rspyts: tail scope vanished")
    });
    match json {
        Ok(json) => {
            // The header stores lengths as u32. A >4 GiB payload must become
            // a status-1 error, not a panic: encode_ok runs AFTER
            // catch_unwind, so a panic here would unwind across the C
            // boundary and abort the host process.
            if json.len() > u32::MAX as usize || tail.len() > u32::MAX as usize {
                return encode_err(&BridgeError::new(
                    "payloadTooLarge",
                    "return value exceeds the 4 GiB envelope limit",
                ));
            }
            seal(STATUS_OK, &json, &tail)
        }
        Err(e) => encode_panic(&format!(
            "rspyts internal: result serialization failed: {e}"
        )),
    }
}

/// Encode an application error (status 1).
pub fn encode_err(err: &BridgeError) -> *mut u8 {
    let json = serde_json::to_vec(err)
        .unwrap_or_else(|_| br#"{"code":"panic","message":"error serialization failed"}"#.to_vec());
    seal(STATUS_ERROR, &json, &[])
}

/// Encode a caught panic (status 2).
pub fn encode_panic(message: &str) -> *mut u8 {
    let err = BridgeError::panic(message);
    let json = serde_json::to_vec(&err)
        .unwrap_or_else(|_| br#"{"code":"panic","message":"panic"}"#.to_vec());
    seal(STATUS_PANIC, &json, &[])
}

/// A decoded envelope, borrowed from raw parts. Used by the CLI (which is
/// itself a foreign caller of the manifest export) and by tests.
#[derive(Debug, PartialEq)]
pub struct Decoded<'a> {
    pub status: u8,
    pub json: &'a [u8],
    pub tail: &'a [u8],
}

/// Total allocation length of the envelope starting at `header`.
///
/// # Safety
/// `header` must point to at least [`HEADER_LEN`] readable bytes.
pub unsafe fn total_len(header: *const u8) -> usize {
    unsafe {
        let head = std::slice::from_raw_parts(header, HEADER_LEN);
        let json_len = u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize;
        let tail_len = u32::from_le_bytes(head[8..12].try_into().unwrap()) as usize;
        HEADER_LEN + json_len + tail_len
    }
}

/// Decode an envelope in place.
///
/// # Safety
/// `ptr` must point to a complete, valid envelope.
pub unsafe fn decode<'a>(ptr: *const u8) -> Decoded<'a> {
    unsafe {
        let total = total_len(ptr);
        let all = std::slice::from_raw_parts(ptr, total);
        let json_len = u32::from_le_bytes(all[4..8].try_into().unwrap()) as usize;
        Decoded {
            status: all[0],
            json: &all[HEADER_LEN..HEADER_LEN + json_len],
            tail: &all[HEADER_LEN + json_len..],
        }
    }
}

/// Test-only helper: decode an envelope into owned parts and free it, so
/// every test envelope is deallocated exactly once (leak hygiene).
#[cfg(test)]
pub(crate) fn decode_owned(ptr: *mut u8) -> (u8, Vec<u8>, Vec<u8>) {
    let total = unsafe { total_len(ptr) };
    let decoded = unsafe { decode(ptr) };
    let parts = (decoded.status, decoded.json.to_vec(), decoded.tail.to_vec());
    unsafe { dealloc(ptr, total) };
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trip() {
        let ptr = encode_ok(&serde_json::json!({"a": 1}));
        let decoded = unsafe { decode(ptr) };
        assert_eq!(decoded.status, STATUS_OK);
        assert_eq!(decoded.json, br#"{"a":1}"#);
        assert!(decoded.tail.is_empty());
        let total = unsafe { total_len(ptr) };
        assert_eq!(total, HEADER_LEN + decoded.json.len());
        unsafe { dealloc(ptr, total) };
    }

    #[test]
    fn error_envelope_carries_code() {
        let ptr = encode_err(&BridgeError::new("invalidArgs", "bad input"));
        let decoded = unsafe { decode(ptr) };
        assert_eq!(decoded.status, STATUS_ERROR);
        let v: serde_json::Value = serde_json::from_slice(decoded.json).unwrap();
        assert_eq!(v["code"], "invalidArgs");
        unsafe { dealloc(ptr, total_len(ptr)) };
    }

    #[test]
    fn alloc_zero_is_dangling_and_free_is_noop() {
        let p = alloc(0);
        assert!(!p.is_null());
        unsafe { dealloc(p, 0) };
    }

    #[test]
    fn tail_alignment_pads() {
        let previous = TAIL.with(|s| s.borrow_mut().replace(Vec::new()));
        assert_eq!(tail_push(&[1], 1), Some(0));
        // Next f64 push must start at offset 8, not 1.
        assert_eq!(tail_push(&[0u8; 16], 8), Some(8));
        TAIL.with(|s| *s.borrow_mut() = previous);
    }

    #[test]
    fn tail_push_outside_scope_returns_none() {
        assert_eq!(tail_push(&[1, 2, 3], 8), None);
    }

    #[test]
    fn empty_json_envelope_is_header_only() {
        let ptr = seal(STATUS_OK, &[], &[]);
        let total = unsafe { total_len(ptr) };
        assert_eq!(total, HEADER_LEN);
        let (status, json, tail) = decode_owned(ptr);
        assert_eq!(status, STATUS_OK);
        assert!(json.is_empty());
        assert!(tail.is_empty());
    }

    #[test]
    fn tail_only_envelope_decodes() {
        let ptr = seal(STATUS_OK, &[], &[9, 8, 7]);
        let (status, json, tail) = decode_owned(ptr);
        assert_eq!(status, STATUS_OK);
        assert!(json.is_empty());
        assert_eq!(tail, [9, 8, 7]);
    }

    #[test]
    fn json_and_tail_combine_in_one_envelope() {
        #[derive(serde::Serialize)]
        struct Out {
            label: String,
            values: crate::bridged::Buf<i32>,
        }
        let ptr = encode_ok(&Out {
            label: "combined".to_string(),
            values: crate::bridged::Buf::new(vec![-1, 2]),
        });
        let total = unsafe { total_len(ptr) };
        let (status, json, tail) = decode_owned(ptr);
        assert_eq!(status, STATUS_OK);
        assert_eq!(total, HEADER_LEN + json.len() + tail.len());
        let v: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(v["label"], "combined");
        assert_eq!(v["values"][crate::bridged::BUF_PLACEHOLDER_KEY]["len"], 2);
        let mut expected = Vec::new();
        expected.extend_from_slice(&(-1i32).to_le_bytes());
        expected.extend_from_slice(&2i32.to_le_bytes());
        assert_eq!(tail, expected);
    }

    #[test]
    fn interleaved_dtype_pushes_pad_byte_exactly() {
        #[derive(serde::Serialize)]
        struct Out {
            a: crate::bridged::Buf<u8>,
            b: crate::bridged::Buf<f64>,
            c: crate::bridged::Buf<i16>,
            d: crate::bridged::Buf<f32>,
        }
        let ptr = encode_ok(&Out {
            a: crate::bridged::Buf::new(vec![1u8, 2, 3]),
            b: crate::bridged::Buf::new(vec![1.5f64, -2.5]),
            c: crate::bridged::Buf::new(vec![-5i16, 7]),
            d: crate::bridged::Buf::new(vec![0.5f32]),
        });
        let (status, json, tail) = decode_owned(ptr);
        assert_eq!(status, STATUS_OK);

        // a: 3 u8 at offset 0; b: pad 5 to reach align 8, 16 bytes at 8;
        // c: already 2-aligned, 4 bytes at 24; d: already 4-aligned, 4 at 28.
        let mut expected = vec![1u8, 2, 3];
        expected.extend_from_slice(&[0u8; 5]);
        expected.extend_from_slice(&1.5f64.to_le_bytes());
        expected.extend_from_slice(&(-2.5f64).to_le_bytes());
        expected.extend_from_slice(&(-5i16).to_le_bytes());
        expected.extend_from_slice(&7i16.to_le_bytes());
        expected.extend_from_slice(&0.5f32.to_le_bytes());
        assert_eq!(tail, expected);
        assert_eq!(tail.len(), 32);

        let key = crate::bridged::BUF_PLACEHOLDER_KEY;
        let v: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(v["a"][key]["off"], 0);
        assert_eq!(v["b"][key]["off"], 8);
        assert_eq!(v["c"][key]["off"], 24);
        assert_eq!(v["d"][key]["off"], 28);
    }

    #[test]
    fn encode_ok_nests_four_levels_deep() {
        #[derive(serde::Serialize)]
        struct L4 {
            value: i32,
        }
        #[derive(serde::Serialize)]
        struct L3 {
            l4: L4,
        }
        #[derive(serde::Serialize)]
        struct L2 {
            l3: L3,
        }
        #[derive(serde::Serialize)]
        struct L1 {
            l2: L2,
        }
        let ptr = encode_ok(&L1 {
            l2: L2 {
                l3: L3 {
                    l4: L4 { value: 41 },
                },
            },
        });
        let (status, json, tail) = decode_owned(ptr);
        assert_eq!(status, STATUS_OK);
        assert!(tail.is_empty());
        let v: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(v["l2"]["l3"]["l4"]["value"], 41);
    }

    #[test]
    fn unicode_json_round_trips() {
        let text = "emoji 😀🦀, CJK 漢字仮名한글, null-adjacent \u{0}x\u{1}y\u{7f}";
        let ptr = encode_ok(&serde_json::json!({ "text": text }));
        let (status, json, _tail) = decode_owned(ptr);
        assert_eq!(status, STATUS_OK);
        // Control characters must be escaped so the payload stays valid JSON.
        let raw = std::str::from_utf8(&json).unwrap();
        assert!(raw.contains("\\u0000"));
        assert!(raw.contains("\\u0001"));
        let v: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(v["text"], text);
    }

    #[test]
    fn total_len_extracts_boundary_header_fields() {
        let mut header = [0u8; HEADER_LEN];
        header[0] = STATUS_PANIC;
        assert_eq!(unsafe { total_len(header.as_ptr()) }, HEADER_LEN);

        header[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        header[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            unsafe { total_len(header.as_ptr()) },
            HEADER_LEN + 2 * (u32::MAX as usize),
        );

        header[4..8].copy_from_slice(&1u32.to_le_bytes());
        header[8..12].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(unsafe { total_len(header.as_ptr()) }, HEADER_LEN + 1);
    }

    #[test]
    fn encode_panic_sets_status_and_code() {
        let ptr = encode_panic("it fell over");
        let (status, json, tail) = decode_owned(ptr);
        assert_eq!(status, STATUS_PANIC);
        assert!(tail.is_empty());
        let v: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(v["code"], "panic");
        assert_eq!(v["message"], "it fell over");
    }
}
