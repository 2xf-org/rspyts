//! Request/response envelopes (ABI §4) and allocation primitives (ABI §2).
//!
//! Requests and responses share one byte layout, but deliberately use
//! direction-specific decoders. A request marker must be zero; a response
//! status must be one of [`STATUS_OK`], [`STATUS_ERROR`], or [`STATUS_PANIC`].
//!
//! ```text
//! offset  size       field
//! 0       1          marker/status
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
use crate::ir::Ty;
use serde::Serialize;
use std::cell::RefCell;
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;

pub const HEADER_LEN: usize = 12;

pub const STATUS_OK: u8 = 0;
pub const STATUS_ERROR: u8 = 1;
pub const STATUS_PANIC: u8 = 2;
pub const REQUEST_MARKER: u8 = 0;

/// A rejected request or response envelope.
///
/// The message is intentionally human-readable rather than a wire contract;
/// runtimes surface it as a protocol error without trying to classify it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvelopeError {
    message: String,
}

impl EnvelopeError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EnvelopeError {}

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
fn frame(status: u8, json: &[u8], tail: &[u8]) -> Result<Vec<u8>, EnvelopeError> {
    let json_len = u32::try_from(json.len())
        .map_err(|_| EnvelopeError::new("JSON payload exceeds the 4 GiB envelope limit"))?;
    let tail_len = u32::try_from(tail.len())
        .map_err(|_| EnvelopeError::new("buffer tail exceeds the 4 GiB envelope limit"))?;
    let total = HEADER_LEN
        .checked_add(json.len())
        .and_then(|total| total.checked_add(tail.len()))
        .ok_or_else(|| EnvelopeError::new("envelope length overflows this platform"))?;
    let mut buf = Vec::with_capacity(total);
    buf.push(status);
    buf.extend_from_slice(&[0u8; 3]);
    buf.extend_from_slice(&json_len.to_le_bytes());
    buf.extend_from_slice(&tail_len.to_le_bytes());
    buf.extend_from_slice(json);
    buf.extend_from_slice(tail);
    debug_assert_eq!(buf.len(), total);
    Ok(buf)
}

fn seal(status: u8, json: &[u8], tail: &[u8]) -> *mut u8 {
    let buf = frame(status, json, tail).expect("rspyts: validated response envelope lengths");
    Box::into_raw(buf.into_boxed_slice()).cast::<u8>()
}

thread_local! {
    /// Collects `Buf<T>` bytes while a return value is being serialized.
    /// `Some` only inside [`encode_ok`]'s serialization scope; `Buf`
    /// serialized outside that scope falls back to a plain JSON array.
    static RESPONSE_TAIL: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };

    /// The request tail visible to `Buf<T>` deserializers on this thread.
    /// The raw pointer is never exposed and is valid for the lifetime carried
    /// by the active [`RequestTailGuard`].
    static REQUEST_TAIL: RefCell<Option<RequestTail>> = const { RefCell::new(None) };
}

/// Restores the previous response-tail state if serialization returns or
/// unwinds. Keeping this scope RAII-managed is part of the FFI panic barrier:
/// a user-provided `Serialize` implementation may panic.
struct ResponseTailGuard {
    previous: Option<Vec<u8>>,
    active: bool,
}

impl ResponseTailGuard {
    fn enter() -> Self {
        let previous = RESPONSE_TAIL.with(|slot| slot.borrow_mut().replace(Vec::new()));
        Self {
            previous,
            active: true,
        }
    }

    fn finish(mut self) -> Vec<u8> {
        let current = RESPONSE_TAIL
            .with(|slot| std::mem::replace(&mut *slot.borrow_mut(), self.previous.take()));
        self.active = false;
        current.expect("rspyts: tail scope vanished")
    }
}

impl Drop for ResponseTailGuard {
    fn drop(&mut self) {
        if self.active {
            RESPONSE_TAIL.with(|slot| {
                *slot.borrow_mut() = self.previous.take();
            });
        }
    }
}

#[derive(Clone, Copy)]
struct RequestTail {
    ptr: *const u8,
    len: usize,
}

/// Restores the previous request-tail state when a deserialization scope
/// exits, including during unwinding.
///
/// The guard is thread-bound and borrows its source tail, preventing normal
/// safe code from moving it to another thread or outliving the bytes.
pub struct RequestTailGuard<'a> {
    previous: Option<RequestTail>,
    _tail: PhantomData<&'a [u8]>,
    _not_send: PhantomData<Rc<()>>,
}

impl Drop for RequestTailGuard<'_> {
    fn drop(&mut self) {
        REQUEST_TAIL.with(|slot| {
            *slot.borrow_mut() = self.previous;
        });
    }
}

/// Enter a request-tail deserialization scope.
///
/// Scopes may nest; dropping the returned guard restores the exact prior
/// state. Prefer [`with_request_tail`] when a closure is convenient.
pub fn request_tail_scope(tail: &[u8]) -> RequestTailGuard<'_> {
    let current = RequestTail {
        ptr: tail.as_ptr(),
        len: tail.len(),
    };
    let previous = REQUEST_TAIL.with(|slot| slot.borrow_mut().replace(current));
    RequestTailGuard {
        previous,
        _tail: PhantomData,
        _not_send: PhantomData,
    }
}

/// Run `operation` with `tail` available to request buffer deserializers.
/// State restoration is guaranteed by [`RequestTailGuard`].
pub fn with_request_tail<R>(tail: &[u8], operation: impl FnOnce() -> R) -> R {
    let _guard = request_tail_scope(tail);
    operation()
}

/// Read `len` naturally aligned numeric elements from the active request
/// tail and decode each element from little-endian bytes.
pub fn request_tail_read<T: crate::bridged::BufElem>(
    off: usize,
    len: usize,
) -> Result<Vec<T>, EnvelopeError> {
    let width = std::mem::size_of::<T>();
    let align = std::mem::align_of::<T>();
    if off % align != 0 {
        return Err(EnvelopeError::new(format!(
            "request buffer offset {off} is not aligned to {align} bytes"
        )));
    }
    let byte_len = len
        .checked_mul(width)
        .ok_or_else(|| EnvelopeError::new("request buffer length overflows this platform"))?;
    let end = off
        .checked_add(byte_len)
        .ok_or_else(|| EnvelopeError::new("request buffer end overflows this platform"))?;

    REQUEST_TAIL.with(|slot| {
        let tail = slot.borrow().ok_or_else(|| {
            EnvelopeError::new("buffer placeholder used outside a request-tail scope")
        })?;
        if end > tail.len {
            return Err(EnvelopeError::new(format!(
                "request buffer range {off}..{end} exceeds tail length {}",
                tail.len
            )));
        }
        // SAFETY: `request_tail_scope` ties the stored pointer to the guard's
        // borrow, the guard is thread-bound, and this copy completes before
        // the thread-local borrow and active scope can end.
        let bytes = unsafe { std::slice::from_raw_parts(tail.ptr.add(off), byte_len) };
        T::from_le_bytes_vec(bytes)
            .ok_or_else(|| EnvelopeError::new("request buffer has an invalid byte length"))
    })
}

/// Append `bytes` to the active tail, zero-padded so the data starts at a
/// multiple of `align`. Returns the byte offset of the data within the
/// tail, or `None` when no envelope serialization is in progress.
pub fn tail_push(bytes: &[u8], align: usize) -> Option<usize> {
    RESPONSE_TAIL.with(|slot| {
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
    let tail_scope = ResponseTailGuard::enter();
    let json = serde_json::to_vec(value);
    let tail = tail_scope.finish();
    match json {
        Ok(json) => {
            // The header stores lengths as u32. A >4 GiB payload must become
            // a status-1 error rather than relying on the outer shim panic
            // barrier.
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

/// Encode a successful bridge return after schema-directed wire conversion.
///
/// The response-tail scope remains active while both ordinary Serde
/// serialization and native-to-wire normalization run. If either fails or
/// unwinds, its RAII guard restores prior thread-local state and no partial
/// attachment tail escapes.
pub fn encode_typed_ok<T: Serialize>(value: &T, ty: &Ty) -> *mut u8 {
    let tail_scope = ResponseTailGuard::enter();
    let json = crate::wire::serialize(value, ty).and_then(|value| {
        serde_json::to_vec(&value)
            .map_err(|error| crate::wire::WireError::from_serialization_error(error.to_string()))
    });
    let tail = tail_scope.finish();
    match json {
        Ok(json) => {
            if json.len() > u32::MAX as usize || tail.len() > u32::MAX as usize {
                return encode_err(&BridgeError::new(
                    "payloadTooLarge",
                    "return value exceeds the 4 GiB envelope limit",
                ));
            }
            seal(STATUS_OK, &json, &tail)
        }
        Err(error) => encode_panic(&format!(
            "rspyts internal: typed result serialization failed: {error}"
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

/// Construct a strict ABI-3 request envelope from JSON and an optional raw
/// attachment tail.
///
/// This helper is primarily useful to native callers and conformance tests;
/// Python and TypeScript build the identical layout in their own runtimes.
pub fn encode_request(json: &[u8], tail: &[u8]) -> Result<Vec<u8>, EnvelopeError> {
    let bytes = frame(REQUEST_MARKER, json, tail)?;
    decode_request(&bytes)?;
    Ok(bytes)
}

/// Decode and validate an ABI-3 request envelope.
///
/// Unlike [`decode_response`], byte zero is a request marker and must be
/// exactly zero.
pub fn decode_request(bytes: &[u8]) -> Result<Decoded<'_>, EnvelopeError> {
    let decoded = decode_layout(bytes)?;
    if decoded.status != REQUEST_MARKER {
        return Err(EnvelopeError::new(format!(
            "invalid request marker {}; expected 0",
            decoded.status
        )));
    }
    serde_json::from_slice::<serde_json::Value>(decoded.json)
        .map_err(|error| EnvelopeError::new(format!("malformed envelope JSON: {error}")))?;
    Ok(decoded)
}

/// Decode and validate an ABI-3 response envelope.
///
/// Response status is closed: only success (0), application error (1), and
/// caught panic (2) are valid.
pub fn decode_response(bytes: &[u8]) -> Result<Decoded<'_>, EnvelopeError> {
    let decoded = decode_layout(bytes)?;
    if !matches!(decoded.status, STATUS_OK | STATUS_ERROR | STATUS_PANIC) {
        return Err(EnvelopeError::new(format!(
            "invalid response status {}; expected 0, 1, or 2",
            decoded.status
        )));
    }
    serde_json::from_slice::<serde_json::Value>(decoded.json)
        .map_err(|error| EnvelopeError::new(format!("malformed envelope JSON: {error}")))?;
    if decoded.status != STATUS_OK && !decoded.tail.is_empty() {
        return Err(EnvelopeError::new(
            "error and panic envelopes must not contain attachment bytes",
        ));
    }
    Ok(decoded)
}

/// Parse the shared physical layout only. Direction semantics are applied by
/// [`decode_request`] and [`decode_response`], so there is no public lenient
/// decoder for foreign bytes.
fn decode_layout(bytes: &[u8]) -> Result<Decoded<'_>, EnvelopeError> {
    if bytes.len() < HEADER_LEN {
        return Err(EnvelopeError::new(format!(
            "truncated envelope header: expected {HEADER_LEN} bytes, got {}",
            bytes.len()
        )));
    }
    if bytes[1..4] != [0, 0, 0] {
        return Err(EnvelopeError::new(
            "invalid envelope: reserved header bytes must be zero",
        ));
    }

    let json_len = usize::try_from(u32::from_le_bytes(bytes[4..8].try_into().unwrap()))
        .map_err(|_| EnvelopeError::new("JSON length does not fit this platform"))?;
    let tail_len = usize::try_from(u32::from_le_bytes(bytes[8..12].try_into().unwrap()))
        .map_err(|_| EnvelopeError::new("tail length does not fit this platform"))?;
    let expected = HEADER_LEN
        .checked_add(json_len)
        .and_then(|total| total.checked_add(tail_len))
        .ok_or_else(|| EnvelopeError::new("envelope length overflows this platform"))?;

    if bytes.len() < expected {
        return Err(EnvelopeError::new(format!(
            "truncated envelope: header declares {expected} bytes, got {}",
            bytes.len()
        )));
    }
    if bytes.len() > expected {
        return Err(EnvelopeError::new(format!(
            "trailing bytes after envelope: header declares {expected} bytes, got {}",
            bytes.len()
        )));
    }

    let json_end = HEADER_LEN + json_len;
    let json = &bytes[HEADER_LEN..json_end];
    let tail = &bytes[json_end..expected];
    Ok(Decoded {
        status: bytes[0],
        json,
        tail,
    })
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

/// Decode a trusted, Rust-produced response envelope in place.
///
/// # Safety
/// `ptr` must point to a complete, valid envelope. Foreign bytes must first
/// be passed to [`decode_response`] instead.
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

    fn valid_request() -> Vec<u8> {
        encode_request(br#"{"value":1}"#, &[]).unwrap()
    }

    #[test]
    fn strict_request_and_response_decoders_are_direction_specific() {
        let request = valid_request();
        let decoded = decode_request(&request).unwrap();
        assert_eq!(decoded.status, REQUEST_MARKER);
        assert_eq!(decoded.json, br#"{"value":1}"#);

        let response = frame(STATUS_PANIC, br#"{"code":"panic"}"#, &[]).unwrap();
        assert_eq!(decode_response(&response).unwrap().status, STATUS_PANIC);

        let mut invalid_request = request;
        invalid_request[0] = STATUS_ERROR;
        assert_eq!(
            decode_request(&invalid_request).unwrap_err().to_string(),
            "invalid request marker 1; expected 0"
        );

        let invalid_response = frame(3, br#"null"#, &[]).unwrap();
        assert_eq!(
            decode_response(&invalid_response).unwrap_err().to_string(),
            "invalid response status 3; expected 0, 1, or 2"
        );
    }

    #[test]
    fn strict_decoder_rejects_reserved_bytes_truncation_and_trailing_bytes() {
        let mut reserved = valid_request();
        reserved[2] = 1;
        assert!(
            decode_request(&reserved)
                .unwrap_err()
                .to_string()
                .contains("reserved header bytes")
        );

        for len in [0, HEADER_LEN - 1, valid_request().len() - 1] {
            let request = valid_request();
            assert!(
                decode_request(&request[..len])
                    .unwrap_err()
                    .to_string()
                    .contains("truncated envelope")
            );
        }

        let mut trailing = valid_request();
        trailing.push(0);
        assert!(
            decode_request(&trailing)
                .unwrap_err()
                .to_string()
                .contains("trailing bytes")
        );

        let mut impossible_lengths = [0u8; HEADER_LEN].to_vec();
        impossible_lengths[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        impossible_lengths[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(
            decode_request(&impossible_lengths)
                .unwrap_err()
                .to_string()
                .contains("truncated envelope")
        );
    }

    #[test]
    fn strict_decoder_rejects_malformed_json() {
        let malformed = frame(REQUEST_MARKER, br#"{"#, &[]).unwrap();
        assert!(
            decode_request(&malformed)
                .unwrap_err()
                .to_string()
                .contains("malformed envelope JSON")
        );

        let empty = frame(REQUEST_MARKER, &[], &[]).unwrap();
        assert!(
            decode_request(&empty)
                .unwrap_err()
                .to_string()
                .contains("malformed envelope JSON")
        );
    }

    #[test]
    fn strict_decoder_leaves_reserved_key_maps_for_schema_directed_decoding() {
        for json in [
            br#"{"__rspyts_buf__":{"off":0,"len":1,"dt":"u8"}}"#.as_slice(),
            br#"{"__rspyts_buf__":null}"#.as_slice(),
            br#"{"__rspyts_buf__":{"off":0,"len":1}}"#.as_slice(),
            br#"{"__rspyts_buf__":{"off":-1,"len":1,"dt":"u8"}}"#.as_slice(),
            br#"{"__rspyts_buf__":{"off":0,"len":1,"dt":"u8"},"extra":true}"#.as_slice(),
        ] {
            let bytes = frame(REQUEST_MARKER, json, &[0]).unwrap();
            assert_eq!(decode_request(&bytes).unwrap().json, json);
        }
    }

    #[test]
    fn request_tail_reads_little_endian_values_with_bounds_and_alignment() {
        let mut tail = Vec::new();
        tail.extend_from_slice(&(-2i16).to_le_bytes());
        tail.extend_from_slice(&7i16.to_le_bytes());
        tail.extend_from_slice(&1.5f32.to_le_bytes());

        with_request_tail(&tail, || {
            assert_eq!(request_tail_read::<i16>(0, 2).unwrap(), [-2, 7]);
            assert_eq!(request_tail_read::<f32>(4, 1).unwrap(), [1.5]);
            assert!(
                request_tail_read::<i16>(1, 1)
                    .unwrap_err()
                    .to_string()
                    .contains("not aligned")
            );
            assert!(
                request_tail_read::<f32>(8, 1)
                    .unwrap_err()
                    .to_string()
                    .contains("exceeds tail length")
            );
            assert!(
                request_tail_read::<f64>(0, usize::MAX)
                    .unwrap_err()
                    .to_string()
                    .contains("overflows")
            );
        });
        assert!(
            request_tail_read::<u8>(0, 0)
                .unwrap_err()
                .to_string()
                .contains("outside a request-tail scope")
        );
    }

    #[test]
    fn request_tail_guard_restores_nested_prior_state() {
        with_request_tail(&[10, 11], || {
            assert_eq!(request_tail_read::<u8>(0, 2).unwrap(), [10, 11]);
            {
                let _inner = request_tail_scope(&[20]);
                assert_eq!(request_tail_read::<u8>(0, 1).unwrap(), [20]);
            }
            assert_eq!(request_tail_read::<u8>(0, 2).unwrap(), [10, 11]);
        });
        assert!(request_tail_read::<u8>(0, 1).is_err());
    }

    #[test]
    fn request_tail_guard_clears_state_after_failure_and_panic() {
        let failed: serde_json::Result<serde_json::Value> =
            with_request_tail(&[1, 2, 3], || serde_json::from_slice(br#"{"#));
        assert!(failed.is_err());
        assert!(request_tail_read::<u8>(0, 1).is_err());

        let panicked = std::panic::catch_unwind(|| {
            with_request_tail(&[4, 5, 6], || panic!("deserializer panic"));
        });
        assert!(panicked.is_err());
        assert!(request_tail_read::<u8>(0, 1).is_err());
    }

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
        let previous = RESPONSE_TAIL.with(|s| s.borrow_mut().replace(Vec::new()));
        assert_eq!(tail_push(&[1], 1), Some(0));
        // Next f64 push must start at offset 8, not 1.
        assert_eq!(tail_push(&[0u8; 16], 8), Some(8));
        RESPONSE_TAIL.with(|s| *s.borrow_mut() = previous);
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
