//! Browser/WASM conversions using native JavaScript values and typed arrays.

use std::collections::BTreeMap;

use js_sys::{
    Array, BigInt, BigInt64Array, BigUint64Array, Float32Array, Float64Array, Int8Array,
    Int16Array, Int32Array, Object, Reflect, Uint8Array, Uint16Array, Uint32Array,
};
use wasm_bindgen::{JsCast, JsValue};

use crate::wire::{BufferDtype, BufferValue, WireValue};

use super::BackendError;

mod typed;

pub use typed::{decode_typed, encode_typed};

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const MIN_SAFE_INTEGER: i64 = -9_007_199_254_740_991;

impl From<BackendError> for JsValue {
    fn from(error: BackendError) -> Self {
        let error = js_sys::Error::new(&error.to_string());
        error.set_name("RspytsBoundaryError");
        error.into()
    }
}

/// Converts a wire value to native JavaScript values.
///
/// Exact 64-bit integers are JavaScript `bigint`, bytes are `Uint8Array`, and
/// every numeric buffer is the matching JavaScript typed array.
pub fn encode(value: &WireValue) -> Result<JsValue, BackendError> {
    value.validate().map_err(BackendError::from_boundary)?;
    encode_at(value)
}

fn encode_at(value: &WireValue) -> Result<JsValue, BackendError> {
    Ok(match value {
        WireValue::Null => JsValue::NULL,
        WireValue::Bool(value) => JsValue::from_bool(*value),
        WireValue::I64(value) => BigInt::from(*value).into(),
        WireValue::U64(value) => BigInt::from(*value).into(),
        WireValue::F64(value) => JsValue::from_f64(*value),
        WireValue::String(value) => JsValue::from_str(value),
        WireValue::Sequence(values) => {
            let array = Array::new_with_length(values.len() as u32);
            for (index, value) in values.iter().enumerate() {
                array.set(index as u32, encode_at(value)?);
            }
            array.into()
        }
        WireValue::Object(values) => {
            // A null prototype makes every string key, including `__proto__`,
            // an ordinary own property and prevents prototype mutation.
            let null_prototype = JsValue::NULL.unchecked_into::<Object>();
            let object = Object::create(&null_prototype);
            for (key, value) in values {
                Reflect::set(&object, &JsValue::from_str(key), &encode_at(value)?).map_err(
                    |error| BackendError::ObjectAccess {
                        host: "JavaScript",
                        path: format!("$.{key}"),
                        message: js_description(&error),
                    },
                )?;
            }
            object.into()
        }
        WireValue::Bytes(bytes) => Uint8Array::from(bytes.as_slice()).into(),
        WireValue::Buffer(buffer) => encode_buffer(buffer),
    })
}

/// Converts native JavaScript values to the complete wire vocabulary.
///
/// The generic convention treats `Uint8Array` as bytes and non-negative
/// `bigint` as `u64`. Generated type-directed wrappers must call
/// [`decode_buffer`], [`decode_i64`], or [`decode_u64`] where the contract
/// supplies the otherwise-ambiguous expected type.
pub fn decode(value: &JsValue) -> Result<WireValue, BackendError> {
    decode_at(value, "$".to_owned())
}

/// Converts the JSON subset of the wire vocabulary to actual JavaScript JSON.
///
/// JSON numbers remain JavaScript `number`, never `bigint`. Integer values
/// outside JavaScript's exact safe range are rejected rather than silently
/// rounded or made incompatible with `JSON.stringify`.
pub fn encode_json(value: &WireValue) -> Result<JsValue, BackendError> {
    encode_json_at(value, "$".to_owned())
}

fn encode_json_at(value: &WireValue, path: String) -> Result<JsValue, BackendError> {
    Ok(match value {
        WireValue::Null => JsValue::NULL,
        WireValue::Bool(value) => JsValue::from_bool(*value),
        WireValue::I64(value)
            if *value >= MIN_SAFE_INTEGER && *value <= MAX_SAFE_INTEGER as i64 =>
        {
            JsValue::from_f64(*value as f64)
        }
        WireValue::U64(value) if *value <= MAX_SAFE_INTEGER => JsValue::from_f64(*value as f64),
        WireValue::I64(_) | WireValue::U64(_) => {
            return Err(BackendError::IntegerOutOfRange {
                host: "JavaScript JSON",
                expected: "an exact safe integer",
            });
        }
        WireValue::F64(value)
            if value.is_finite()
                && (value.fract() != 0.0
                    || (*value >= MIN_SAFE_INTEGER as f64
                        && *value <= MAX_SAFE_INTEGER as f64)) =>
        {
            JsValue::from_f64(*value)
        }
        WireValue::F64(value) if value.is_finite() => {
            return Err(BackendError::IntegerOutOfRange {
                host: "JavaScript JSON",
                expected: "an exact safe integer",
            });
        }
        WireValue::F64(_) => return Err(BackendError::NonFiniteStructuredFloat),
        WireValue::String(value) => JsValue::from_str(value),
        WireValue::Sequence(values) => {
            let array = Array::new_with_length(values.len() as u32);
            for (index, value) in values.iter().enumerate() {
                array.set(
                    index as u32,
                    encode_json_at(value, format!("{path}[{index}]"))?,
                );
            }
            array.into()
        }
        WireValue::Object(values) => {
            let null_prototype = JsValue::NULL.unchecked_into::<Object>();
            let object = Object::create(&null_prototype);
            for (key, value) in values {
                Reflect::set(
                    &object,
                    &JsValue::from_str(key),
                    &encode_json_at(value, format!("{path}.{key}"))?,
                )
                .map_err(|error| BackendError::ObjectAccess {
                    host: "JavaScript",
                    path: format!("{path}.{key}"),
                    message: js_description(&error),
                })?;
            }
            object.into()
        }
        WireValue::Bytes(_) | WireValue::Buffer(_) => {
            return Err(BackendError::UnsupportedValue {
                host: "JavaScript JSON",
                path,
                actual: "bytes or numeric buffer".to_owned(),
            });
        }
    })
}

/// Converts JavaScript JSON to the JSON subset of the wire vocabulary.
///
/// Integer-valued numbers are retained as signed/unsigned integer wire values
/// while fractional values remain `F64`. BigInt, typed arrays, undefined,
/// symbols, functions, and class instances are rejected.
pub fn decode_json(value: &JsValue) -> Result<WireValue, BackendError> {
    decode_json_at(value, "$".to_owned())
}

fn decode_json_at(value: &JsValue, path: String) -> Result<WireValue, BackendError> {
    if value.is_null() {
        return Ok(WireValue::Null);
    }
    if let Some(value) = value.as_bool() {
        return Ok(WireValue::Bool(value));
    }
    if let Some(value) = value.as_f64() {
        if !value.is_finite() {
            return Err(BackendError::NonFiniteStructuredFloat);
        }
        if value.fract() == 0.0 {
            if value < MIN_SAFE_INTEGER as f64 || value > MAX_SAFE_INTEGER as f64 {
                return Err(BackendError::IntegerOutOfRange {
                    host: "JavaScript JSON",
                    expected: "an exact safe integer",
                });
            }
            return if value < 0.0 {
                Ok(WireValue::I64(value as i64))
            } else {
                Ok(WireValue::U64(value as u64))
            };
        }
        return Ok(WireValue::F64(value));
    }
    if let Some(value) = value.as_string() {
        return Ok(WireValue::String(value));
    }
    if Array::is_array(value) {
        let array = value.clone().unchecked_into::<Array>();
        let mut values = Vec::with_capacity(array.length() as usize);
        for index in 0..array.length() {
            values.push(decode_json_at(
                &array.get(index),
                format!("{path}[{index}]"),
            )?);
        }
        return Ok(WireValue::Sequence(values));
    }
    if value.is_object() && is_plain_record(value, &path)? {
        let object = decode_object_json(value, path)?;
        return Ok(WireValue::Object(object));
    }
    Err(BackendError::UnsupportedValue {
        host: "JavaScript JSON",
        path,
        actual: js_type_name(value),
    })
}

fn decode_at(value: &JsValue, path: String) -> Result<WireValue, BackendError> {
    if value.is_null() {
        return Ok(WireValue::Null);
    }
    if let Some(value) = value.as_bool() {
        return Ok(WireValue::Bool(value));
    }
    if value.is_bigint() {
        let bigint = value.clone().unchecked_into::<BigInt>();
        let decimal = bigint
            .to_string(10)
            .map_err(|error| BackendError::ObjectAccess {
                host: "JavaScript",
                path,
                message: js_description(error.as_ref()),
            })?
            .as_string()
            .expect("BigInt.toString always returns a JavaScript string");
        return if decimal.starts_with('-') {
            decimal.parse::<i64>().map(WireValue::I64).map_err(|_| {
                BackendError::IntegerOutOfRange {
                    host: "JavaScript bigint",
                    expected: "i64",
                }
            })
        } else {
            decimal.parse::<u64>().map(WireValue::U64).map_err(|_| {
                BackendError::IntegerOutOfRange {
                    host: "JavaScript bigint",
                    expected: "u64",
                }
            })
        };
    }
    if let Some(value) = value.as_f64() {
        if !value.is_finite() {
            return Err(BackendError::NonFiniteStructuredFloat);
        }
        return Ok(WireValue::F64(value));
    }
    if let Some(value) = value.as_string() {
        return Ok(WireValue::String(value));
    }
    if value.is_instance_of::<Uint8Array>() {
        return Ok(WireValue::Bytes(
            value.clone().unchecked_into::<Uint8Array>().to_vec(),
        ));
    }
    if let Some(buffer) = infer_non_u8_buffer(value) {
        return buffer.map(WireValue::Buffer);
    }
    if Array::is_array(value) {
        let array = value.clone().unchecked_into::<Array>();
        let mut values = Vec::with_capacity(array.length() as usize);
        for index in 0..array.length() {
            values.push(decode_at(&array.get(index), format!("{path}[{index}]"))?);
        }
        return Ok(WireValue::Sequence(values));
    }
    if value.is_object() {
        if !is_plain_record(value, &path)? {
            return Err(BackendError::UnsupportedValue {
                host: "JavaScript",
                path,
                actual: format!("non-plain {} object", js_type_name(value)),
            });
        }
        return decode_object(value, path).map(WireValue::Object);
    }

    Err(BackendError::UnsupportedValue {
        host: "JavaScript",
        path,
        actual: js_type_name(value),
    })
}

fn decode_object(
    value: &JsValue,
    path: String,
) -> Result<BTreeMap<String, WireValue>, BackendError> {
    let object = value.clone().unchecked_into::<Object>();
    let keys = Reflect::own_keys(&object).map_err(|error| BackendError::ObjectAccess {
        host: "JavaScript",
        path: path.clone(),
        message: js_description(&error),
    })?;
    let mut values = BTreeMap::new();
    for key in keys.iter() {
        let key = key
            .as_string()
            .ok_or_else(|| BackendError::UnsupportedValue {
                host: "JavaScript",
                path: path.clone(),
                actual: "record with a symbol key".to_owned(),
            })?;
        let member = Reflect::get(&object, &JsValue::from_str(&key)).map_err(|error| {
            BackendError::ObjectAccess {
                host: "JavaScript",
                path: format!("{path}.{key}"),
                message: js_description(&error),
            }
        })?;
        values.insert(key.clone(), decode_at(&member, format!("{path}.{key}"))?);
    }
    Ok(values)
}

fn decode_object_json(
    value: &JsValue,
    path: String,
) -> Result<BTreeMap<String, WireValue>, BackendError> {
    let object = value.clone().unchecked_into::<Object>();
    let keys = Reflect::own_keys(&object).map_err(|error| BackendError::ObjectAccess {
        host: "JavaScript JSON",
        path: path.clone(),
        message: js_description(&error),
    })?;
    let mut values = BTreeMap::new();
    for key in keys.iter() {
        let key = key
            .as_string()
            .ok_or_else(|| BackendError::UnsupportedValue {
                host: "JavaScript JSON",
                path: path.clone(),
                actual: "object with a symbol key".to_owned(),
            })?;
        let member = Reflect::get(&object, &JsValue::from_str(&key)).map_err(|error| {
            BackendError::ObjectAccess {
                host: "JavaScript JSON",
                path: format!("{path}.{key}"),
                message: js_description(&error),
            }
        })?;
        values.insert(
            key.clone(),
            decode_json_at(&member, format!("{path}.{key}"))?,
        );
    }
    Ok(values)
}

fn is_plain_record(value: &JsValue, path: &str) -> Result<bool, BackendError> {
    let prototype =
        Reflect::get_prototype_of(value).map_err(|error| BackendError::ObjectAccess {
            host: "JavaScript",
            path: path.to_owned(),
            message: js_description(&error),
        })?;
    if JsValue::from(prototype.clone()).is_null() {
        return Ok(true);
    }
    let plain = Object::new();
    Ok(prototype == Object::get_prototype_of(plain.as_ref()))
}

/// Converts a numeric buffer directly to the corresponding typed array.
pub fn encode_buffer(buffer: &BufferValue) -> JsValue {
    match buffer {
        BufferValue::U8(values) => Uint8Array::from(values.as_slice()).into(),
        BufferValue::I8(values) => Int8Array::from(values.as_slice()).into(),
        BufferValue::U16(values) => Uint16Array::from(values.as_slice()).into(),
        BufferValue::I16(values) => Int16Array::from(values.as_slice()).into(),
        BufferValue::U32(values) => Uint32Array::from(values.as_slice()).into(),
        BufferValue::I32(values) => Int32Array::from(values.as_slice()).into(),
        BufferValue::U64(values) => BigUint64Array::from(values.as_slice()).into(),
        BufferValue::I64(values) => BigInt64Array::from(values.as_slice()).into(),
        BufferValue::F32(values) => Float32Array::from(values.as_slice()).into(),
        BufferValue::F64(values) => Float64Array::from(values.as_slice()).into(),
    }
}

/// Converts a JavaScript typed array to the expected numeric-buffer dtype.
///
/// Requiring the expected dtype preserves the bytes-versus-u8-buffer
/// distinction and gives a useful error when a caller supplies the wrong view.
pub fn decode_buffer(value: &JsValue, expected: BufferDtype) -> Result<BufferValue, BackendError> {
    let decoded = match expected {
        BufferDtype::U8 if value.is_instance_of::<Uint8Array>() => {
            BufferValue::U8(value.clone().unchecked_into::<Uint8Array>().to_vec())
        }
        BufferDtype::I8 if value.is_instance_of::<Int8Array>() => {
            BufferValue::I8(value.clone().unchecked_into::<Int8Array>().to_vec())
        }
        BufferDtype::U16 if value.is_instance_of::<Uint16Array>() => {
            BufferValue::U16(value.clone().unchecked_into::<Uint16Array>().to_vec())
        }
        BufferDtype::I16 if value.is_instance_of::<Int16Array>() => {
            BufferValue::I16(value.clone().unchecked_into::<Int16Array>().to_vec())
        }
        BufferDtype::U32 if value.is_instance_of::<Uint32Array>() => {
            BufferValue::U32(value.clone().unchecked_into::<Uint32Array>().to_vec())
        }
        BufferDtype::I32 if value.is_instance_of::<Int32Array>() => {
            BufferValue::I32(value.clone().unchecked_into::<Int32Array>().to_vec())
        }
        BufferDtype::U64 if value.is_instance_of::<BigUint64Array>() => {
            BufferValue::U64(value.clone().unchecked_into::<BigUint64Array>().to_vec())
        }
        BufferDtype::I64 if value.is_instance_of::<BigInt64Array>() => {
            BufferValue::I64(value.clone().unchecked_into::<BigInt64Array>().to_vec())
        }
        BufferDtype::F32 if value.is_instance_of::<Float32Array>() => {
            BufferValue::F32(value.clone().unchecked_into::<Float32Array>().to_vec())
        }
        BufferDtype::F64 if value.is_instance_of::<Float64Array>() => {
            BufferValue::F64(value.clone().unchecked_into::<Float64Array>().to_vec())
        }
        _ => {
            return Err(BackendError::InvalidBuffer(format!(
                "expected {} typed array, received {}",
                expected.name(),
                js_type_name(value)
            )));
        }
    };
    Ok(decoded)
}

/// Converts a `Uint8Array` to owned Rust bytes.
pub fn decode_bytes(value: &JsValue) -> Result<Vec<u8>, BackendError> {
    if !value.is_instance_of::<Uint8Array>() {
        return Err(BackendError::UnsupportedValue {
            host: "JavaScript",
            path: "$".to_owned(),
            actual: format!("expected Uint8Array, received {}", js_type_name(value)),
        });
    }
    Ok(value.clone().unchecked_into::<Uint8Array>().to_vec())
}

/// Decodes an exact signed 64-bit JavaScript `bigint`.
pub fn decode_i64(value: &JsValue) -> Result<i64, BackendError> {
    decode_bigint_decimal(value)?
        .parse::<i64>()
        .map_err(|_| BackendError::IntegerOutOfRange {
            host: "JavaScript bigint",
            expected: "i64",
        })
}

/// Decodes an exact unsigned 64-bit JavaScript `bigint`.
pub fn decode_u64(value: &JsValue) -> Result<u64, BackendError> {
    decode_bigint_decimal(value)?
        .parse::<u64>()
        .map_err(|_| BackendError::IntegerOutOfRange {
            host: "JavaScript bigint",
            expected: "u64",
        })
}

fn decode_bigint_decimal(value: &JsValue) -> Result<String, BackendError> {
    if !value.is_bigint() {
        return Err(BackendError::UnsupportedValue {
            host: "JavaScript",
            path: "$".to_owned(),
            actual: format!("expected bigint, received {}", js_type_name(value)),
        });
    }
    value
        .clone()
        .unchecked_into::<BigInt>()
        .to_string(10)
        .map_err(|error| BackendError::ObjectAccess {
            host: "JavaScript",
            path: "$".to_owned(),
            message: js_description(error.as_ref()),
        })?
        .as_string()
        .ok_or_else(|| BackendError::ObjectAccess {
            host: "JavaScript",
            path: "$".to_owned(),
            message: "BigInt.toString did not return a string".to_owned(),
        })
}

fn infer_non_u8_buffer(value: &JsValue) -> Option<Result<BufferValue, BackendError>> {
    const DTYPES: [BufferDtype; 9] = [
        BufferDtype::I8,
        BufferDtype::U16,
        BufferDtype::I16,
        BufferDtype::U32,
        BufferDtype::I32,
        BufferDtype::U64,
        BufferDtype::I64,
        BufferDtype::F32,
        BufferDtype::F64,
    ];
    DTYPES
        .into_iter()
        .find_map(|dtype| decode_buffer(value, dtype).ok().map(Ok))
}

fn js_type_name(value: &JsValue) -> String {
    if value.is_instance_of::<Uint8Array>() {
        return "Uint8Array".to_owned();
    }
    if value.is_instance_of::<Int8Array>() {
        return "Int8Array".to_owned();
    }
    if value.is_instance_of::<Uint16Array>() {
        return "Uint16Array".to_owned();
    }
    if value.is_instance_of::<Int16Array>() {
        return "Int16Array".to_owned();
    }
    if value.is_instance_of::<Uint32Array>() {
        return "Uint32Array".to_owned();
    }
    if value.is_instance_of::<Int32Array>() {
        return "Int32Array".to_owned();
    }
    if value.is_instance_of::<BigUint64Array>() {
        return "BigUint64Array".to_owned();
    }
    if value.is_instance_of::<BigInt64Array>() {
        return "BigInt64Array".to_owned();
    }
    if value.is_instance_of::<Float32Array>() {
        return "Float32Array".to_owned();
    }
    if value.is_instance_of::<Float64Array>() {
        return "Float64Array".to_owned();
    }
    value
        .js_typeof()
        .as_string()
        .unwrap_or_else(|| "unknown".to_owned())
}

fn js_description(value: &JsValue) -> String {
    value
        .as_string()
        .unwrap_or_else(|| format!("JavaScript {}", js_type_name(value)))
}
