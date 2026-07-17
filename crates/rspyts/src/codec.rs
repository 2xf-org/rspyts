//! Schema-directed Rust value conversion for generated boundary wrappers.
//!
//! This module deliberately does not pass through `serde_json::Value`.
//! JavaScript-safe JSON is only one leaf in the contract vocabulary; bytes,
//! exact 64-bit integers, and typed buffers require a richer intermediate
//! representation. The private Serde value below retains those distinctions
//! until the contract schema can coerce every nested value to [`WireValue`].

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::Serialize;
use serde::de::{
    self, DeserializeOwned, DeserializeSeed, EnumAccess, IntoDeserializer, MapAccess, SeqAccess,
    VariantAccess, Visitor,
};
use serde::ser::{
    self, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant,
};
use thiserror::Error;

use crate::backend::normalize_wire;
use crate::ir::{BufferElement, DefinitionId, FieldDef, TypeDef, TypeRef, TypeShape};
use crate::wire::{BufferDtype, BufferValue, WireValue};

/// A rejected Rust-to-wire or wire-to-Rust conversion.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct CodecError {
    message: String,
}

impl CodecError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn schema(path: &str, expected: &str, actual: &str) -> Self {
        Self::new(format!(
            "contract mismatch at {path}: expected {expected}, received {actual}"
        ))
    }
}

impl ser::Error for CodecError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self::new(message.to_string())
    }
}

impl de::Error for CodecError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self::new(message.to_string())
    }
}

/// Serializes a Rust value and coerces it through the exact contract schema.
pub fn encode<T: Serialize + ?Sized>(
    value: &T,
    ty: &TypeRef,
    types: &[TypeDef],
) -> Result<WireValue, CodecError> {
    let raw = value.serialize(RawSerializer)?;
    let wire = raw_to_wire(raw, ty, types, "$".to_owned())?;
    normalize_wire(&wire, ty, types).map_err(|error| CodecError::new(error.to_string()))
}

/// Validates a wire value against the schema and deserializes the Rust value.
pub fn decode<T: DeserializeOwned>(
    value: WireValue,
    ty: &TypeRef,
    types: &[TypeDef],
) -> Result<T, CodecError> {
    let normalized =
        normalize_wire(&value, ty, types).map_err(|error| CodecError::new(error.to_string()))?;
    T::deserialize(RawValue::from(normalized))
}

#[derive(Debug, Clone, PartialEq)]
enum RawValue {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    F32(f32),
    F64(f64),
    Char(char),
    String(String),
    Sequence(Vec<RawValue>),
    Object(BTreeMap<String, RawValue>),
    Bytes(Vec<u8>),
    Buffer(BufferValue),
}

impl From<WireValue> for RawValue {
    fn from(value: WireValue) -> Self {
        match value {
            WireValue::Null => Self::Null,
            WireValue::Bool(value) => Self::Bool(value),
            WireValue::I64(value) => Self::I64(value),
            WireValue::U64(value) => Self::U64(value),
            WireValue::F64(value) => Self::F64(value),
            WireValue::String(value) => Self::String(value),
            WireValue::Sequence(values) => {
                Self::Sequence(values.into_iter().map(Self::from).collect())
            }
            WireValue::Object(values) => Self::Object(
                values
                    .into_iter()
                    .map(|(key, value)| (key, Self::from(value)))
                    .collect(),
            ),
            WireValue::Bytes(value) => Self::Bytes(value),
            WireValue::Buffer(value) => Self::Buffer(value),
        }
    }
}

struct RawSerializer;

impl ser::Serializer for RawSerializer {
    type Ok = RawValue;
    type Error = CodecError;
    type SerializeSeq = SequenceSerializer;
    type SerializeTuple = SequenceSerializer;
    type SerializeTupleStruct = SequenceSerializer;
    type SerializeTupleVariant = TupleVariantSerializer;
    type SerializeMap = ObjectSerializer;
    type SerializeStruct = ObjectSerializer;
    type SerializeStructVariant = StructVariantSerializer;

    fn serialize_bool(self, value: bool) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::Bool(value))
    }

    fn serialize_i8(self, value: i8) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::I64(value.into()))
    }

    fn serialize_i16(self, value: i16) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::I64(value.into()))
    }

    fn serialize_i32(self, value: i32) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::I64(value.into()))
    }

    fn serialize_i64(self, value: i64) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::I64(value))
    }

    fn serialize_i128(self, value: i128) -> Result<Self::Ok, Self::Error> {
        i64::try_from(value)
            .map(RawValue::I64)
            .map_err(|_| CodecError::new("i128 value is outside the rspyts i64 vocabulary"))
    }

    fn serialize_u8(self, value: u8) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::U64(value.into()))
    }

    fn serialize_u16(self, value: u16) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::U64(value.into()))
    }

    fn serialize_u32(self, value: u32) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::U64(value.into()))
    }

    fn serialize_u64(self, value: u64) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::U64(value))
    }

    fn serialize_u128(self, value: u128) -> Result<Self::Ok, Self::Error> {
        u64::try_from(value)
            .map(RawValue::U64)
            .map_err(|_| CodecError::new("u128 value is outside the rspyts u64 vocabulary"))
    }

    fn serialize_f32(self, value: f32) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::F32(value))
    }

    fn serialize_f64(self, value: f64) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::F64(value))
    }

    fn serialize_char(self, value: char) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::Char(value))
    }

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::String(value.to_owned()))
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::Bytes(value.to_vec()))
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::Null)
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::Null)
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::Null)
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::String(variant.to_owned()))
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::Object(BTreeMap::from([(
            variant.to_owned(),
            value.serialize(RawSerializer)?,
        )])))
    }

    fn serialize_seq(self, length: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Ok(SequenceSerializer::new(length))
    }

    fn serialize_tuple(self, length: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Ok(SequenceSerializer::new(Some(length)))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        length: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Ok(SequenceSerializer::new(Some(length)))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        length: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Ok(TupleVariantSerializer {
            variant: variant.to_owned(),
            values: Vec::with_capacity(length),
        })
    }

    fn serialize_map(self, length: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Ok(ObjectSerializer::new(length))
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        length: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(ObjectSerializer::new(Some(length)))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        length: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Ok(StructVariantSerializer {
            variant: variant.to_owned(),
            object: ObjectSerializer::new(Some(length)),
        })
    }

    fn collect_str<T: fmt::Display + ?Sized>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::String(value.to_string()))
    }

    fn is_human_readable(&self) -> bool {
        true
    }
}

struct SequenceSerializer {
    values: Vec<RawValue>,
}

impl SequenceSerializer {
    fn new(length: Option<usize>) -> Self {
        Self {
            values: Vec::with_capacity(length.unwrap_or(0)),
        }
    }

    fn push<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), CodecError> {
        self.values.push(value.serialize(RawSerializer)?);
        Ok(())
    }

    fn finish(self) -> RawValue {
        RawValue::Sequence(self.values)
    }
}

impl SerializeSeq for SequenceSerializer {
    type Ok = RawValue;
    type Error = CodecError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.push(value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(self.finish())
    }
}

impl SerializeTuple for SequenceSerializer {
    type Ok = RawValue;
    type Error = CodecError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.push(value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(self.finish())
    }
}

impl SerializeTupleStruct for SequenceSerializer {
    type Ok = RawValue;
    type Error = CodecError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.push(value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(self.finish())
    }
}

struct TupleVariantSerializer {
    variant: String,
    values: Vec<RawValue>,
}

impl SerializeTupleVariant for TupleVariantSerializer {
    type Ok = RawValue;
    type Error = CodecError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.values.push(value.serialize(RawSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::Object(BTreeMap::from([(
            self.variant,
            RawValue::Sequence(self.values),
        )])))
    }
}

struct ObjectSerializer {
    values: BTreeMap<String, RawValue>,
    next_key: Option<String>,
}

impl ObjectSerializer {
    fn new(_length: Option<usize>) -> Self {
        Self {
            values: BTreeMap::new(),
            next_key: None,
        }
    }

    fn insert<T: Serialize + ?Sized>(
        &mut self,
        key: impl Into<String>,
        value: &T,
    ) -> Result<(), CodecError> {
        let key = key.into();
        if self
            .values
            .insert(key.clone(), value.serialize(RawSerializer)?)
            .is_some()
        {
            return Err(CodecError::new(format!("duplicate object key {key:?}")));
        }
        Ok(())
    }

    fn finish(self) -> Result<RawValue, CodecError> {
        if self.next_key.is_some() {
            return Err(CodecError::new("map ended before serializing a value"));
        }
        Ok(RawValue::Object(self.values))
    }
}

impl SerializeMap for ObjectSerializer {
    type Ok = RawValue;
    type Error = CodecError;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Self::Error> {
        if self.next_key.is_some() {
            return Err(CodecError::new(
                "map serialized two keys without an intervening value",
            ));
        }
        self.next_key = Some(key.serialize(StringKeySerializer)?);
        Ok(())
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        let key = self
            .next_key
            .take()
            .ok_or_else(|| CodecError::new("map value was serialized before its key"))?;
        self.insert(key, value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.finish()
    }
}

impl SerializeStruct for ObjectSerializer {
    type Ok = RawValue;
    type Error = CodecError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.insert(key, value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.finish()
    }
}

struct StructVariantSerializer {
    variant: String,
    object: ObjectSerializer,
}

impl SerializeStructVariant for StructVariantSerializer {
    type Ok = RawValue;
    type Error = CodecError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.object.insert(key, value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(RawValue::Object(BTreeMap::from([(
            self.variant,
            self.object.finish()?,
        )])))
    }
}

struct StringKeySerializer;

impl ser::Serializer for StringKeySerializer {
    type Ok = String;
    type Error = CodecError;
    type SerializeSeq = ser::Impossible<String, CodecError>;
    type SerializeTuple = ser::Impossible<String, CodecError>;
    type SerializeTupleStruct = ser::Impossible<String, CodecError>;
    type SerializeTupleVariant = ser::Impossible<String, CodecError>;
    type SerializeMap = ser::Impossible<String, CodecError>;
    type SerializeStruct = ser::Impossible<String, CodecError>;
    type SerializeStructVariant = ser::Impossible<String, CodecError>;

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        Ok(value.to_owned())
    }

    fn serialize_char(self, value: char) -> Result<Self::Ok, Self::Error> {
        Ok(value.to_string())
    }

    fn collect_str<T: fmt::Display + ?Sized>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        Ok(value.to_string())
    }

    fn serialize_bool(self, _value: bool) -> Result<Self::Ok, Self::Error> {
        Err(CodecError::new("map keys must be strings"))
    }

    fn serialize_i8(self, _value: i8) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_i16(self, _value: i16) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_i32(self, _value: i32) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_i64(self, _value: i64) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_i128(self, _value: i128) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_u8(self, _value: u8) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_u16(self, _value: u16) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_u32(self, _value: u32) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_u64(self, _value: u64) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_u128(self, _value: u128) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_f32(self, _value: f32) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_f64(self, _value: f64) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_some<T: Serialize + ?Sized>(self, _value: &T) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.invalid()
    }
    fn serialize_seq(self, _length: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.invalid()
    }
    fn serialize_tuple(self, _length: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.invalid()
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.invalid()
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.invalid()
    }
    fn serialize_map(self, _length: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        self.invalid()
    }
    fn serialize_struct(
        self,
        _name: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        self.invalid()
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.invalid()
    }
}

impl StringKeySerializer {
    fn invalid<T>(self) -> Result<T, CodecError> {
        Err(CodecError::new("map keys must be strings"))
    }
}

fn raw_to_wire(
    value: RawValue,
    ty: &TypeRef,
    types: &[TypeDef],
    path: String,
) -> Result<WireValue, CodecError> {
    match ty {
        TypeRef::Unit => match value {
            RawValue::Null => Ok(WireValue::Null),
            value => Err(CodecError::schema(&path, "unit/null", raw_kind(&value))),
        },
        TypeRef::Bool => match value {
            RawValue::Bool(value) => Ok(WireValue::Bool(value)),
            value => Err(CodecError::schema(&path, "bool", raw_kind(&value))),
        },
        TypeRef::Int { signed, bits } => raw_integer(value, *signed, *bits, &path),
        TypeRef::Float { bits } => raw_float(value, *bits, false, &path).map(WireValue::F64),
        TypeRef::String => match value {
            RawValue::String(value) => Ok(WireValue::String(value)),
            RawValue::Char(value) => Ok(WireValue::String(value.to_string())),
            value => Err(CodecError::schema(&path, "string", raw_kind(&value))),
        },
        TypeRef::DateTime => match value {
            RawValue::String(value) => chrono::DateTime::parse_from_rfc3339(&value)
                .map(|_| WireValue::String(value))
                .map_err(|_| {
                    CodecError::schema(&path, "aware RFC3339 datetime", "invalid datetime")
                }),
            value => Err(CodecError::schema(
                &path,
                "aware RFC3339 datetime",
                raw_kind(&value),
            )),
        },
        TypeRef::Json => raw_json(value, path),
        TypeRef::Option { item } => match value {
            RawValue::Null => Ok(WireValue::Null),
            value => raw_to_wire(value, item, types, path),
        },
        TypeRef::List { item } => {
            let RawValue::Sequence(values) = value else {
                return Err(CodecError::schema(&path, "sequence", raw_kind(&value)));
            };
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| raw_to_wire(value, item, types, format!("{path}[{index}]")))
                .collect::<Result<Vec<_>, _>>()
                .map(WireValue::Sequence)
        }
        TypeRef::Map { value: item } => {
            let RawValue::Object(values) = value else {
                return Err(CodecError::schema(&path, "object/map", raw_kind(&value)));
            };
            values
                .into_iter()
                .map(|(key, value)| {
                    raw_to_wire(value, item, types, format!("{path}.{key}"))
                        .map(|value| (key, value))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()
                .map(WireValue::Object)
        }
        TypeRef::Tuple { items } => {
            let RawValue::Sequence(values) = value else {
                return Err(CodecError::schema(&path, "tuple", raw_kind(&value)));
            };
            if values.len() != items.len() {
                return Err(CodecError::schema(
                    &path,
                    &format!("{}-item tuple", items.len()),
                    &format!("{}-item sequence", values.len()),
                ));
            }
            values
                .into_iter()
                .zip(items)
                .enumerate()
                .map(|(index, (value, item))| {
                    raw_to_wire(value, item, types, format!("{path}[{index}]"))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(WireValue::Sequence)
        }
        TypeRef::Named { identity } => raw_named(value, identity, types, path),
        TypeRef::Bytes => raw_bytes(value, path).map(WireValue::Bytes),
        TypeRef::Buffer { element } => raw_buffer(value, *element, path).map(WireValue::Buffer),
    }
}

fn raw_integer(
    value: RawValue,
    signed: bool,
    bits: u16,
    path: &str,
) -> Result<WireValue, CodecError> {
    if !matches!(bits, 8 | 16 | 32 | 64) {
        return Err(CodecError::schema(
            path,
            "8-bit, 16-bit, 32-bit, or 64-bit integer",
            &format!("{bits}-bit integer"),
        ));
    }
    if signed {
        let value = match value {
            RawValue::I64(value) => value,
            RawValue::U64(value) => i64::try_from(value).map_err(|_| {
                CodecError::schema(path, signed_name(bits), "out-of-range unsigned integer")
            })?,
            value => {
                return Err(CodecError::schema(
                    path,
                    signed_name(bits),
                    raw_kind(&value),
                ));
            }
        };
        let (minimum, maximum) = signed_range(bits);
        if value < minimum || value > maximum {
            return Err(CodecError::schema(
                path,
                signed_name(bits),
                "out-of-range integer",
            ));
        }
        Ok(WireValue::I64(value))
    } else {
        let value = match value {
            RawValue::U64(value) => value,
            RawValue::I64(value) => u64::try_from(value)
                .map_err(|_| CodecError::schema(path, unsigned_name(bits), "negative integer"))?,
            value => {
                return Err(CodecError::schema(
                    path,
                    unsigned_name(bits),
                    raw_kind(&value),
                ));
            }
        };
        if value > unsigned_max(bits) {
            return Err(CodecError::schema(
                path,
                unsigned_name(bits),
                "out-of-range integer",
            ));
        }
        Ok(WireValue::U64(value))
    }
}

fn raw_float(
    value: RawValue,
    bits: u16,
    allow_non_finite: bool,
    path: &str,
) -> Result<f64, CodecError> {
    let value = match value {
        RawValue::F32(value) => f64::from(value),
        RawValue::F64(value) => value,
        RawValue::I64(value) => value as f64,
        RawValue::U64(value) => value as f64,
        value => return Err(CodecError::schema(path, "number", raw_kind(&value))),
    };
    if !allow_non_finite && !value.is_finite() {
        return Err(CodecError::schema(path, "finite float", "NaN or infinity"));
    }
    match bits {
        64 => Ok(value),
        32 if !value.is_finite() || value.abs() <= f32::MAX as f64 => Ok(value),
        32 => Err(CodecError::schema(path, "f32", "out-of-range float")),
        _ => Err(CodecError::schema(
            path,
            "32-bit or 64-bit float",
            &format!("f{bits}"),
        )),
    }
}

fn raw_json(value: RawValue, path: String) -> Result<WireValue, CodecError> {
    match value {
        RawValue::Null => Ok(WireValue::Null),
        RawValue::Bool(value) => Ok(WireValue::Bool(value)),
        RawValue::I64(value) => Ok(WireValue::I64(value)),
        RawValue::U64(value) => Ok(WireValue::U64(value)),
        RawValue::F32(value) if value.is_finite() => Ok(WireValue::F64(f64::from(value))),
        RawValue::F64(value) if value.is_finite() => Ok(WireValue::F64(value)),
        RawValue::F32(_) | RawValue::F64(_) => Err(CodecError::schema(
            &path,
            "finite JSON number",
            "NaN or infinity",
        )),
        RawValue::Char(value) => Ok(WireValue::String(value.to_string())),
        RawValue::String(value) => Ok(WireValue::String(value)),
        RawValue::Sequence(values) => values
            .into_iter()
            .enumerate()
            .map(|(index, value)| raw_json(value, format!("{path}[{index}]")))
            .collect::<Result<Vec<_>, _>>()
            .map(WireValue::Sequence),
        RawValue::Object(values) => values
            .into_iter()
            .map(|(key, value)| raw_json(value, format!("{path}.{key}")).map(|value| (key, value)))
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map(WireValue::Object),
        RawValue::Bytes(_) | RawValue::Buffer(_) => Err(CodecError::schema(
            &path,
            "JSON value",
            "bytes or numeric buffer",
        )),
    }
}

fn raw_bytes(value: RawValue, path: String) -> Result<Vec<u8>, CodecError> {
    match value {
        RawValue::Bytes(value) => Ok(value),
        RawValue::Sequence(values) => values
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                match raw_integer(value, false, 8, &format!("{path}[{index}]"))? {
                    WireValue::U64(value) => Ok(value as u8),
                    _ => unreachable!("unsigned 8-bit normalization returns U64"),
                }
            })
            .collect(),
        value => Err(CodecError::schema(&path, "bytes", raw_kind(&value))),
    }
}

fn raw_buffer(
    value: RawValue,
    element: BufferElement,
    path: String,
) -> Result<BufferValue, CodecError> {
    if let RawValue::Buffer(value) = value {
        let expected = element_dtype(element);
        if value.dtype() == expected {
            return Ok(value);
        }
        return Err(CodecError::schema(
            &path,
            expected.name(),
            value.dtype().name(),
        ));
    }

    if matches!(element, BufferElement::U8)
        && let RawValue::Bytes(value) = value
    {
        return Ok(BufferValue::U8(value));
    }

    let RawValue::Sequence(values) = value else {
        return Err(CodecError::schema(
            &path,
            "numeric sequence",
            raw_kind(&value),
        ));
    };

    macro_rules! integers {
        ($signed:literal, $bits:literal, $variant:ident, $type:ty) => {{
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| {
                    let value = raw_integer(value, $signed, $bits, &format!("{path}[{index}]"))?;
                    match value {
                        WireValue::I64(value) => Ok(value as $type),
                        WireValue::U64(value) => Ok(value as $type),
                        _ => unreachable!("integer normalization returns an integer"),
                    }
                })
                .collect::<Result<Vec<_>, CodecError>>()
                .map(BufferValue::$variant)
        }};
    }

    macro_rules! floats {
        ($bits:literal, $variant:ident, $type:ty) => {{
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| {
                    raw_float(value, $bits, true, &format!("{path}[{index}]"))
                        .map(|value| value as $type)
                })
                .collect::<Result<Vec<_>, CodecError>>()
                .map(BufferValue::$variant)
        }};
    }

    match element {
        BufferElement::U8 => integers!(false, 8, U8, u8),
        BufferElement::I8 => integers!(true, 8, I8, i8),
        BufferElement::U16 => integers!(false, 16, U16, u16),
        BufferElement::I16 => integers!(true, 16, I16, i16),
        BufferElement::U32 => integers!(false, 32, U32, u32),
        BufferElement::I32 => integers!(true, 32, I32, i32),
        BufferElement::U64 => integers!(false, 64, U64, u64),
        BufferElement::I64 => integers!(true, 64, I64, i64),
        BufferElement::F32 => values
            .into_iter()
            .enumerate()
            .map(|(index, value)| match value {
                // Retain the original f32 bits, including NaN payload bits.
                RawValue::F32(value) => Ok(value),
                value => raw_float(value, 32, true, &format!("{path}[{index}]"))
                    .map(|value| value as f32),
            })
            .collect::<Result<Vec<_>, CodecError>>()
            .map(BufferValue::F32),
        BufferElement::F64 => floats!(64, F64, f64),
    }
}

fn raw_named(
    value: RawValue,
    identity: &DefinitionId,
    types: &[TypeDef],
    path: String,
) -> Result<WireValue, CodecError> {
    let definition = types
        .iter()
        .find(|definition| definition.owner == identity.owner && definition.id == identity.id)
        .ok_or_else(|| CodecError::schema(&path, "known named type", &identity.to_string()))?;
    match &definition.shape {
        TypeShape::Alias { target } => raw_to_wire(value, target, types, path),
        TypeShape::StringEnum { variants } => {
            let RawValue::String(value) = value else {
                return Err(CodecError::schema(&path, "enum string", raw_kind(&value)));
            };
            if !variants.iter().any(|variant| variant.wire_name == value) {
                return Err(CodecError::schema(&path, "known enum string", &value));
            }
            Ok(WireValue::String(value))
        }
        TypeShape::Struct { fields } => raw_fields(value, fields, None, types, path),
        TypeShape::TaggedEnum { tag, variants } => {
            let RawValue::Object(values) = &value else {
                return Err(CodecError::schema(
                    &path,
                    "tagged enum object",
                    raw_kind(&value),
                ));
            };
            let variant_name = values
                .get(tag)
                .and_then(|value| match value {
                    RawValue::String(value) => Some(value),
                    _ => None,
                })
                .cloned()
                .ok_or_else(|| {
                    CodecError::schema(&format!("{path}.{tag}"), "variant string", "missing")
                })?;
            let variant = variants
                .iter()
                .find(|variant| variant.wire_name == variant_name)
                .ok_or_else(|| {
                    CodecError::schema(&format!("{path}.{tag}"), "known variant", &variant_name)
                })?;
            raw_fields(
                value,
                &variant.fields,
                Some((tag.as_str(), variant_name.as_str())),
                types,
                path,
            )
        }
    }
}

fn raw_fields(
    value: RawValue,
    fields: &[FieldDef],
    tag: Option<(&str, &str)>,
    types: &[TypeDef],
    path: String,
) -> Result<WireValue, CodecError> {
    let RawValue::Object(mut values) = value else {
        return Err(CodecError::schema(&path, "object", raw_kind(&value)));
    };
    let allowed = fields
        .iter()
        .map(|field| field.wire_name.as_str())
        .chain(tag.map(|(tag, _)| tag))
        .collect::<BTreeSet<_>>();
    if let Some(extra) = values.keys().find(|key| !allowed.contains(key.as_str())) {
        return Err(CodecError::schema(
            &format!("{path}.{extra}"),
            "declared field",
            "unknown field",
        ));
    }

    let mut wire = BTreeMap::new();
    if let Some((tag, variant)) = tag {
        values.remove(tag);
        wire.insert(tag.to_owned(), WireValue::String(variant.to_owned()));
    }
    for field in fields {
        match values.remove(&field.wire_name) {
            Some(value) => {
                wire.insert(
                    field.wire_name.clone(),
                    raw_to_wire(
                        value,
                        &field.ty,
                        types,
                        format!("{path}.{}", field.wire_name),
                    )?,
                );
            }
            None if field.required => {
                return Err(CodecError::schema(
                    &format!("{path}.{}", field.wire_name),
                    "required field",
                    "missing",
                ));
            }
            None => {}
        }
    }
    Ok(WireValue::Object(wire))
}

fn raw_kind(value: &RawValue) -> &'static str {
    match value {
        RawValue::Null => "null",
        RawValue::Bool(_) => "bool",
        RawValue::I64(_) => "signed integer",
        RawValue::U64(_) => "unsigned integer",
        RawValue::F32(_) => "f32",
        RawValue::F64(_) => "f64",
        RawValue::Char(_) => "char",
        RawValue::String(_) => "string",
        RawValue::Sequence(_) => "sequence",
        RawValue::Object(_) => "object",
        RawValue::Bytes(_) => "bytes",
        RawValue::Buffer(_) => "numeric buffer",
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

impl<'de> de::Deserializer<'de> for RawValue {
    type Error = CodecError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Null => visitor.visit_unit(),
            Self::Bool(value) => visitor.visit_bool(value),
            Self::I64(value) => visitor.visit_i64(value),
            Self::U64(value) => visitor.visit_u64(value),
            Self::F32(value) => visitor.visit_f32(value),
            Self::F64(value) => visitor.visit_f64(value),
            Self::Char(value) => visitor.visit_char(value),
            Self::String(value) => visitor.visit_string(value),
            Self::Sequence(values) => visitor.visit_seq(RawSeqAccess::new(values)),
            Self::Object(values) => visitor.visit_map(RawMapAccess::new(values)),
            Self::Bytes(value) => visitor.visit_byte_buf(value),
            Self::Buffer(value) => visitor.visit_seq(RawSeqAccess::new(buffer_values(value))),
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Bool(value) => visitor.visit_bool(value),
            value => Err(CodecError::schema("$", "bool", raw_kind(&value))),
        }
    }

    fn deserialize_i8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_i8(integer_cast(self, "i8")?)
    }

    fn deserialize_i16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_i16(integer_cast(self, "i16")?)
    }

    fn deserialize_i32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_i32(integer_cast(self, "i32")?)
    }

    fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_i64(integer_cast(self, "i64")?)
    }

    fn deserialize_i128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::I64(value) => visitor.visit_i128(i128::from(value)),
            Self::U64(value) => visitor.visit_i128(i128::from(value)),
            value => Err(CodecError::schema("$", "i128", raw_kind(&value))),
        }
    }

    fn deserialize_u8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_u8(integer_cast(self, "u8")?)
    }

    fn deserialize_u16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_u16(integer_cast(self, "u16")?)
    }

    fn deserialize_u32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_u32(integer_cast(self, "u32")?)
    }

    fn deserialize_u64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_u64(integer_cast(self, "u64")?)
    }

    fn deserialize_u128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::U64(value) => visitor.visit_u128(u128::from(value)),
            Self::I64(value) => u128::try_from(value)
                .map_err(|_| CodecError::schema("$", "u128", "negative integer"))
                .and_then(|value| visitor.visit_u128(value)),
            value => Err(CodecError::schema("$", "u128", raw_kind(&value))),
        }
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::F32(value) => visitor.visit_f32(value),
            Self::F64(value) if !value.is_finite() || value.abs() <= f32::MAX as f64 => {
                visitor.visit_f32(value as f32)
            }
            Self::F64(_) => Err(CodecError::schema("$", "f32", "out-of-range f64")),
            value => Err(CodecError::schema("$", "f32", raw_kind(&value))),
        }
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::F32(value) => visitor.visit_f64(f64::from(value)),
            Self::F64(value) => visitor.visit_f64(value),
            value => Err(CodecError::schema("$", "f64", raw_kind(&value))),
        }
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Char(value) => visitor.visit_char(value),
            Self::String(value) => {
                let mut chars = value.chars();
                let character = chars
                    .next()
                    .ok_or_else(|| CodecError::schema("$", "one character", "empty string"))?;
                if chars.next().is_some() {
                    return Err(CodecError::schema(
                        "$",
                        "one character",
                        "multi-character string",
                    ));
                }
                visitor.visit_char(character)
            }
            value => Err(CodecError::schema("$", "char", raw_kind(&value))),
        }
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_string(visitor)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::String(value) => visitor.visit_string(value),
            Self::Char(value) => visitor.visit_string(value.to_string()),
            value => Err(CodecError::schema("$", "string", raw_kind(&value))),
        }
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Bytes(value) => visitor.visit_byte_buf(value),
            Self::Buffer(BufferValue::U8(value)) => visitor.visit_byte_buf(value),
            value => Err(CodecError::schema("$", "bytes", raw_kind(&value))),
        }
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Null => visitor.visit_none(),
            value => visitor.visit_some(value),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Null => visitor.visit_unit(),
            value => Err(CodecError::schema("$", "unit/null", raw_kind(&value))),
        }
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Sequence(values) => visitor.visit_seq(RawSeqAccess::new(values)),
            Self::Buffer(value) => visitor.visit_seq(RawSeqAccess::new(buffer_values(value))),
            Self::Bytes(value) => visitor.visit_seq(RawSeqAccess::new(
                value
                    .into_iter()
                    .map(|value| RawValue::U64(value.into()))
                    .collect(),
            )),
            value => Err(CodecError::schema("$", "sequence", raw_kind(&value))),
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        length: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match &self {
            Self::Sequence(values) if values.len() == length => self.deserialize_seq(visitor),
            Self::Sequence(values) => Err(CodecError::schema(
                "$",
                &format!("{length}-item tuple"),
                &format!("{}-item sequence", values.len()),
            )),
            value => Err(CodecError::schema("$", "tuple", raw_kind(value))),
        }
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        length: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_tuple(length, visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Object(values) => visitor.visit_map(RawMapAccess::new(values)),
            value => Err(CodecError::schema("$", "object/map", raw_kind(&value))),
        }
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self {
            Self::String(variant) => visitor.visit_enum(RawEnumAccess {
                variant,
                value: None,
            }),
            Self::Object(values) if values.len() == 1 => {
                let (variant, value) = values
                    .into_iter()
                    .next()
                    .expect("a one-item map has one item");
                visitor.visit_enum(RawEnumAccess {
                    variant,
                    value: Some(value),
                })
            }
            value => Err(CodecError::schema(
                "$",
                "enum string or single-key object",
                raw_kind(&value),
            )),
        }
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_string(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }

    fn is_human_readable(&self) -> bool {
        true
    }
}

fn integer_cast<T>(value: RawValue, expected: &str) -> Result<T, CodecError>
where
    T: TryFrom<i64> + TryFrom<u64>,
{
    match value {
        RawValue::I64(value) => T::try_from(value)
            .map_err(|_| CodecError::schema("$", expected, "out-of-range signed integer")),
        RawValue::U64(value) => T::try_from(value)
            .map_err(|_| CodecError::schema("$", expected, "out-of-range unsigned integer")),
        value => Err(CodecError::schema("$", expected, raw_kind(&value))),
    }
}

fn buffer_values(value: BufferValue) -> Vec<RawValue> {
    macro_rules! values {
        ($values:expr, $variant:ident) => {
            $values.into_iter().map(RawValue::$variant).collect()
        };
    }

    match value {
        BufferValue::U8(values) => values
            .into_iter()
            .map(|value| RawValue::U64(value.into()))
            .collect(),
        BufferValue::I8(values) => values
            .into_iter()
            .map(|value| RawValue::I64(value.into()))
            .collect(),
        BufferValue::U16(values) => values
            .into_iter()
            .map(|value| RawValue::U64(value.into()))
            .collect(),
        BufferValue::I16(values) => values
            .into_iter()
            .map(|value| RawValue::I64(value.into()))
            .collect(),
        BufferValue::U32(values) => values
            .into_iter()
            .map(|value| RawValue::U64(value.into()))
            .collect(),
        BufferValue::I32(values) => values
            .into_iter()
            .map(|value| RawValue::I64(value.into()))
            .collect(),
        BufferValue::U64(values) => values!(values, U64),
        BufferValue::I64(values) => values!(values, I64),
        BufferValue::F32(values) => values!(values, F32),
        BufferValue::F64(values) => values!(values, F64),
    }
}

struct RawSeqAccess {
    values: std::vec::IntoIter<RawValue>,
}

impl RawSeqAccess {
    fn new(values: Vec<RawValue>) -> Self {
        Self {
            values: values.into_iter(),
        }
    }
}

impl<'de> SeqAccess<'de> for RawSeqAccess {
    type Error = CodecError;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Self::Error> {
        self.values
            .next()
            .map(|value| seed.deserialize(value))
            .transpose()
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.values.len())
    }
}

struct RawMapAccess {
    values: std::collections::btree_map::IntoIter<String, RawValue>,
    next_value: Option<RawValue>,
}

impl RawMapAccess {
    fn new(values: BTreeMap<String, RawValue>) -> Self {
        Self {
            values: values.into_iter(),
            next_value: None,
        }
    }
}

impl<'de> MapAccess<'de> for RawMapAccess {
    type Error = CodecError;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Self::Error> {
        let Some((key, value)) = self.values.next() else {
            return Ok(None);
        };
        self.next_value = Some(value);
        seed.deserialize(RawValue::String(key)).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, Self::Error> {
        let value = self
            .next_value
            .take()
            .ok_or_else(|| CodecError::new("map requested a value before its key"))?;
        seed.deserialize(value)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.values.len())
    }
}

struct RawEnumAccess {
    variant: String,
    value: Option<RawValue>,
}

impl<'de> EnumAccess<'de> for RawEnumAccess {
    type Error = CodecError;
    type Variant = RawVariantAccess;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), Self::Error> {
        let variant = seed.deserialize(self.variant.into_deserializer())?;
        Ok((variant, RawVariantAccess { value: self.value }))
    }
}

struct RawVariantAccess {
    value: Option<RawValue>,
}

impl<'de> VariantAccess<'de> for RawVariantAccess {
    type Error = CodecError;

    fn unit_variant(self) -> Result<(), Self::Error> {
        match self.value {
            None | Some(RawValue::Null) => Ok(()),
            Some(value) => Err(CodecError::schema("$", "unit variant", raw_kind(&value))),
        }
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(
        self,
        seed: T,
    ) -> Result<T::Value, Self::Error> {
        seed.deserialize(
            self.value
                .ok_or_else(|| CodecError::new("newtype enum variant is missing its value"))?,
        )
    }

    fn tuple_variant<V: Visitor<'de>>(
        self,
        length: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        de::Deserializer::deserialize_tuple(
            self.value
                .ok_or_else(|| CodecError::new("tuple enum variant is missing its value"))?,
            length,
            visitor,
        )
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        de::Deserializer::deserialize_struct(
            self.value
                .ok_or_else(|| CodecError::new("struct enum variant is missing its value"))?,
            "variant",
            fields,
            visitor,
        )
    }
}
