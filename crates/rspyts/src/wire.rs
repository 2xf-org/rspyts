//! Backend-neutral values passed across generated host boundaries.
//!
//! The wire model is deliberately richer than JSON: signed and unsigned
//! 64-bit integers remain distinct, bytes are not sequences, and numeric
//! buffers retain their element dtype. Backends translate these values to
//! native host values without first passing through a generic JSON protocol.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};
use thiserror::Error;

/// A scalar dtype carried by a numeric buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BufferDtype {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    F32,
    F64,
}

impl BufferDtype {
    /// The canonical Rust scalar spelling for this dtype.
    pub const fn name(self) -> &'static str {
        match self {
            Self::U8 => "u8",
            Self::I8 => "i8",
            Self::U16 => "u16",
            Self::I16 => "i16",
            Self::U32 => "u32",
            Self::I32 => "i32",
            Self::U64 => "u64",
            Self::I64 => "i64",
            Self::F32 => "f32",
            Self::F64 => "f64",
        }
    }

    /// The width of one element in bits.
    pub const fn bits(self) -> u16 {
        match self {
            Self::U8 | Self::I8 => 8,
            Self::U16 | Self::I16 => 16,
            Self::U32 | Self::I32 | Self::F32 => 32,
            Self::U64 | Self::I64 | Self::F64 => 64,
        }
    }

    /// The width of one element in bytes.
    pub const fn bytes(self) -> usize {
        (self.bits() / 8) as usize
    }

    pub const fn is_integer(self) -> bool {
        !self.is_float()
    }

    pub const fn is_float(self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }

    /// Returns whether an integer dtype is signed, or `None` for floats.
    pub const fn is_signed(self) -> Option<bool> {
        match self {
            Self::I8 | Self::I16 | Self::I32 | Self::I64 => Some(true),
            Self::U8 | Self::U16 | Self::U32 | Self::U64 => Some(false),
            Self::F32 | Self::F64 => None,
        }
    }
}

/// A contiguous numeric buffer whose dtype is retained at the boundary.
#[derive(Debug, Clone, PartialEq)]
pub enum BufferValue {
    U8(Vec<u8>),
    I8(Vec<i8>),
    U16(Vec<u16>),
    I16(Vec<i16>),
    U32(Vec<u32>),
    I32(Vec<i32>),
    U64(Vec<u64>),
    I64(Vec<i64>),
    F32(Vec<f32>),
    F64(Vec<f64>),
}

impl BufferValue {
    pub const fn dtype(&self) -> BufferDtype {
        match self {
            Self::U8(_) => BufferDtype::U8,
            Self::I8(_) => BufferDtype::I8,
            Self::U16(_) => BufferDtype::U16,
            Self::I16(_) => BufferDtype::I16,
            Self::U32(_) => BufferDtype::U32,
            Self::I32(_) => BufferDtype::I32,
            Self::U64(_) => BufferDtype::U64,
            Self::I64(_) => BufferDtype::I64,
            Self::F32(_) => BufferDtype::F32,
            Self::F64(_) => BufferDtype::F64,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::U8(values) => values.len(),
            Self::I8(values) => values.len(),
            Self::U16(values) => values.len(),
            Self::I16(values) => values.len(),
            Self::U32(values) => values.len(),
            Self::I32(values) => values.len(),
            Self::U64(values) => values.len(),
            Self::I64(values) => values.len(),
            Self::F32(values) => values.len(),
            Self::F64(values) => values.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn byte_len(&self) -> usize {
        self.len()
            .checked_mul(self.dtype().bytes())
            .expect("a Rust Vec allocation cannot exceed usize::MAX bytes")
    }
}

macro_rules! buffer_from_vec {
    ($type:ty, $variant:ident) => {
        impl From<Vec<$type>> for BufferValue {
            fn from(values: Vec<$type>) -> Self {
                Self::$variant(values)
            }
        }
    };
}

buffer_from_vec!(u8, U8);
buffer_from_vec!(i8, I8);
buffer_from_vec!(u16, U16);
buffer_from_vec!(i16, I16);
buffer_from_vec!(u32, U32);
buffer_from_vec!(i32, I32);
buffer_from_vec!(u64, U64);
buffer_from_vec!(i64, I64);
buffer_from_vec!(f32, F32);
buffer_from_vec!(f64, F64);

/// The complete value vocabulary shared by the Python and TypeScript backends.
#[derive(Debug, Clone, PartialEq)]
pub enum WireValue {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    String(String),
    Sequence(Vec<WireValue>),
    Object(BTreeMap<String, WireValue>),
    Bytes(Vec<u8>),
    Buffer(BufferValue),
}

impl WireValue {
    /// Validates invariants required by every backend.
    ///
    /// Structured floats must be finite. Numeric buffer floats intentionally
    /// retain all IEEE values, including infinities and NaNs.
    pub fn validate(&self) -> Result<(), BoundaryError> {
        match self {
            Self::F64(value) if !value.is_finite() => Err(BoundaryError::NonFiniteStructuredFloat),
            Self::Sequence(values) => values.iter().try_for_each(Self::validate),
            Self::Object(values) => values.values().try_for_each(Self::validate),
            _ => Ok(()),
        }
    }

    const fn json_incompatible_kind(&self) -> Option<&'static str> {
        match self {
            Self::Bytes(_) => Some("bytes"),
            Self::Buffer(_) => Some("numeric buffer"),
            _ => None,
        }
    }
}

impl From<BufferValue> for WireValue {
    fn from(value: BufferValue) -> Self {
        Self::Buffer(value)
    }
}

/// A rejected or lossy host-boundary conversion.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BoundaryError {
    #[error("structured floating-point values must be finite")]
    NonFiniteStructuredFloat,

    #[error("{kind} has no lossless serde_json::Value representation")]
    NotJsonRepresentable { kind: &'static str },

    #[error("serde_json number is outside the rspyts wire vocabulary")]
    UnsupportedJsonNumber,
}

impl TryFrom<Value> for WireValue {
    type Error = BoundaryError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::Null => Ok(Self::Null),
            Value::Bool(value) => Ok(Self::Bool(value)),
            Value::Number(value) => wire_number(value),
            Value::String(value) => Ok(Self::String(value)),
            Value::Array(values) => values
                .into_iter()
                .map(Self::try_from)
                .collect::<Result<_, _>>()
                .map(Self::Sequence),
            Value::Object(values) => values
                .into_iter()
                .map(|(key, value)| Self::try_from(value).map(|value| (key, value)))
                .collect::<Result<_, _>>()
                .map(Self::Object),
        }
    }
}

impl TryFrom<&WireValue> for Value {
    type Error = BoundaryError;

    fn try_from(value: &WireValue) -> Result<Self, Self::Error> {
        if let Some(kind) = value.json_incompatible_kind() {
            return Err(BoundaryError::NotJsonRepresentable { kind });
        }

        match value {
            WireValue::Null => Ok(Self::Null),
            WireValue::Bool(value) => Ok(Self::Bool(*value)),
            WireValue::I64(value) => Ok(Self::Number(Number::from(*value))),
            WireValue::U64(value) => Ok(Self::Number(Number::from(*value))),
            WireValue::F64(value) => Number::from_f64(*value)
                .map(Self::Number)
                .ok_or(BoundaryError::NonFiniteStructuredFloat),
            WireValue::String(value) => Ok(Self::String(value.clone())),
            WireValue::Sequence(values) => values
                .iter()
                .map(Self::try_from)
                .collect::<Result<_, _>>()
                .map(Self::Array),
            WireValue::Object(values) => values
                .iter()
                .map(|(key, value)| Self::try_from(value).map(|value| (key.clone(), value)))
                .collect::<Result<Map<_, _>, _>>()
                .map(Self::Object),
            WireValue::Bytes(_) | WireValue::Buffer(_) => {
                unreachable!("JSON-incompatible values returned before conversion")
            }
        }
    }
}

impl TryFrom<WireValue> for Value {
    type Error = BoundaryError;

    fn try_from(value: WireValue) -> Result<Self, Self::Error> {
        Self::try_from(&value)
    }
}

fn wire_number(value: Number) -> Result<WireValue, BoundaryError> {
    if let Some(value) = value.as_u64() {
        return Ok(WireValue::U64(value));
    }
    if let Some(value) = value.as_i64() {
        return Ok(WireValue::I64(value));
    }
    value
        .as_f64()
        .filter(|value| value.is_finite())
        .map(WireValue::F64)
        .ok_or(BoundaryError::UnsupportedJsonNumber)
}
