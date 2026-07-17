use crate::wire::{BufferDtype, BufferValue};

use super::BackendError;

/// Encodes a numeric buffer in canonical little-endian byte order.
pub fn encode_buffer_bytes(buffer: &BufferValue) -> Vec<u8> {
    macro_rules! encode {
        ($values:expr) => {{
            let values = $values;
            let mut bytes = Vec::with_capacity(buffer.byte_len());
            for value in values {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            bytes
        }};
    }

    match buffer {
        BufferValue::U8(values) => values.clone(),
        BufferValue::I8(values) => values.iter().map(|value| *value as u8).collect(),
        BufferValue::U16(values) => encode!(values),
        BufferValue::I16(values) => encode!(values),
        BufferValue::U32(values) => encode!(values),
        BufferValue::I32(values) => encode!(values),
        BufferValue::U64(values) => encode!(values),
        BufferValue::I64(values) => encode!(values),
        BufferValue::F32(values) => encode!(values),
        BufferValue::F64(values) => encode!(values),
    }
}

/// Decodes canonical little-endian bytes into an owned numeric buffer.
pub fn decode_buffer_bytes(
    dtype: BufferDtype,
    length: usize,
    bytes: &[u8],
) -> Result<BufferValue, BackendError> {
    let expected = length
        .checked_mul(dtype.bytes())
        .ok_or_else(|| BackendError::InvalidBuffer("byte length overflow".to_owned()))?;
    if bytes.len() != expected {
        return Err(BackendError::InvalidBuffer(format!(
            "dtype {} with length {length} requires {expected} bytes, received {}",
            dtype.name(),
            bytes.len()
        )));
    }

    macro_rules! decode {
        ($type:ty, $variant:ident) => {{
            let values = bytes
                .chunks_exact(core::mem::size_of::<$type>())
                .map(|chunk| {
                    let array: [u8; core::mem::size_of::<$type>()] = chunk
                        .try_into()
                        .expect("chunks_exact returns one complete scalar");
                    <$type>::from_le_bytes(array)
                })
                .collect();
            BufferValue::$variant(values)
        }};
    }

    Ok(match dtype {
        BufferDtype::U8 => BufferValue::U8(bytes.to_vec()),
        BufferDtype::I8 => BufferValue::I8(bytes.iter().map(|value| *value as i8).collect()),
        BufferDtype::U16 => decode!(u16, U16),
        BufferDtype::I16 => decode!(i16, I16),
        BufferDtype::U32 => decode!(u32, U32),
        BufferDtype::I32 => decode!(i32, I32),
        BufferDtype::U64 => decode!(u64, U64),
        BufferDtype::I64 => decode!(i64, I64),
        BufferDtype::F32 => decode!(f32, F32),
        BufferDtype::F64 => decode!(f64, F64),
    })
}
