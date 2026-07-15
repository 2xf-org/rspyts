//! Schema-directed conversion between ordinary Rust Serde values and the
//! exact JSON representation used by the foreign runtimes.
//!
//! Rust values keep their natural Serde representation everywhere except at
//! the bridge boundary. In particular, `i64`/`u64` remain JSON numbers during
//! ordinary Rust serialization and are converted to canonical decimal
//! strings only when their [`Ty`] says they cross the wire as exact integers.

use crate::ir::{Dtype, ErrorVariantDecl, FieldDecl, ParamDecl, Ty, TypeDecl};
use crate::registry::type_registry;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Number, Value};
use std::fmt;

const MAX_SAFE_JSON_INTEGER: u64 = 9_007_199_254_740_991;

/// A value does not conform to the bridge schema at the native boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireError {
    message: String,
}

impl WireError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub(crate) fn from_serialization_error(message: impl Into<String>) -> Self {
        Self::new(format!(
            "wire JSON serialization failed: {}",
            message.into()
        ))
    }
}

impl fmt::Display for WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for WireError {}

#[derive(Clone, Copy)]
enum Direction {
    ToWire,
    FromWire,
}

/// Serialize an ordinary Rust value, then project it through its bridge type.
///
/// The caller is responsible for opening the response-tail scope first when
/// the value may contain [`crate::Buf`] or [`crate::Bytes`].
pub fn serialize<T: Serialize>(value: &T, ty: &Ty) -> Result<Value, WireError> {
    let value = serde_json::to_value(value)
        .map_err(|error| WireError::new(format!("Rust serialization failed: {error}")))?;
    normalize(value, ty, Direction::ToWire, "$")
}

/// Project wire JSON through its bridge type, then deserialize a Rust value.
///
/// The caller is responsible for opening the request-tail scope first when
/// the value may contain [`crate::Buf`] or [`crate::Bytes`].
pub fn deserialize<T: DeserializeOwned>(value: Value, ty: &Ty) -> Result<T, WireError> {
    let value = normalize(value, ty, Direction::FromWire, "$")?;
    serde_json::from_value(value)
        .map_err(|error| WireError::new(format!("Rust deserialization failed: {error}")))
}

/// Deserialize a macro-generated argument object using its declared fields.
pub fn deserialize_fields<T: DeserializeOwned>(
    value: Value,
    fields: &[FieldDecl],
) -> Result<T, WireError> {
    let value = normalize_object(value, fields, &[], Direction::FromWire, "$")?;
    serde_json::from_value(value)
        .map_err(|error| WireError::new(format!("Rust deserialization failed: {error}")))
}

/// Deserialize a macro-generated function/method argument object.
/// Parameters are structurally required even when their value type is
/// `Option<T>`; nullability never implies that a call argument may be omitted.
pub fn deserialize_params<T: DeserializeOwned>(
    value: Value,
    params: &[ParamDecl],
) -> Result<T, WireError> {
    let fields = params
        .iter()
        .map(|param| FieldDecl {
            name: param.name.clone(),
            wire_name: param.wire_name.clone(),
            docs: String::new(),
            ty: param.ty.clone(),
            required: true,
        })
        .collect::<Vec<_>>();
    deserialize_fields(value, &fields)
}

/// Convert a serialized error variant's `data` object to its wire shape.
///
/// Error enums are not ordinary data types, so this deliberately uses the
/// variant code to select the field schema rather than accepting `Ty::Ref`
/// through [`serialize`].
pub fn normalize_error_data(
    data: Value,
    error_reference: &str,
    wire_code: &str,
) -> Result<Value, WireError> {
    let declaration = type_registry().declaration_for_ref(
        error_reference,
        &format!("error `{wire_code}` data normalization"),
    );
    let TypeDecl::ErrorEnum { variants, .. } = declaration else {
        return Err(WireError::new(format!(
            "`{error_reference}` is not a bridged error enum"
        )));
    };
    let variant = variants
        .iter()
        .find(|variant| variant.wire_code == wire_code)
        .ok_or_else(|| {
            WireError::new(format!(
                "error enum `{error_reference}` has no variant code `{wire_code}`"
            ))
        })?;
    normalize_error_variant(data, variant)
}

fn normalize_error_variant(data: Value, variant: &ErrorVariantDecl) -> Result<Value, WireError> {
    normalize_object(
        data,
        &variant.fields,
        &[],
        Direction::ToWire,
        &format!("error data for `{}`", variant.wire_code),
    )
}

fn normalize(value: Value, ty: &Ty, direction: Direction, path: &str) -> Result<Value, WireError> {
    match ty {
        Ty::Bool => expect(value, path, "a boolean", Value::is_boolean),
        Ty::U8 => normalize_unsigned(value, u8::MAX as u64, path),
        Ty::U16 => normalize_unsigned(value, u16::MAX as u64, path),
        Ty::U32 => normalize_unsigned(value, u32::MAX as u64, path),
        Ty::I8 => normalize_signed(value, i8::MIN as i64, i8::MAX as i64, path),
        Ty::I16 => normalize_signed(value, i16::MIN as i64, i16::MAX as i64, path),
        Ty::I32 => normalize_signed(value, i32::MIN as i64, i32::MAX as i64, path),
        Ty::I64 => normalize_i64(value, direction, path),
        Ty::U64 => normalize_u64(value, direction, path),
        Ty::F32 => normalize_float(value, Some(f32::MAX as f64), path),
        Ty::F64 => normalize_float(value, None, path),
        Ty::String => expect(value, path, "a string", Value::is_string),
        Ty::Bytes | Ty::Buf { .. } => {
            // Their custom Serde implementations own marker validation and
            // attachment bounds. The schema is what grants marker semantics.
            Ok(value)
        }
        Ty::Unit | Ty::Null => expect(value, path, "null", Value::is_null),
        Ty::Option { inner } => {
            if value.is_null() {
                Ok(value)
            } else {
                normalize(value, inner, direction, path)
            }
        }
        Ty::List { inner } => {
            let Value::Array(values) = value else {
                return Err(expected(path, "an array"));
            };
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| {
                    normalize(value, inner, direction, &format!("{path}[{index}]"))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        Ty::Map { value: value_ty } => {
            let Value::Object(values) = value else {
                return Err(expected(path, "an object"));
            };
            values
                .into_iter()
                .map(|(key, value)| {
                    let normalized = normalize(
                        value,
                        value_ty,
                        direction,
                        &format!("{path}.{}", display_key(&key)),
                    )?;
                    Ok((key, normalized))
                })
                .collect::<Result<Map<_, _>, _>>()
                .map(Value::Object)
        }
        Ty::Tuple { items } => {
            let Value::Array(values) = value else {
                return Err(expected(path, "an array"));
            };
            if values.len() != items.len() {
                return Err(WireError::new(format!(
                    "{path}: expected tuple length {}, got {}",
                    items.len(),
                    values.len()
                )));
            }
            values
                .into_iter()
                .zip(items)
                .enumerate()
                .map(|(index, (value, item_ty))| {
                    normalize(value, item_ty, direction, &format!("{path}[{index}]"))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        Ty::Ref { name } => normalize_ref(value, name, direction, path),
        Ty::Json => {
            validate_portable_json(&value, path)?;
            let mut value = value;
            canonicalize_json_signed_zero(&mut value);
            Ok(value)
        }
        Ty::Slice { dt } => Err(WireError::new(format!(
            "{path}: borrowed slice `{}` cannot appear in structured JSON",
            dtype_name(*dt)
        ))),
    }
}

fn normalize_ref(
    value: Value,
    reference: &str,
    direction: Direction,
    path: &str,
) -> Result<Value, WireError> {
    let declaration =
        type_registry().declaration_for_ref(reference, &format!("wire value at {path}"));
    match declaration {
        TypeDecl::Newtype { inner, .. } => normalize(value, inner, direction, path),
        TypeDecl::Struct { fields, .. } => normalize_object(value, fields, &[], direction, path),
        TypeDecl::Enum { tag, variants, .. } => {
            let Value::Object(values) = value else {
                return Err(expected(path, "an object"));
            };
            let wire_variant = values.get(tag).and_then(Value::as_str).ok_or_else(|| {
                WireError::new(format!("{path}: missing or non-string enum tag `{tag}`"))
            })?;
            let variant = variants
                .iter()
                .find(|variant| variant.wire_name == wire_variant)
                .ok_or_else(|| {
                    WireError::new(format!("{path}: unknown enum tag value `{wire_variant}`"))
                })?;
            normalize_object(
                Value::Object(values),
                &variant.fields,
                std::slice::from_ref(tag),
                direction,
                path,
            )
        }
        TypeDecl::StringEnum { variants, .. } => {
            let Value::String(variant) = value else {
                return Err(expected(path, "a string"));
            };
            if variants.iter().any(|known| known.wire_name == variant) {
                Ok(Value::String(variant))
            } else {
                Err(WireError::new(format!(
                    "{path}: unknown string-enum value `{variant}`"
                )))
            }
        }
        TypeDecl::ErrorEnum { .. } => Err(WireError::new(format!(
            "{path}: error enum `{reference}` cannot be used as ordinary data"
        ))),
    }
}

fn normalize_object(
    value: Value,
    fields: &[FieldDecl],
    passthrough_keys: &[String],
    direction: Direction,
    path: &str,
) -> Result<Value, WireError> {
    let Value::Object(mut values) = value else {
        return Err(expected(path, "an object"));
    };

    for key in values.keys() {
        if !passthrough_keys.iter().any(|allowed| allowed == key)
            && !fields.iter().any(|field| field.wire_name == *key)
        {
            return Err(WireError::new(format!("{path}: unknown field `{key}`")));
        }
    }

    for field in fields {
        let Some(value) = values.remove(&field.wire_name) else {
            if field.required {
                return Err(WireError::new(format!(
                    "{path}: missing required field `{}`",
                    field.wire_name
                )));
            }
            continue;
        };
        let value = normalize(
            value,
            &field.ty,
            direction,
            &format!("{path}.{}", display_key(&field.wire_name)),
        )?;
        values.insert(field.wire_name.clone(), value);
    }
    Ok(Value::Object(values))
}

fn normalize_i64(value: Value, direction: Direction, path: &str) -> Result<Value, WireError> {
    match direction {
        Direction::ToWire => {
            let Value::Number(number) = value else {
                return Err(expected(path, "a signed 64-bit integer"));
            };
            let integer = number
                .as_i64()
                .ok_or_else(|| expected(path, "a signed 64-bit integer"))?;
            Ok(Value::String(integer.to_string()))
        }
        Direction::FromWire => {
            let Value::String(text) = value else {
                return Err(expected(path, "a canonical signed decimal string"));
            };
            let integer = text
                .parse::<i64>()
                .map_err(|_| expected(path, "a canonical signed decimal string"))?;
            if integer.to_string() != text {
                return Err(expected(path, "a canonical signed decimal string"));
            }
            Ok(Value::Number(Number::from(integer)))
        }
    }
}

fn normalize_u64(value: Value, direction: Direction, path: &str) -> Result<Value, WireError> {
    match direction {
        Direction::ToWire => {
            let Value::Number(number) = value else {
                return Err(expected(path, "an unsigned 64-bit integer"));
            };
            let integer = number
                .as_u64()
                .ok_or_else(|| expected(path, "an unsigned 64-bit integer"))?;
            Ok(Value::String(integer.to_string()))
        }
        Direction::FromWire => {
            let Value::String(text) = value else {
                return Err(expected(path, "a canonical unsigned decimal string"));
            };
            let integer = text
                .parse::<u64>()
                .map_err(|_| expected(path, "a canonical unsigned decimal string"))?;
            if integer.to_string() != text {
                return Err(expected(path, "a canonical unsigned decimal string"));
            }
            Ok(Value::Number(Number::from(integer)))
        }
    }
}

fn normalize_unsigned(value: Value, maximum: u64, path: &str) -> Result<Value, WireError> {
    let Value::Number(number) = &value else {
        return Err(expected(path, "an unsigned integer"));
    };
    if number.as_u64().is_some_and(|integer| integer <= maximum) {
        Ok(value)
    } else {
        Err(WireError::new(format!(
            "{path}: expected an unsigned integer in 0..={maximum}"
        )))
    }
}

fn normalize_signed(
    value: Value,
    minimum: i64,
    maximum: i64,
    path: &str,
) -> Result<Value, WireError> {
    let Value::Number(number) = &value else {
        return Err(expected(path, "a signed integer"));
    };
    if number
        .as_i64()
        .is_some_and(|integer| (minimum..=maximum).contains(&integer))
    {
        Ok(value)
    } else {
        Err(WireError::new(format!(
            "{path}: expected a signed integer in {minimum}..={maximum}"
        )))
    }
}

fn normalize_float(value: Value, maximum: Option<f64>, path: &str) -> Result<Value, WireError> {
    let Value::Number(number) = &value else {
        return Err(expected(path, "a finite number"));
    };
    let Some(float) = number.as_f64() else {
        return Err(expected(path, "a finite number"));
    };
    if !float.is_finite() || maximum.is_some_and(|maximum| float.abs() > maximum) {
        return Err(expected(path, "a finite in-range number"));
    }
    Ok(value)
}

fn expect(
    value: Value,
    path: &str,
    description: &str,
    predicate: impl FnOnce(&Value) -> bool,
) -> Result<Value, WireError> {
    if predicate(&value) {
        Ok(value)
    } else {
        Err(expected(path, description))
    }
}

fn expected(path: &str, description: &str) -> WireError {
    WireError::new(format!("{path}: expected {description}"))
}

fn validate_portable_json(value: &Value, path: &str) -> Result<(), WireError> {
    match value {
        Value::Null | Value::Bool(_) | Value::String(_) => Ok(()),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                if integer.unsigned_abs() <= MAX_SAFE_JSON_INTEGER {
                    Ok(())
                } else {
                    Err(WireError::new(format!(
                        "{path}: JSON integer {integer} is not exactly representable in JavaScript"
                    )))
                }
            } else if let Some(integer) = number.as_u64() {
                if integer <= MAX_SAFE_JSON_INTEGER {
                    Ok(())
                } else {
                    Err(WireError::new(format!(
                        "{path}: JSON integer {integer} is not exactly representable in JavaScript"
                    )))
                }
            } else if let Some(float) = number.as_f64() {
                if !float.is_finite() {
                    return Err(WireError::new(format!(
                        "{path}: JSON contains a non-finite number"
                    )));
                }
                if float.fract() == 0.0 && float.abs() > MAX_SAFE_JSON_INTEGER as f64 {
                    return Err(WireError::new(format!(
                        "{path}: JSON integral number {float} is not exactly representable in JavaScript"
                    )));
                }
                Ok(())
            } else {
                Err(WireError::new(format!(
                    "{path}: JSON contains a non-finite number"
                )))
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                validate_portable_json(value, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        Value::Object(values) => {
            for (key, value) in values {
                validate_portable_json(value, &format!("{path}.{}", display_key(key)))?;
            }
            Ok(())
        }
    }
}

fn canonicalize_json_signed_zero(value: &mut Value) {
    match value {
        Value::Number(number) => {
            if number
                .as_f64()
                .is_some_and(|value| value == 0.0 && value.is_sign_negative())
            {
                *number = Number::from_f64(0.0).expect("zero is a finite JSON number");
            }
        }
        Value::Array(values) => {
            for value in values {
                canonicalize_json_signed_zero(value);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                canonicalize_json_signed_zero(value);
            }
        }
        Value::Null | Value::Bool(_) | Value::String(_) => {}
    }
}

fn dtype_name(dtype: Dtype) -> &'static str {
    match dtype {
        Dtype::U8 => "u8",
        Dtype::I8 => "i8",
        Dtype::U16 => "u16",
        Dtype::I16 => "i16",
        Dtype::U32 => "u32",
        Dtype::I32 => "i32",
        Dtype::U64 => "u64",
        Dtype::I64 => "i64",
        Dtype::F32 => "f32",
        Dtype::F64 => "f64",
    }
}

fn display_key(key: &str) -> String {
    if key
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        key.to_string()
    } else {
        format!("[{key:?}]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_integers_are_strings_only_at_the_wire_boundary() {
        assert_eq!(
            serde_json::to_value(i64::MIN).unwrap(),
            Value::from(i64::MIN)
        );
        assert_eq!(
            serialize(&i64::MIN, &Ty::I64).unwrap(),
            Value::String(i64::MIN.to_string())
        );
        assert_eq!(
            deserialize::<i64>(Value::String(i64::MAX.to_string()), &Ty::I64).unwrap(),
            i64::MAX
        );
        assert_eq!(
            serialize(&u64::MAX, &Ty::U64).unwrap(),
            Value::String(u64::MAX.to_string())
        );
        assert_eq!(
            deserialize::<u64>(Value::String(u64::MAX.to_string()), &Ty::U64).unwrap(),
            u64::MAX
        );
    }

    #[test]
    fn exact_integer_wire_strings_must_be_canonical() {
        for invalid in ["01", "+1", "-0", " 1", "1.0"] {
            assert!(deserialize::<i64>(Value::String(invalid.into()), &Ty::I64).is_err());
        }
        for invalid in ["01", "+1", "-0", "-1", " 1", "1.0"] {
            assert!(deserialize::<u64>(Value::String(invalid.into()), &Ty::U64).is_err());
        }
    }

    #[test]
    fn nested_containers_are_normalized_recursively() {
        let ty = Ty::Map {
            value: Box::new(Ty::List {
                inner: Box::new(Ty::Tuple {
                    items: vec![
                        Ty::I64,
                        Ty::Option {
                            inner: Box::new(Ty::U64),
                        },
                    ],
                }),
            }),
        };
        let domain = serde_json::json!({"numbers": [[-4, 9], [5, null]]});
        let wire = normalize(domain.clone(), &ty, Direction::ToWire, "$").unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"numbers": [["-4", "9"], ["5", null]]})
        );
        assert_eq!(
            normalize(wire, &ty, Direction::FromWire, "$").unwrap(),
            domain
        );
    }

    #[test]
    fn field_presence_is_independent_from_option_nullability() {
        let fields = vec![
            FieldDecl {
                name: "optional".into(),
                wire_name: "optional".into(),
                docs: String::new(),
                ty: Ty::Option {
                    inner: Box::new(Ty::I64),
                },
                required: false,
            },
            FieldDecl {
                name: "required".into(),
                wire_name: "required".into(),
                docs: String::new(),
                ty: Ty::Option {
                    inner: Box::new(Ty::U64),
                },
                required: true,
            },
        ];
        assert!(
            normalize_object(
                serde_json::json!({}),
                &fields,
                &[],
                Direction::FromWire,
                "$"
            )
            .is_err()
        );
        assert_eq!(
            normalize_object(
                serde_json::json!({"required": null}),
                &fields,
                &[],
                Direction::FromWire,
                "$"
            )
            .unwrap(),
            serde_json::json!({"required": null})
        );
        assert!(
            normalize_object(
                serde_json::json!({"required": null, "extra": 1}),
                &fields,
                &[],
                Direction::FromWire,
                "$"
            )
            .is_err()
        );
    }

    #[test]
    fn json_is_terminal_but_rejects_unsafe_integral_numbers() {
        let marker_like = serde_json::json!({
            "__rspyts_buf__": {"off": 0, "len": 1, "dt": "u8"},
            "nested": ["9223372036854775807"],
            "negativeZero": -0.0
        });
        let normalized = serialize(&marker_like, &Ty::Json).unwrap();
        assert_eq!(normalized["__rspyts_buf__"], marker_like["__rspyts_buf__"]);
        assert_eq!(normalized["nested"], marker_like["nested"]);
        assert!(
            !normalized["negativeZero"]
                .as_f64()
                .unwrap()
                .is_sign_negative()
        );
        let unsafe_integer = Value::Number(Number::from(MAX_SAFE_JSON_INTEGER + 1));
        assert!(serialize(&unsafe_integer, &Ty::Json).is_err());
        for unsafe_float in [MAX_SAFE_JSON_INTEGER as f64 + 1.0, 1e100] {
            let value = Value::Number(Number::from_f64(unsafe_float).unwrap());
            assert!(serialize(&value, &Ty::Json).is_err());
            assert!(deserialize::<Value>(value, &Ty::Json).is_err());
        }
    }
}
