//! Lossless host-language conversions for the rspyts wire vocabulary.
//!
//! These conversions are intentionally separate from Serde. Serde's generic
//! data model cannot preserve the distinction between bytes and numeric
//! buffers, nor can it promise exact 64-bit integer behavior in JavaScript.

mod buffer;
mod schema;

pub use buffer::{decode_buffer_bytes, encode_buffer_bytes};
pub use schema::normalize_wire;

use thiserror::Error;

/// A value rejected at a Python or JavaScript boundary.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BackendError {
    #[error("structured floating-point values must be finite")]
    NonFiniteStructuredFloat,

    #[error("{host} integer is outside the {expected} range")]
    IntegerOutOfRange {
        host: &'static str,
        expected: &'static str,
    },

    #[error("unsupported {host} value at {path}: {actual}")]
    UnsupportedValue {
        host: &'static str,
        path: String,
        actual: String,
    },

    #[error("invalid numeric buffer payload: {0}")]
    InvalidBuffer(String),

    #[error("failed to read {host} object at {path}: {message}")]
    ObjectAccess {
        host: &'static str,
        path: String,
        message: String,
    },
}

#[cfg(all(feature = "python", not(target_arch = "wasm32")))]
pub mod python;

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub mod typescript;
