use std::collections::{BTreeMap, BTreeSet};

use js_sys::{Array, BigInt, Object, Reflect};
use wasm_bindgen::{JsCast, JsValue};

use crate::backend::normalize_wire;
use crate::ir::{BufferElement, DefinitionId, FieldDef, TypeDef, TypeRef, TypeShape};
use crate::wire::{BufferDtype, WireValue};

use super::{
    BackendError, decode_buffer, decode_bytes, decode_i64, decode_json, decode_u64, encode_buffer,
    encode_json, is_plain_record, js_description, js_type_name,
};

/// Encodes a wire value according to its exact contract type.
///
/// Unlike [`super::encode`], this API has enough information to preserve
/// nested integer widths and to distinguish bytes from an unsigned-byte
/// numeric buffer. Generated WASM wrappers should always use this API.
pub fn encode_typed(
    value: &WireValue,
    ty: &TypeRef,
    types: &[TypeDef],
) -> Result<JsValue, BackendError> {
    let value = normalize_wire(value, ty, types)?;
    encode_at(&value, ty, types, "$".to_owned())
}

/// Decodes a JavaScript value according to its exact contract type.
///
/// The recursive schema direction removes every ambiguity present in a
/// generic JavaScript value: number versus bigint, signedness, JSON numbers,
/// and bytes versus a `u8` numeric buffer.
pub fn decode_typed(
    value: &JsValue,
    ty: &TypeRef,
    types: &[TypeDef],
) -> Result<WireValue, BackendError> {
    let value = decode_at(value, ty, types, "$".to_owned())?;
    normalize_wire(&value, ty, types)
}

fn encode_at(
    value: &WireValue,
    ty: &TypeRef,
    types: &[TypeDef],
    path: String,
) -> Result<JsValue, BackendError> {
    match ty {
        TypeRef::Unit => match value {
            WireValue::Null => Ok(JsValue::NULL),
            _ => Err(type_error(&path, "null", wire_kind(value))),
        },
        TypeRef::Bool => match value {
            WireValue::Bool(value) => Ok(JsValue::from_bool(*value)),
            _ => Err(type_error(&path, "bool", wire_kind(value))),
        },
        TypeRef::Int { signed, bits } => encode_integer(value, *signed, *bits, &path),
        TypeRef::Float { bits } => encode_float(value, *bits, &path),
        TypeRef::String => match value {
            WireValue::String(value) => Ok(JsValue::from_str(value)),
            _ => Err(type_error(&path, "string", wire_kind(value))),
        },
        TypeRef::DateTime => match value {
            WireValue::String(value) => Ok(JsValue::from_str(value)),
            _ => Err(type_error(
                &path,
                "aware RFC3339 datetime",
                wire_kind(value),
            )),
        },
        TypeRef::Json => encode_json(value),
        TypeRef::Option { item } => match value {
            WireValue::Null => Ok(JsValue::NULL),
            value => encode_at(value, item, types, path),
        },
        TypeRef::List { item } => {
            let WireValue::Sequence(values) = value else {
                return Err(type_error(&path, "list", wire_kind(value)));
            };
            let array = Array::new_with_length(values.len() as u32);
            for (index, value) in values.iter().enumerate() {
                array.set(
                    index as u32,
                    encode_at(value, item, types, format!("{path}[{index}]"))?,
                );
            }
            Ok(array.into())
        }
        TypeRef::Map { value: item } => {
            let WireValue::Object(values) = value else {
                return Err(type_error(&path, "string-keyed map", wire_kind(value)));
            };
            encode_map(values, item, types, path)
        }
        TypeRef::Tuple { items } => {
            let WireValue::Sequence(values) = value else {
                return Err(type_error(&path, "tuple", wire_kind(value)));
            };
            if values.len() != items.len() {
                return Err(type_error(
                    &path,
                    &format!("{}-item tuple", items.len()),
                    &format!("{}-item sequence", values.len()),
                ));
            }
            let array = Array::new_with_length(values.len() as u32);
            for (index, (value, item)) in values.iter().zip(items).enumerate() {
                array.set(
                    index as u32,
                    encode_at(value, item, types, format!("{path}[{index}]"))?,
                );
            }
            Ok(array.into())
        }
        TypeRef::Named { identity } => encode_named(value, identity, types, path),
        TypeRef::Bytes => match value {
            WireValue::Bytes(value) => Ok(js_sys::Uint8Array::from(value.as_slice()).into()),
            _ => Err(type_error(&path, "bytes", wire_kind(value))),
        },
        TypeRef::Buffer { element } => {
            let WireValue::Buffer(buffer) = value else {
                return Err(type_error(&path, "numeric buffer", wire_kind(value)));
            };
            let expected = element_dtype(*element);
            if buffer.dtype() != expected {
                return Err(type_error(&path, expected.name(), buffer.dtype().name()));
            }
            Ok(encode_buffer(buffer))
        }
    }
}

fn decode_at(
    value: &JsValue,
    ty: &TypeRef,
    types: &[TypeDef],
    path: String,
) -> Result<WireValue, BackendError> {
    match ty {
        TypeRef::Unit => {
            if value.is_null() {
                Ok(WireValue::Null)
            } else {
                Err(type_error(&path, "null", &js_type_name(value)))
            }
        }
        TypeRef::Bool => value
            .as_bool()
            .map(WireValue::Bool)
            .ok_or_else(|| type_error(&path, "boolean", &js_type_name(value))),
        TypeRef::Int { signed, bits } => decode_integer(value, *signed, *bits, &path),
        TypeRef::Float { bits } => decode_float(value, *bits, &path),
        TypeRef::String => value
            .as_string()
            .map(WireValue::String)
            .ok_or_else(|| type_error(&path, "string", &js_type_name(value))),
        TypeRef::DateTime => {
            let value = value
                .as_string()
                .ok_or_else(|| type_error(&path, "aware RFC3339 datetime", &js_type_name(value)))?;
            chrono::DateTime::parse_from_rfc3339(&value)
                .map_err(|_| type_error(&path, "aware RFC3339 datetime", "invalid datetime"))?;
            Ok(WireValue::String(value))
        }
        TypeRef::Json => decode_json(value),
        TypeRef::Option { item } => {
            if value.is_null() {
                Ok(WireValue::Null)
            } else {
                decode_at(value, item, types, path)
            }
        }
        TypeRef::List { item } => {
            if !Array::is_array(value) {
                return Err(type_error(&path, "array", &js_type_name(value)));
            }
            let array = value.clone().unchecked_into::<Array>();
            let mut values = Vec::with_capacity(array.length() as usize);
            for index in 0..array.length() {
                values.push(decode_at(
                    &array.get(index),
                    item,
                    types,
                    format!("{path}[{index}]"),
                )?);
            }
            Ok(WireValue::Sequence(values))
        }
        TypeRef::Map { value: item } => {
            let members = object_members(value, &path)?;
            members
                .into_iter()
                .map(|(key, value)| {
                    decode_at(&value, item, types, format!("{path}.{key}"))
                        .map(|value| (key, value))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()
                .map(WireValue::Object)
        }
        TypeRef::Tuple { items } => {
            if !Array::is_array(value) {
                return Err(type_error(&path, "tuple array", &js_type_name(value)));
            }
            let array = value.clone().unchecked_into::<Array>();
            if array.length() as usize != items.len() {
                return Err(type_error(
                    &path,
                    &format!("{}-item tuple", items.len()),
                    &format!("{}-item array", array.length()),
                ));
            }
            items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    decode_at(
                        &array.get(index as u32),
                        item,
                        types,
                        format!("{path}[{index}]"),
                    )
                })
                .collect::<Result<Vec<_>, _>>()
                .map(WireValue::Sequence)
        }
        TypeRef::Named { identity } => decode_named(value, identity, types, path),
        TypeRef::Bytes => decode_bytes(value).map(WireValue::Bytes),
        TypeRef::Buffer { element } => decode_buffer(value, element_dtype(*element))
            .map(WireValue::Buffer)
            .map_err(|error| at_path(error, &path)),
    }
}

fn encode_integer(
    value: &WireValue,
    signed: bool,
    bits: u16,
    path: &str,
) -> Result<JsValue, BackendError> {
    validate_integer_bits(bits, path)?;
    if signed {
        let value = signed_value(value, bits, path)?;
        if bits == 64 {
            Ok(BigInt::from(value).into())
        } else {
            Ok(JsValue::from_f64(value as f64))
        }
    } else {
        let value = unsigned_value(value, bits, path)?;
        if bits == 64 {
            Ok(BigInt::from(value).into())
        } else {
            Ok(JsValue::from_f64(value as f64))
        }
    }
}

fn decode_integer(
    value: &JsValue,
    signed: bool,
    bits: u16,
    path: &str,
) -> Result<WireValue, BackendError> {
    validate_integer_bits(bits, path)?;
    if bits == 64 {
        return if signed {
            decode_i64(value)
                .map(WireValue::I64)
                .map_err(|error| at_path(error, path))
        } else {
            decode_u64(value)
                .map(WireValue::U64)
                .map_err(|error| at_path(error, path))
        };
    }

    let number = value
        .as_f64()
        .filter(|value| value.is_finite() && value.fract() == 0.0)
        .ok_or_else(|| type_error(path, "integer number", &js_type_name(value)))?;
    if signed {
        let integer = number as i64;
        let (minimum, maximum) = signed_range(bits);
        if number != integer as f64 || integer < minimum || integer > maximum {
            return Err(BackendError::IntegerOutOfRange {
                host: "JavaScript number",
                expected: signed_name(bits),
            });
        }
        Ok(WireValue::I64(integer))
    } else {
        if number < 0.0 {
            return Err(BackendError::IntegerOutOfRange {
                host: "JavaScript number",
                expected: unsigned_name(bits),
            });
        }
        let integer = number as u64;
        let maximum = unsigned_max(bits);
        if number != integer as f64 || integer > maximum {
            return Err(BackendError::IntegerOutOfRange {
                host: "JavaScript number",
                expected: unsigned_name(bits),
            });
        }
        Ok(WireValue::U64(integer))
    }
}

fn encode_float(value: &WireValue, bits: u16, path: &str) -> Result<JsValue, BackendError> {
    let WireValue::F64(value) = value else {
        return Err(type_error(path, "floating-point number", wire_kind(value)));
    };
    validate_float(*value, bits, path)?;
    Ok(JsValue::from_f64(*value))
}

fn decode_float(value: &JsValue, bits: u16, path: &str) -> Result<WireValue, BackendError> {
    let value = value
        .as_f64()
        .ok_or_else(|| type_error(path, "number", &js_type_name(value)))?;
    validate_float(value, bits, path)?;
    Ok(WireValue::F64(value))
}

fn validate_float(value: f64, bits: u16, path: &str) -> Result<(), BackendError> {
    if !value.is_finite() {
        return Err(BackendError::NonFiniteStructuredFloat);
    }
    match bits {
        64 => Ok(()),
        32 if value.abs() <= f32::MAX as f64 => Ok(()),
        32 => Err(type_error(path, "finite f32", "out-of-range number")),
        _ => Err(type_error(
            path,
            "32-bit or 64-bit float",
            &format!("f{bits}"),
        )),
    }
}

fn encode_map(
    values: &BTreeMap<String, WireValue>,
    item: &TypeRef,
    types: &[TypeDef],
    path: String,
) -> Result<JsValue, BackendError> {
    let null_prototype = JsValue::NULL.unchecked_into::<Object>();
    let object = Object::create(&null_prototype);
    for (key, value) in values {
        set_member(
            &object,
            key,
            &encode_at(value, item, types, format!("{path}.{key}"))?,
            &path,
        )?;
    }
    Ok(object.into())
}

fn encode_named(
    value: &WireValue,
    identity: &DefinitionId,
    types: &[TypeDef],
    path: String,
) -> Result<JsValue, BackendError> {
    let definition = named_type(identity, types, &path)?;
    match &definition.shape {
        TypeShape::Alias { target } => encode_at(value, target, types, path),
        TypeShape::StringEnum { variants } => {
            let WireValue::String(value) = value else {
                return Err(type_error(&path, "enum string", wire_kind(value)));
            };
            if !variants.iter().any(|variant| variant.wire_name == *value) {
                return Err(type_error(&path, "known enum string", value));
            }
            Ok(JsValue::from_str(value))
        }
        TypeShape::Struct { fields } => encode_fields(value, fields, None, types, path),
        TypeShape::TaggedEnum { tag, variants } => {
            let WireValue::Object(values) = value else {
                return Err(type_error(&path, "tagged enum object", wire_kind(value)));
            };
            let variant_name = values
                .get(tag)
                .and_then(|value| match value {
                    WireValue::String(value) => Some(value),
                    _ => None,
                })
                .ok_or_else(|| type_error(&format!("{path}.{tag}"), "variant string", "missing"))?;
            let variant = variants
                .iter()
                .find(|variant| variant.wire_name == *variant_name)
                .ok_or_else(|| {
                    type_error(&format!("{path}.{tag}"), "known variant", variant_name)
                })?;
            encode_fields(
                value,
                &variant.fields,
                Some((tag, variant_name)),
                types,
                path,
            )
        }
    }
}

fn decode_named(
    value: &JsValue,
    identity: &DefinitionId,
    types: &[TypeDef],
    path: String,
) -> Result<WireValue, BackendError> {
    let definition = named_type(identity, types, &path)?;
    match &definition.shape {
        TypeShape::Alias { target } => decode_at(value, target, types, path),
        TypeShape::StringEnum { variants } => {
            let value = value
                .as_string()
                .ok_or_else(|| type_error(&path, "enum string", &js_type_name(value)))?;
            if !variants.iter().any(|variant| variant.wire_name == value) {
                return Err(type_error(&path, "known enum string", &value));
            }
            Ok(WireValue::String(value))
        }
        TypeShape::Struct { fields } => decode_fields(value, fields, None, types, path),
        TypeShape::TaggedEnum { tag, variants } => {
            let members = object_members(value, &path)?;
            let variant_name = members
                .get(tag)
                .and_then(JsValue::as_string)
                .ok_or_else(|| type_error(&format!("{path}.{tag}"), "variant string", "missing"))?;
            let variant = variants
                .iter()
                .find(|variant| variant.wire_name == variant_name)
                .ok_or_else(|| {
                    type_error(&format!("{path}.{tag}"), "known variant", &variant_name)
                })?;
            decode_members(
                members,
                &variant.fields,
                Some((tag, variant_name)),
                types,
                path,
            )
        }
    }
}

fn encode_fields(
    value: &WireValue,
    fields: &[FieldDef],
    tag: Option<(&str, &str)>,
    types: &[TypeDef],
    path: String,
) -> Result<JsValue, BackendError> {
    let WireValue::Object(values) = value else {
        return Err(type_error(&path, "object", wire_kind(value)));
    };
    let allowed = fields
        .iter()
        .map(|field| field.wire_name.as_str())
        .chain(tag.map(|(tag, _)| tag))
        .collect::<BTreeSet<_>>();
    if let Some(extra) = values.keys().find(|key| !allowed.contains(key.as_str())) {
        return Err(type_error(
            &format!("{path}.{extra}"),
            "declared field",
            "unknown field",
        ));
    }

    let null_prototype = JsValue::NULL.unchecked_into::<Object>();
    let object = Object::create(&null_prototype);
    if let Some((tag, variant)) = tag {
        set_member(&object, tag, &JsValue::from_str(variant), &path)?;
    }
    for field in fields {
        match values.get(&field.wire_name) {
            Some(value) => set_member(
                &object,
                &field.wire_name,
                &encode_at(
                    value,
                    &field.ty,
                    types,
                    format!("{path}.{}", field.wire_name),
                )?,
                &path,
            )?,
            None if field.required => {
                return Err(type_error(
                    &format!("{path}.{}", field.wire_name),
                    "required field",
                    "missing",
                ));
            }
            None => {}
        }
    }
    Ok(object.into())
}

fn decode_fields(
    value: &JsValue,
    fields: &[FieldDef],
    tag: Option<(&str, String)>,
    types: &[TypeDef],
    path: String,
) -> Result<WireValue, BackendError> {
    let members = object_members(value, &path)?;
    decode_members(members, fields, tag, types, path)
}

fn decode_members(
    mut members: BTreeMap<String, JsValue>,
    fields: &[FieldDef],
    tag: Option<(&str, String)>,
    types: &[TypeDef],
    path: String,
) -> Result<WireValue, BackendError> {
    let mut values = BTreeMap::new();
    if let Some((tag, variant)) = tag {
        members.remove(tag);
        values.insert(tag.to_owned(), WireValue::String(variant));
    }
    for field in fields {
        match members.remove(&field.wire_name) {
            Some(value) if value.is_undefined() && !field.required => {}
            Some(value) => {
                values.insert(
                    field.wire_name.clone(),
                    decode_at(
                        &value,
                        &field.ty,
                        types,
                        format!("{path}.{}", field.wire_name),
                    )?,
                );
            }
            None if field.required => {
                return Err(type_error(
                    &format!("{path}.{}", field.wire_name),
                    "required field",
                    "missing",
                ));
            }
            None => {}
        }
    }
    if let Some((extra, _)) = members.into_iter().next() {
        return Err(type_error(
            &format!("{path}.{extra}"),
            "declared field",
            "unknown field",
        ));
    }
    Ok(WireValue::Object(values))
}

fn object_members(value: &JsValue, path: &str) -> Result<BTreeMap<String, JsValue>, BackendError> {
    if !value.is_object() || !is_plain_record(value, path)? {
        return Err(type_error(path, "plain object", &js_type_name(value)));
    }
    let object = value.clone().unchecked_into::<Object>();
    let keys = Reflect::own_keys(&object).map_err(|error| BackendError::ObjectAccess {
        host: "JavaScript",
        path: path.to_owned(),
        message: js_description(&error),
    })?;
    keys.iter()
        .map(|key| {
            let key = key
                .as_string()
                .ok_or_else(|| BackendError::UnsupportedValue {
                    host: "JavaScript",
                    path: path.to_owned(),
                    actual: "object with a symbol key".to_owned(),
                })?;
            let value = Reflect::get(&object, &JsValue::from_str(&key)).map_err(|error| {
                BackendError::ObjectAccess {
                    host: "JavaScript",
                    path: format!("{path}.{key}"),
                    message: js_description(&error),
                }
            })?;
            Ok((key, value))
        })
        .collect()
}

fn set_member(object: &Object, key: &str, value: &JsValue, path: &str) -> Result<(), BackendError> {
    let stored = Reflect::set(object, &JsValue::from_str(key), value).map_err(|error| {
        BackendError::ObjectAccess {
            host: "JavaScript",
            path: format!("{path}.{key}"),
            message: js_description(&error),
        }
    })?;
    if !stored {
        return Err(BackendError::ObjectAccess {
            host: "JavaScript",
            path: format!("{path}.{key}"),
            message: "property assignment returned false".to_owned(),
        });
    }
    Ok(())
}

fn named_type<'a>(
    identity: &DefinitionId,
    types: &'a [TypeDef],
    path: &str,
) -> Result<&'a TypeDef, BackendError> {
    types
        .iter()
        .find(|definition| definition.owner == identity.owner && definition.id == identity.id)
        .ok_or_else(|| type_error(path, "known named type", &identity.to_string()))
}

fn signed_value(value: &WireValue, bits: u16, path: &str) -> Result<i64, BackendError> {
    let value = match value {
        WireValue::I64(value) => *value,
        WireValue::U64(value) => {
            i64::try_from(*value).map_err(|_| BackendError::IntegerOutOfRange {
                host: "wire unsigned integer",
                expected: signed_name(bits),
            })?
        }
        _ => return Err(type_error(path, signed_name(bits), wire_kind(value))),
    };
    let (minimum, maximum) = signed_range(bits);
    if value < minimum || value > maximum {
        return Err(BackendError::IntegerOutOfRange {
            host: "wire integer",
            expected: signed_name(bits),
        });
    }
    Ok(value)
}

fn unsigned_value(value: &WireValue, bits: u16, path: &str) -> Result<u64, BackendError> {
    let value = match value {
        WireValue::U64(value) => *value,
        WireValue::I64(value) => {
            u64::try_from(*value).map_err(|_| BackendError::IntegerOutOfRange {
                host: "wire signed integer",
                expected: unsigned_name(bits),
            })?
        }
        _ => return Err(type_error(path, unsigned_name(bits), wire_kind(value))),
    };
    if value > unsigned_max(bits) {
        return Err(BackendError::IntegerOutOfRange {
            host: "wire integer",
            expected: unsigned_name(bits),
        });
    }
    Ok(value)
}

fn validate_integer_bits(bits: u16, path: &str) -> Result<(), BackendError> {
    if matches!(bits, 8 | 16 | 32 | 64) {
        Ok(())
    } else {
        Err(type_error(
            path,
            "8-bit, 16-bit, 32-bit, or 64-bit integer",
            &format!("{bits}-bit integer"),
        ))
    }
}

const fn signed_range(bits: u16) -> (i64, i64) {
    match bits {
        8 => (i8::MIN as i64, i8::MAX as i64),
        16 => (i16::MIN as i64, i16::MAX as i64),
        32 => (i32::MIN as i64, i32::MAX as i64),
        64 => (i64::MIN, i64::MAX),
        _ => (0, 0),
    }
}

const fn unsigned_max(bits: u16) -> u64 {
    match bits {
        8 => u8::MAX as u64,
        16 => u16::MAX as u64,
        32 => u32::MAX as u64,
        64 => u64::MAX,
        _ => 0,
    }
}

const fn signed_name(bits: u16) -> &'static str {
    match bits {
        8 => "i8",
        16 => "i16",
        32 => "i32",
        64 => "i64",
        _ => "supported signed integer",
    }
}

const fn unsigned_name(bits: u16) -> &'static str {
    match bits {
        8 => "u8",
        16 => "u16",
        32 => "u32",
        64 => "u64",
        _ => "supported unsigned integer",
    }
}

const fn element_dtype(element: BufferElement) -> BufferDtype {
    match element {
        BufferElement::U8 => BufferDtype::U8,
        BufferElement::I8 => BufferDtype::I8,
        BufferElement::U16 => BufferDtype::U16,
        BufferElement::I16 => BufferDtype::I16,
        BufferElement::U32 => BufferDtype::U32,
        BufferElement::I32 => BufferDtype::I32,
        BufferElement::U64 => BufferDtype::U64,
        BufferElement::I64 => BufferDtype::I64,
        BufferElement::F32 => BufferDtype::F32,
        BufferElement::F64 => BufferDtype::F64,
    }
}

fn wire_kind(value: &WireValue) -> &'static str {
    match value {
        WireValue::Null => "null",
        WireValue::Bool(_) => "bool",
        WireValue::I64(_) => "signed integer",
        WireValue::U64(_) => "unsigned integer",
        WireValue::F64(_) => "float",
        WireValue::String(_) => "string",
        WireValue::Sequence(_) => "sequence",
        WireValue::Object(_) => "object",
        WireValue::Bytes(_) => "bytes",
        WireValue::Buffer(_) => "numeric buffer",
    }
}

fn type_error(path: &str, expected: &str, actual: &str) -> BackendError {
    BackendError::UnsupportedValue {
        host: "JavaScript",
        path: path.to_owned(),
        actual: format!("expected {expected}, received {actual}"),
    }
}

fn at_path(error: BackendError, path: &str) -> BackendError {
    match error {
        BackendError::UnsupportedValue { host, actual, .. } => BackendError::UnsupportedValue {
            host,
            path: path.to_owned(),
            actual,
        },
        error => error,
    }
}
