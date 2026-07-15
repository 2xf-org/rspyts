//! Panic-contained entry points for macro-generated `extern "C"` shims
//! (ABI §9).
//!
//! Every exported function body is exactly:
//!
//! ```ignore
//! rspyts_core::shim::run(|| {
//!     let args: Args = unsafe { shim::decode_args(args_ptr, args_len) }?;
//!     let values = unsafe { shim::slice_arg(s0_ptr, s0_len) };
//!     shim::map_result(process_values(values, args.batch_size, &args.options))
//! })
//! ```
//!
//! No user code runs outside the `catch_unwind`; no panic ever unwinds
//! across the C boundary. (On `wasm32-unknown-unknown`, where the default
//! panic strategy is `abort`, a panic traps the instance instead of
//! producing a status-2 envelope — the shim is identical, the platform is
//! just less forgiving.)

use crate::bridged::{Bridged, SliceElem};
use crate::envelope;
use crate::error::{BridgeErr, BridgeError};
use crate::ir::ParamDecl;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::mem::ManuallyDrop;
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
        Ok(Ok(value)) => encode_success(value),
        Ok(Err(err)) => envelope::encode_err(&err),
        Err(payload) => envelope::encode_panic(&panic_message(payload)),
    }
}

/// Run a user-value shim body and encode its success through the declared
/// native Rust bridge type. Module metadata and opaque class-handle shims use
/// [`run`] instead because their payloads are ABI infrastructure, not user
/// values.
pub fn run_typed<T, F>(body: F) -> *mut u8
where
    T: Serialize + Bridged,
    F: FnOnce() -> Result<T, BridgeError>,
{
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(value)) => encode_typed_success(value),
        Ok(Err(err)) => envelope::encode_err(&err),
        Err(payload) => envelope::encode_panic(&panic_message(payload)),
    }
}

/// Serialize and destroy a successful value behind separate unwind barriers.
///
/// `ManuallyDrop` prevents a serialization panic from beginning a second
/// unwind if the value's destructor also panics. If destruction fails after
/// an envelope was allocated, discard that envelope before returning the
/// status-2 replacement.
fn encode_success<T: Serialize>(value: T) -> *mut u8 {
    let mut value = ManuallyDrop::new(value);
    let encoded = catch_unwind(AssertUnwindSafe(|| envelope::encode_ok(&*value)));
    let dropped = catch_unwind(AssertUnwindSafe(|| unsafe {
        ManuallyDrop::drop(&mut value);
    }));

    match (encoded, dropped) {
        (Ok(ptr), Ok(())) => ptr,
        (Ok(ptr), Err(payload)) => {
            let len = unsafe { envelope::total_len(ptr) };
            unsafe { envelope::dealloc(ptr, len) };
            envelope::encode_panic(&panic_message(payload))
        }
        (Err(payload), Ok(())) => envelope::encode_panic(&panic_message(payload)),
        (Err(encode_payload), Err(drop_payload)) => envelope::encode_panic(&format!(
            "{}; value destruction also panicked: {}",
            panic_message(encode_payload),
            panic_message(drop_payload)
        )),
    }
}

/// Serialize, schema-normalize, and destroy a successful user value behind
/// separate unwind barriers.
fn encode_typed_success<T: Serialize + Bridged>(value: T) -> *mut u8 {
    let mut value = ManuallyDrop::new(value);
    let encoded = catch_unwind(AssertUnwindSafe(|| {
        envelope::encode_typed_ok(&*value, &T::inventory_return_ty())
    }));
    let dropped = catch_unwind(AssertUnwindSafe(|| unsafe {
        ManuallyDrop::drop(&mut value);
    }));

    match (encoded, dropped) {
        (Ok(ptr), Ok(())) => ptr,
        (Ok(ptr), Err(payload)) => {
            let len = unsafe { envelope::total_len(ptr) };
            unsafe { envelope::dealloc(ptr, len) };
            envelope::encode_panic(&panic_message(payload))
        }
        (Err(payload), Ok(())) => envelope::encode_panic(&panic_message(payload)),
        (Err(encode_payload), Err(drop_payload)) => envelope::encode_panic(&format!(
            "{}; value destruction also panicked: {}",
            panic_message(encode_payload),
            panic_message(drop_payload)
        )),
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

/// Deserialize an ABI-3 request envelope.
///
/// # Safety
/// `(ptr, len)` must describe `len` readable bytes (the caller-owned
/// request buffer, ABI §2). `len == 0` remains a compatibility shorthand for
/// an empty JSON object; every non-empty request must use envelope framing.
pub unsafe fn decode_args<T: DeserializeOwned>(
    ptr: *const u8,
    len: usize,
) -> Result<T, BridgeError> {
    unsafe {
        if len == 0 {
            return serde_json::from_slice(b"{}").map_err(BridgeError::invalid_args);
        }
        let bytes = std::slice::from_raw_parts(ptr, len);
        let decoded = envelope::decode_request(bytes).map_err(BridgeError::invalid_args)?;
        envelope::with_request_tail(decoded.tail, || serde_json::from_slice(decoded.json))
            .map_err(BridgeError::invalid_args)
    }
}

/// Deserialize an ABI-3 request envelope after schema-directed conversion
/// from wire JSON to ordinary Rust Serde values.
///
/// # Safety
/// `(ptr, len)` follows the same requirements as [`decode_args`].
pub unsafe fn decode_typed_args<T: DeserializeOwned>(
    ptr: *const u8,
    len: usize,
    params: &[ParamDecl],
) -> Result<T, BridgeError> {
    unsafe {
        if len == 0 {
            return crate::wire::deserialize_params(serde_json::json!({}), params)
                .map_err(BridgeError::invalid_args);
        }
        let bytes = std::slice::from_raw_parts(ptr, len);
        let decoded = envelope::decode_request(bytes).map_err(BridgeError::invalid_args)?;
        envelope::with_request_tail(decoded.tail, || {
            let value = serde_json::from_slice(decoded.json).map_err(BridgeError::invalid_args)?;
            crate::wire::deserialize_params(value, params).map_err(BridgeError::invalid_args)
        })
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
    fn run_typed_encodes_native_exact_integers_as_wire_strings() {
        let ptr = run_typed(|| Ok::<_, BridgeError>(i64::MIN));
        let (status, json, tail) = envelope::decode_owned(ptr);
        assert_eq!(status, envelope::STATUS_OK);
        assert_eq!(json, format!(r#""{}""#, i64::MIN).as_bytes());
        assert!(tail.is_empty());
    }

    #[test]
    fn run_typed_uses_return_context_for_unit_aliases() {
        type Nothing = ();
        let ptr = run_typed(|| Ok::<Nothing, BridgeError>(()));
        let (status, json, tail) = envelope::decode_owned(ptr);
        assert_eq!(status, envelope::STATUS_OK);
        assert_eq!(json, b"null");
        assert!(tail.is_empty());
        assert_eq!(
            <Nothing as Bridged>::inventory_return_ty(),
            crate::ir::Ty::Unit
        );
    }

    #[test]
    fn typed_normalization_failure_discards_partial_attachment_tail() {
        let unsafe_json = serde_json::Value::from(9_007_199_254_740_992_u64);
        let ptr = run_typed(|| Ok::<_, BridgeError>((crate::Buf::new(vec![1_u8, 2]), unsafe_json)));
        let (status, json, tail) = envelope::decode_owned(ptr);
        assert_eq!(status, envelope::STATUS_PANIC);
        assert!(tail.is_empty());
        let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert!(
            value["message"]
                .as_str()
                .unwrap()
                .contains("not exactly representable")
        );
        assert!(envelope::tail_push(&[1], 1).is_none());
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

    struct PanicSerialize;

    impl Serialize for PanicSerialize {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            panic!("serialization exploded")
        }
    }

    #[test]
    fn run_catches_panics_during_success_serialization() {
        let ptr = run(|| Ok::<_, BridgeError>(PanicSerialize));
        let (status, json, tail) = envelope::decode_owned(ptr);
        assert_eq!(status, envelope::STATUS_PANIC);
        assert!(tail.is_empty());
        let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(value["message"], "serialization exploded");

        // The response-tail scope was restored while unwinding.
        assert!(envelope::tail_push(&[1], 1).is_none());
    }

    struct PanicDrop;

    impl Serialize for PanicDrop {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            serializer.serialize_str("encoded before drop")
        }
    }

    impl Drop for PanicDrop {
        fn drop(&mut self) {
            panic!("destructor exploded")
        }
    }

    #[test]
    fn run_catches_panics_during_success_value_destruction() {
        let ptr = run(|| Ok::<_, BridgeError>(PanicDrop));
        let (status, json, tail) = envelope::decode_owned(ptr);
        assert_eq!(status, envelope::STATUS_PANIC);
        assert!(tail.is_empty());
        let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(value["message"], "destructor exploded");
    }

    #[test]
    fn decode_args_empty_is_empty_object() {
        #[derive(serde::Deserialize)]
        struct NoArgs {}
        let parsed: Result<NoArgs, _> = unsafe { decode_args(std::ptr::null(), 0) };
        assert!(parsed.is_ok());
    }

    #[test]
    fn decode_typed_args_normalizes_exact_integers_and_requires_option_params() {
        #[derive(Debug, serde::Deserialize, PartialEq)]
        struct Args {
            exact: i64,
            maybe: Option<u64>,
        }
        let params = [
            ParamDecl {
                name: "exact".into(),
                wire_name: "exact".into(),
                ty: crate::ir::Ty::I64,
            },
            ParamDecl {
                name: "maybe".into(),
                wire_name: "maybe".into(),
                ty: crate::ir::Ty::Option {
                    inner: Box::new(crate::ir::Ty::U64),
                },
            },
        ];

        let request = envelope::encode_request(
            br#"{"exact":"-9223372036854775808","maybe":"18446744073709551615"}"#,
            &[],
        )
        .unwrap();
        let decoded: Args =
            unsafe { decode_typed_args(request.as_ptr(), request.len(), &params) }.unwrap();
        assert_eq!(
            decoded,
            Args {
                exact: i64::MIN,
                maybe: Some(u64::MAX)
            }
        );

        let missing = envelope::encode_request(br#"{"exact":"0"}"#, &[]).unwrap();
        let error = unsafe { decode_typed_args::<Args>(missing.as_ptr(), missing.len(), &params) }
            .unwrap_err();
        assert_eq!(error.code, crate::error::codes::INVALID_ARGS);
        assert!(error.message.contains("missing required field `maybe`"));

        let numeric = envelope::encode_request(br#"{"exact":0,"maybe":null}"#, &[]).unwrap();
        assert!(
            unsafe { decode_typed_args::<Args>(numeric.as_ptr(), numeric.len(), &params) }.is_err()
        );
    }

    #[derive(Debug, serde::Deserialize)]
    struct CountArgs {
        #[allow(dead_code)]
        count: i32,
    }

    fn decode_count(bytes: &[u8]) -> Result<CountArgs, BridgeError> {
        let request = envelope::encode_request(bytes, &[]).unwrap();
        unsafe { decode_args(request.as_ptr(), request.len()) }
    }

    fn unchecked_request(json: &[u8], tail: &[u8]) -> Vec<u8> {
        let mut request = vec![0u8; envelope::HEADER_LEN];
        request[4..8].copy_from_slice(&(json.len() as u32).to_le_bytes());
        request[8..12].copy_from_slice(&(tail.len() as u32).to_le_bytes());
        request.extend_from_slice(json);
        request.extend_from_slice(tail);
        request
    }

    #[test]
    fn decode_args_invalid_utf8_is_invalid_args() {
        let request = unchecked_request(&[0xFF, 0xFE, 0x01], &[]);
        let err: BridgeError =
            unsafe { decode_args::<CountArgs>(request.as_ptr(), request.len()) }.unwrap_err();
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
            let request = unchecked_request(bytes, &[]);
            let err: BridgeError =
                unsafe { decode_args::<CountArgs>(request.as_ptr(), request.len()) }.unwrap_err();
            assert_eq!(err.code, crate::error::codes::INVALID_ARGS);
        }
    }

    #[test]
    fn decode_args_rejects_unframed_json() {
        let parsed = decode_count(br#"{"count":1}"#).unwrap();
        assert_eq!(parsed.count, 1);

        let raw = br#"{"count":1}"#;
        let err = unsafe { decode_args::<CountArgs>(raw.as_ptr(), raw.len()) }.unwrap_err();
        assert_eq!(err.code, crate::error::codes::INVALID_ARGS);
        assert!(err.message.contains("truncated envelope header"));
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
