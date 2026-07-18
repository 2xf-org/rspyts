use std::collections::{BTreeMap, BTreeSet};

use crate::ir::{
    BufferElement, DefinitionId, FieldConstraints, FieldDef, ScalarValue, TypeDef, TypeRef,
    TypeShape,
};
use crate::wire::{BufferDtype, WireValue};

use super::BackendError;

const JSON_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const JSON_MIN_SAFE_INTEGER: i64 = -9_007_199_254_740_991;

/// Validates and canonicalizes a wire value against an exact contract type.
///
/// Host runtimes are less precise than Rust. This recursive pass restores the
/// contract's integer signedness, checks every width and buffer dtype, and
/// applies strict named-field and enum validation before Serde sees the value.
pub fn normalize_wire(
    value: &WireValue,
    ty: &TypeRef,
    types: &[TypeDef],
) -> Result<WireValue, BackendError> {
    value.validate().map_err(BackendError::from_boundary)?;
    normalize_at(value, ty, types, "$".to_owned())
}

fn normalize_at(
    value: &WireValue,
    ty: &TypeRef,
    types: &[TypeDef],
    path: String,
) -> Result<WireValue, BackendError> {
    match ty {
        TypeRef::Unit => match value {
            WireValue::Null => Ok(WireValue::Null),
            _ => Err(type_error(&path, "null", wire_kind(value))),
        },
        TypeRef::Bool => match value {
            WireValue::Bool(value) => Ok(WireValue::Bool(*value)),
            _ => Err(type_error(&path, "bool", wire_kind(value))),
        },
        TypeRef::Int { signed, bits } => normalize_integer(value, *signed, *bits, &path),
        TypeRef::Float { bits } => normalize_float(value, *bits, &path),
        TypeRef::String => match value {
            WireValue::String(value) => Ok(WireValue::String(value.clone())),
            _ => Err(type_error(&path, "string", wire_kind(value))),
        },
        TypeRef::DateTime => normalize_datetime(value, &path),
        TypeRef::Json => normalize_json(value, path),
        TypeRef::Option { item } => match value {
            WireValue::Null => Ok(WireValue::Null),
            value => normalize_at(value, item, types, path),
        },
        TypeRef::List { item } => {
            let WireValue::Sequence(values) = value else {
                return Err(type_error(&path, "list", wire_kind(value)));
            };
            values
                .iter()
                .enumerate()
                .map(|(index, value)| normalize_at(value, item, types, format!("{path}[{index}]")))
                .collect::<Result<Vec<_>, _>>()
                .map(WireValue::Sequence)
        }
        TypeRef::Map { value: item } => {
            let WireValue::Object(values) = value else {
                return Err(type_error(&path, "string-keyed map", wire_kind(value)));
            };
            values
                .iter()
                .map(|(key, value)| {
                    normalize_at(value, item, types, format!("{path}.{key}"))
                        .map(|value| (key.clone(), value))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()
                .map(WireValue::Object)
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
            values
                .iter()
                .zip(items)
                .enumerate()
                .map(|(index, (value, item))| {
                    normalize_at(value, item, types, format!("{path}[{index}]"))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(WireValue::Sequence)
        }
        TypeRef::Named { identity } => normalize_named(value, identity, types, path),
        TypeRef::Bytes => match value {
            WireValue::Bytes(value) => Ok(WireValue::Bytes(value.clone())),
            _ => Err(type_error(&path, "bytes", wire_kind(value))),
        },
        TypeRef::FixedBytes { length } => match value {
            WireValue::Bytes(value) if u64::try_from(value.len()).ok().as_ref() == Some(length) => {
                Ok(WireValue::Bytes(value.clone()))
            }
            WireValue::Bytes(value) => Err(type_error(
                &path,
                &format!("{length}-byte array"),
                &format!("{} bytes", value.len()),
            )),
            _ => Err(type_error(
                &path,
                &format!("{length}-byte array"),
                wire_kind(value),
            )),
        },
        TypeRef::Buffer { element } => match value {
            WireValue::Buffer(value) if value.dtype() == element_dtype(*element) => {
                Ok(WireValue::Buffer(value.clone()))
            }
            WireValue::Buffer(value) => Err(type_error(
                &path,
                element_dtype(*element).name(),
                value.dtype().name(),
            )),
            _ => Err(type_error(&path, "numeric buffer", wire_kind(value))),
        },
    }
}

fn normalize_integer(
    value: &WireValue,
    signed: bool,
    bits: u16,
    path: &str,
) -> Result<WireValue, BackendError> {
    if !matches!(bits, 8 | 16 | 32 | 64) {
        return Err(type_error(
            path,
            "8-bit, 16-bit, 32-bit, or 64-bit integer",
            &format!("{bits}-bit integer"),
        ));
    }
    if signed {
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
        Ok(WireValue::I64(value))
    } else {
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
        Ok(WireValue::U64(value))
    }
}

fn normalize_float(value: &WireValue, bits: u16, path: &str) -> Result<WireValue, BackendError> {
    let value = match value {
        WireValue::F64(value) => *value,
        WireValue::I64(value) => *value as f64,
        WireValue::U64(value) => *value as f64,
        value => return Err(type_error(path, "number", wire_kind(value))),
    };
    if !value.is_finite() {
        return Err(BackendError::NonFiniteStructuredFloat);
    }
    match bits {
        64 => Ok(WireValue::F64(value)),
        32 if value.abs() <= f32::MAX as f64 => Ok(WireValue::F64(value)),
        32 => Err(type_error(path, "finite f32", "out-of-range number")),
        _ => Err(type_error(
            path,
            "32-bit or 64-bit float",
            &format!("f{bits}"),
        )),
    }
}

fn normalize_datetime(value: &WireValue, path: &str) -> Result<WireValue, BackendError> {
    let WireValue::String(value) = value else {
        return Err(type_error(path, "aware RFC3339 datetime", wire_kind(value)));
    };
    chrono::DateTime::parse_from_rfc3339(value)
        .map_err(|_| type_error(path, "aware RFC3339 datetime", "invalid or naive datetime"))?;
    Ok(WireValue::String(value.clone()))
}

fn normalize_json(value: &WireValue, path: String) -> Result<WireValue, BackendError> {
    match value {
        WireValue::Null | WireValue::Bool(_) | WireValue::String(_) => Ok(value.clone()),
        WireValue::I64(value)
            if *value >= JSON_MIN_SAFE_INTEGER && *value <= JSON_MAX_SAFE_INTEGER as i64 =>
        {
            Ok(WireValue::I64(*value))
        }
        WireValue::U64(value) if *value <= JSON_MAX_SAFE_INTEGER => Ok(WireValue::U64(*value)),
        WireValue::I64(value) => Err(json_integer_out_of_range(&path, *value)),
        WireValue::U64(value) => Err(json_integer_out_of_range(&path, *value)),
        WireValue::F64(value)
            if value.is_finite()
                && (value.fract() != 0.0
                    || (*value >= JSON_MIN_SAFE_INTEGER as f64
                        && *value <= JSON_MAX_SAFE_INTEGER as f64)) =>
        {
            Ok(WireValue::F64(*value))
        }
        WireValue::F64(value) if value.is_finite() => Err(json_integer_out_of_range(&path, *value)),
        WireValue::F64(_) => Err(BackendError::NonFiniteStructuredFloat),
        WireValue::Sequence(values) => values
            .iter()
            .enumerate()
            .map(|(index, value)| normalize_json(value, format!("{path}[{index}]")))
            .collect::<Result<Vec<_>, _>>()
            .map(WireValue::Sequence),
        WireValue::Object(values) => values
            .iter()
            .map(|(key, value)| {
                normalize_json(value, format!("{path}.{key}")).map(|value| (key.clone(), value))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map(WireValue::Object),
        WireValue::Bytes(_) | WireValue::Buffer(_) => {
            Err(type_error(&path, "JSON value", wire_kind(value)))
        }
    }
}

fn json_integer_out_of_range(path: &str, value: impl std::fmt::Display) -> BackendError {
    type_error(
        path,
        "integer-valued JSON number within JavaScript's safe integer range",
        &value.to_string(),
    )
}

fn normalize_named(
    value: &WireValue,
    identity: &DefinitionId,
    types: &[TypeDef],
    path: String,
) -> Result<WireValue, BackendError> {
    let definition = types
        .iter()
        .find(|definition| definition.owner == identity.owner && definition.id == identity.id)
        .ok_or_else(|| type_error(&path, "known named type", &identity.to_string()))?;
    match &definition.shape {
        TypeShape::Alias { target } => normalize_at(value, target, types, path),
        TypeShape::StringEnum { variants } => {
            let WireValue::String(value) = value else {
                return Err(type_error(&path, "enum string", wire_kind(value)));
            };
            if !variants.iter().any(|variant| variant.wire_name == *value) {
                return Err(type_error(&path, "known enum string", value));
            }
            Ok(WireValue::String(value.clone()))
        }
        TypeShape::Struct { fields } => normalize_fields(value, fields, None, types, path),
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
            normalize_fields(
                value,
                &variant.fields,
                Some((tag.as_str(), variant_name.as_str())),
                types,
                path,
            )
        }
    }
}

fn normalize_fields(
    value: &WireValue,
    fields: &[FieldDef],
    tag: Option<(&str, &str)>,
    types: &[TypeDef],
    path: String,
) -> Result<WireValue, BackendError> {
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

    let mut normalized = BTreeMap::new();
    if let Some((tag, variant)) = tag {
        normalized.insert(tag.to_owned(), WireValue::String(variant.to_owned()));
    }
    for field in fields {
        match values.get(&field.wire_name) {
            Some(value) => {
                let field_path = format!("{path}.{}", field.wire_name);
                let value = normalize_at(value, &field.ty, types, field_path.clone())?;
                validate_constraints(&value, &field.ty, &field.constraints, types, &field_path)?;
                normalized.insert(field.wire_name.clone(), value);
            }
            None if field.default.is_some() => {
                let field_path = format!("{path}.{}", field.wire_name);
                let default = scalar_wire(field.default.as_ref().expect("checked default"));
                let value = normalize_at(&default, &field.ty, types, field_path.clone())?;
                validate_constraints(&value, &field.ty, &field.constraints, types, &field_path)?;
                normalized.insert(field.wire_name.clone(), value);
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
    Ok(WireValue::Object(normalized))
}

fn scalar_wire(value: &ScalarValue) -> WireValue {
    match value {
        ScalarValue::Bool(value) => WireValue::Bool(*value),
        ScalarValue::I64(value) => WireValue::I64(*value),
        ScalarValue::String(value) => WireValue::String(value.clone()),
    }
}

fn validate_constraints(
    value: &WireValue,
    ty: &TypeRef,
    constraints: &FieldConstraints,
    types: &[TypeDef],
    path: &str,
) -> Result<(), BackendError> {
    if matches!(value, WireValue::Null) && type_allows_null(ty, types, &mut BTreeSet::new()) {
        return Ok(());
    }

    if let Some(literal) = &constraints.literal {
        let literal = normalize_at(&scalar_wire(literal), ty, types, path.to_owned())?;
        if value != &literal {
            return Err(type_error(path, "declared literal", wire_kind(value)));
        }
    }

    if constraints.min_length.is_some() || constraints.max_length.is_some() {
        let length = match value {
            WireValue::String(value) => value.chars().count() as u64,
            WireValue::Sequence(value) => value.len() as u64,
            WireValue::Bytes(value) => value.len() as u64,
            _ => {
                return Err(type_error(
                    path,
                    "string, list, or bytes for length constraint",
                    wire_kind(value),
                ));
            }
        };
        if let Some(minimum) = constraints.min_length
            && length < minimum
        {
            return Err(type_error(
                path,
                &format!("length >= {minimum}"),
                &format!("length {length}"),
            ));
        }
        if let Some(maximum) = constraints.max_length
            && length > maximum
        {
            return Err(type_error(
                path,
                &format!("length <= {maximum}"),
                &format!("length {length}"),
            ));
        }
    }

    if let Some(minimum) = constraints.ge {
        let passes = match value {
            WireValue::I64(value) => *value >= minimum,
            WireValue::U64(_) if minimum <= 0 => true,
            WireValue::U64(value) => *value >= minimum as u64,
            _ => {
                return Err(type_error(
                    path,
                    "integer for ge constraint",
                    wire_kind(value),
                ));
            }
        };
        if !passes {
            return Err(type_error(
                path,
                &format!("integer >= {minimum}"),
                wire_kind(value),
            ));
        }
    }
    if let Some(maximum) = constraints.le {
        let passes = match value {
            WireValue::I64(value) => *value <= maximum,
            WireValue::U64(value) if maximum >= 0 => *value <= maximum as u64,
            WireValue::U64(_) => false,
            _ => {
                return Err(type_error(
                    path,
                    "integer for le constraint",
                    wire_kind(value),
                ));
            }
        };
        if !passes {
            return Err(type_error(
                path,
                &format!("integer <= {maximum}"),
                wire_kind(value),
            ));
        }
    }
    Ok(())
}

fn type_allows_null(
    reference: &TypeRef,
    types: &[TypeDef],
    visited: &mut BTreeSet<DefinitionId>,
) -> bool {
    match reference {
        TypeRef::Option { .. } => true,
        TypeRef::Named { identity } if visited.insert(identity.clone()) => types
            .iter()
            .find(|definition| definition.owner == identity.owner && definition.id == identity.id)
            .is_some_and(|definition| match &definition.shape {
                TypeShape::Alias { target } => type_allows_null(target, types, visited),
                _ => false,
            }),
        _ => false,
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
        host: "wire",
        path: path.to_owned(),
        actual: format!("expected {expected}, received {actual}"),
    }
}

impl BackendError {
    pub(crate) fn from_boundary(error: crate::wire::BoundaryError) -> Self {
        match error {
            crate::wire::BoundaryError::NonFiniteStructuredFloat => Self::NonFiniteStructuredFloat,
            other => Self::UnsupportedValue {
                host: "wire",
                path: "$".to_owned(),
                actual: other.to_string(),
            },
        }
    }
}
