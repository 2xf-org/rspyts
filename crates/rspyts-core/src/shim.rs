//! Panic-safe entry points for macro-generated `extern "C"` shims
//! (ABI §9).
//!
//! Every exported function body is exactly:
//!
//! ```ignore
//! rspyts_core::shim::run(|| {
//!     let args: Args = unsafe { shim::decode_args(args_ptr, args_len) }?;
//!     let samples = unsafe { shim::slice_arg(s0_ptr, s0_len) };
//!     shim::map_result(analyze_signal(samples, args.sample_rate, &args.params))
//! })
//! ```
//!
//! No user code runs outside the `catch_unwind`; no panic ever unwinds
//! across the C boundary. (On `wasm32-unknown-unknown`, where the default
//! panic strategy is `abort`, a panic traps the instance instead of
//! producing a status-2 envelope — the shim is identical, the platform is
//! just less forgiving.)

use crate::bridged::SliceElem;
use crate::envelope;
use crate::error::{BridgeErr, BridgeError};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Run a shim body, converting the three possible outcomes into an
/// envelope pointer: `Ok(value)` → status 0, `Err(bridge_error)` →
/// status 1, panic → status 2.
pub fn run<T, F>(body: F) -> *mut u8
where
    T: Serialize,
    F: FnOnce() -> Result<T, BridgeError>,
{
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(value)) => envelope::encode_ok(&value),
        Ok(Err(err)) => envelope::encode_err(&err),
        Err(payload) => envelope::encode_panic(&panic_message(payload)),
    }
}

/// Run a `__drop` shim body: nothing to encode, panics are swallowed
/// (drop must be infallible and idempotent, ABI §8).
pub fn run_drop<F: FnOnce()>(body: F) {
    let _ = catch_unwind(AssertUnwindSafe(body));
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    match payload.downcast::<&str>() {
        Ok(s) => (*s).to_string(),
        Err(payload) => match payload.downcast::<String>() {
            Ok(s) => *s,
            Err(_) => "panic (non-string payload)".to_string(),
        },
    }
}

/// Deserialize the args JSON payload.
///
/// # Safety
/// `(ptr, len)` must describe `len` readable bytes (the caller-owned
/// request buffer, ABI §2). `len == 0` is treated as an empty object.
pub unsafe fn decode_args<T: DeserializeOwned>(
    ptr: *const u8,
    len: usize,
) -> Result<T, BridgeError> {
    unsafe {
        let bytes: &[u8] = if len == 0 {
            b"{}"
        } else {
            std::slice::from_raw_parts(ptr, len)
        };
        serde_json::from_slice(bytes).map_err(BridgeError::invalid_args)
    }
}

/// Reconstitute a borrowed slice parameter.
///
/// # Safety
/// `(ptr, len)` must describe `len` valid, naturally-aligned elements of
/// `T` that outlive the current call (guaranteed by the runtimes, ABI §6).
pub unsafe fn slice_arg<'a, T: SliceElem>(ptr: *const T, len: usize) -> &'a [T] {
    unsafe {
        if len == 0 {
            return &[];
        }
        std::slice::from_raw_parts(ptr, len)
    }
}

/// Map a user `Result<T, E>` into the shim result. The macro calls this
/// for functions whose return type is written literally as `Result<…>`.
pub fn map_result<T, E: BridgeErr>(result: Result<T, E>) -> Result<T, BridgeError> {
    result.map_err(BridgeErr::into_bridge_error)
}

/// Wrap an infallible return value. The macro calls this for functions
/// with plain return types.
pub fn map_plain<T>(value: T) -> Result<T, BridgeError> {
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_encodes_ok() {
        let ptr = run(|| Ok::<_, BridgeError>(41i32));
        let decoded = unsafe { envelope::decode(ptr) };
        assert_eq!(decoded.status, envelope::STATUS_OK);
        assert_eq!(decoded.json, b"41");
        unsafe { envelope::dealloc(ptr, envelope::total_len(ptr)) };
    }

    #[test]
    fn run_encodes_app_error() {
        let ptr = run(|| Err::<i32, _>(BridgeError::new("boom", "it broke")));
        let decoded = unsafe { envelope::decode(ptr) };
        assert_eq!(decoded.status, envelope::STATUS_ERROR);
        unsafe { envelope::dealloc(ptr, envelope::total_len(ptr)) };
    }

    #[test]
    fn run_catches_panics() {
        let ptr = run(|| -> Result<i32, BridgeError> { panic!("kaboom {}", 7) });
        let decoded = unsafe { envelope::decode(ptr) };
        assert_eq!(decoded.status, envelope::STATUS_PANIC);
        let v: serde_json::Value = serde_json::from_slice(decoded.json).unwrap();
        assert_eq!(v["message"], "kaboom 7");
        unsafe { envelope::dealloc(ptr, envelope::total_len(ptr)) };
    }

    #[test]
    fn decode_args_empty_is_empty_object() {
        #[derive(serde::Deserialize)]
        struct NoArgs {}
        let parsed: Result<NoArgs, _> = unsafe { decode_args(std::ptr::null(), 0) };
        assert!(parsed.is_ok());
    }

    #[derive(Debug, serde::Deserialize)]
    struct CountArgs {
        #[allow(dead_code)]
        count: i32,
    }

    fn decode_count(bytes: &[u8]) -> Result<CountArgs, BridgeError> {
        unsafe { decode_args(bytes.as_ptr(), bytes.len()) }
    }

    #[test]
    fn decode_args_invalid_utf8_is_invalid_args() {
        let err = decode_count(&[0xFF, 0xFE, 0x01]).unwrap_err();
        assert_eq!(err.code, crate::error::codes::INVALID_ARGS);
        assert!(err.message.starts_with("invalid arguments:"));
    }

    #[test]
    fn decode_args_rejects_malformed_json_variants() {
        let malformed: [&[u8]; 5] = [
            br#"{"count":"#,        // truncated
            br#"[1, 2]"#,           // array where object expected
            br#"{"count":"nine"}"#, // wrong field type
            br#"{}"#,               // missing required field
            br#"{"count":1}extra"#, // trailing garbage
        ];
        for bytes in malformed {
            let err = decode_count(bytes).unwrap_err();
            assert_eq!(err.code, crate::error::codes::INVALID_ARGS);
        }
    }

    #[test]
    fn slice_arg_zero_len_accepts_null_and_dangling() {
        let from_null: &[f64] = unsafe { slice_arg(std::ptr::null(), 0) };
        assert!(from_null.is_empty());
        let dangling = std::ptr::NonNull::<i16>::dangling().as_ptr();
        let from_dangling: &[i16] = unsafe { slice_arg(dangling, 0) };
        assert!(from_dangling.is_empty());
    }

    #[test]
    fn run_reports_non_string_panic_payload() {
        let ptr = run(|| -> Result<(), BridgeError> { std::panic::panic_any(7i32) });
        let (status, json, _tail) = envelope::decode_owned(ptr);
        assert_eq!(status, envelope::STATUS_PANIC);
        let v: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(v["code"], "panic");
        assert_eq!(v["message"], "panic (non-string payload)");
    }

    #[test]
    fn run_reports_str_panic_payload_verbatim() {
        let ptr = run(|| -> Result<(), BridgeError> { std::panic::panic_any("plain str") });
        let (status, json, _tail) = envelope::decode_owned(ptr);
        assert_eq!(status, envelope::STATUS_PANIC);
        let v: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(v["message"], "plain str");
    }

    #[test]
    fn run_drop_swallows_panics() {
        run_drop(|| panic!("must not unwind"));
    }

    struct Overflow;

    impl BridgeErr for Overflow {
        fn into_bridge_error(self) -> BridgeError {
            BridgeError::new("overflow", "too big")
        }
    }

    #[test]
    fn map_result_converts_custom_errors() {
        assert_eq!(map_result(Ok::<_, Overflow>(5)).unwrap(), 5);
        let err = map_result(Err::<i32, _>(Overflow)).unwrap_err();
        assert_eq!(err.code, "overflow");
        assert_eq!(err.message, "too big");
    }

    #[test]
    fn map_plain_wraps_infallible_values() {
        assert_eq!(map_plain("hi").unwrap(), "hi");
    }
}
