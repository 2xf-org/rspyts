//! The bridge error model (ABI §5).

use serde::Serialize;
use serde::ser::SerializeStruct;

/// Well-known error codes used by the bridge itself.
pub mod codes {
    /// A panic crossed the shim boundary (envelope status 2).
    pub const PANIC: &str = "panic";
    /// The args payload failed to deserialize.
    pub const INVALID_ARGS: &str = "invalidArgs";
    /// A method was called on a dropped or unknown handle.
    pub const STALE_HANDLE: &str = "staleHandle";
}

/// The language-neutral error that crosses the boundary.
///
/// Serialized as `{"code": …, "message": …, "data": …}` with `data`
/// omitted when `None`. Runtimes map `code` to generated exception/error
/// classes.
#[derive(Clone, Debug, PartialEq)]
pub struct BridgeError {
    pub code: String,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

impl BridgeError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn panic(message: impl Into<String>) -> Self {
        Self::new(codes::PANIC, message)
    }

    pub fn invalid_args(detail: impl std::fmt::Display) -> Self {
        Self::new(codes::INVALID_ARGS, format!("invalid arguments: {detail}"))
    }

    pub fn stale_handle() -> Self {
        Self::new(
            codes::STALE_HANDLE,
            "handle has been dropped or never existed",
        )
    }
}

impl Serialize for BridgeError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let fields = if self.data.is_some() { 3 } else { 2 };
        let mut s = serializer.serialize_struct("BridgeError", fields)?;
        s.serialize_field("code", &self.code)?;
        s.serialize_field("message", &self.message)?;
        if let Some(data) = &self.data {
            s.serialize_field("data", data)?;
        }
        s.end()
    }
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for BridgeError {}

/// Conversion into a [`BridgeError`]; the `E` in a bridged
/// `Result<T, E>` must implement this.
///
/// `#[bridge(error)]` derives it for enums: the camelCase variant name
/// becomes `code`, the `Display` string becomes `message`, and named
/// variant fields become `data`.
pub trait BridgeErr {
    fn into_bridge_error(self) -> BridgeError;
}

impl BridgeErr for BridgeError {
    fn into_bridge_error(self) -> BridgeError {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_is_omitted_when_none() {
        let json = serde_json::to_string(&BridgeError::new("x", "y")).unwrap();
        assert_eq!(json, r#"{"code":"x","message":"y"}"#);
    }

    #[test]
    fn data_is_present_when_set() {
        let err = BridgeError::new("x", "y").with_data(serde_json::json!({"max": 5}));
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, r#"{"code":"x","message":"y","data":{"max":5}}"#);
    }

    #[test]
    fn explicit_null_data_is_not_skipped() {
        let err = BridgeError::new("x", "y").with_data(serde_json::Value::Null);
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, r#"{"code":"x","message":"y","data":null}"#);
    }

    #[test]
    fn display_formats_code_and_message() {
        let err = BridgeError::new("staleHandle", "handle has been dropped");
        assert_eq!(err.to_string(), "[staleHandle] handle has been dropped");
    }

    #[test]
    fn unicode_data_and_message_round_trip() {
        let err = BridgeError::new("limitExceeded", "limite excedido 🚫 上限")
            .with_data(serde_json::json!({"campo": "预算", "emoji": "😀"}));
        let json = serde_json::to_string(&err).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["message"], "limite excedido 🚫 上限");
        assert_eq!(v["data"]["campo"], "预算");
        assert_eq!(v["data"]["emoji"], "😀");
    }

    #[test]
    fn helper_constructors_set_well_known_codes() {
        assert_eq!(BridgeError::panic("boom").code, codes::PANIC);
        let invalid = BridgeError::invalid_args("missing field `x`");
        assert_eq!(invalid.code, codes::INVALID_ARGS);
        assert_eq!(invalid.message, "invalid arguments: missing field `x`");
        assert_eq!(BridgeError::stale_handle().code, codes::STALE_HANDLE);
    }
}
