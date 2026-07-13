//! The [`Bridged`] trait — membership in the portable type system — and
//! the [`Buf`] zero-JSON-copy numeric buffer.
//!
//! A Rust type can cross the bridge if and only if it implements
//! [`Bridged`]. Deliberately **not** implemented (see
//! `docs/design/type-system.md` §1): `u64`, `i64`, `u128`, `i128`,
//! `usize`, `isize`, `char`, tuples. If you hit a "trait bound not
//! satisfied" error on one of these, that is the type system saying no —
//! use `u32`/`i32` or `String`.

use crate::ir::{Dtype, Ty};
use serde::de::{SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, HashMap};

/// A type that participates in the portable type system.
///
/// `ty()` returns the IR shape used for the manifest. Structs and enums
/// annotated with `#[bridge]` get a derived impl returning
/// [`Ty::Ref`]; everything else is structural.
pub trait Bridged {
    fn ty() -> Ty;
}

macro_rules! impl_bridged_scalar {
    ($($rust:ty => $variant:ident),* $(,)?) => {
        $(impl Bridged for $rust {
            fn ty() -> Ty { Ty::$variant }
        })*
    };
}

impl_bridged_scalar! {
    bool => Bool,
    u8 => U8,
    u16 => U16,
    u32 => U32,
    i8 => I8,
    i16 => I16,
    i32 => I32,
    f32 => F32,
    f64 => F64,
    String => String,
    () => Unit,
}

impl<T: Bridged> Bridged for Option<T> {
    fn ty() -> Ty {
        Ty::Option {
            inner: Box::new(T::ty()),
        }
    }
}

impl<T: Bridged> Bridged for Vec<T> {
    fn ty() -> Ty {
        Ty::List {
            inner: Box::new(T::ty()),
        }
    }
}

impl<T: Bridged> Bridged for HashMap<String, T> {
    fn ty() -> Ty {
        Ty::Map {
            value: Box::new(T::ty()),
        }
    }
}

impl<T: Bridged> Bridged for BTreeMap<String, T> {
    fn ty() -> Ty {
        Ty::Map {
            value: Box::new(T::ty()),
        }
    }
}

/// Element types allowed in `&[T]` slice parameters (ABI §6).
pub trait SliceElem: Copy + 'static {
    const DTYPE: Dtype;
}

/// Element types allowed in [`Buf<T>`] returns (ABI §6).
pub trait BufElem: Copy + Serialize + for<'de> Deserialize<'de> + 'static {
    const DTYPE: Dtype;
    fn to_le_bytes_vec(items: &[Self]) -> Vec<u8>;
}

macro_rules! impl_elem {
    ($($rust:ty => $dt:ident),* $(,)?) => {
        $(
            impl SliceElem for $rust {
                const DTYPE: Dtype = Dtype::$dt;
            }
            impl BufElem for $rust {
                const DTYPE: Dtype = Dtype::$dt;
                fn to_le_bytes_vec(items: &[Self]) -> Vec<u8> {
                    let mut out = Vec::with_capacity(items.len() * std::mem::size_of::<Self>());
                    for item in items {
                        out.extend_from_slice(&item.to_le_bytes());
                    }
                    out
                }
            }
        )*
    };
}

impl_elem! {
    u8 => U8,
    i16 => I16,
    i32 => I32,
    f32 => F32,
    f64 => F64,
}

/// An owned numeric vector that returns to the caller through the
/// envelope's raw tail instead of JSON — O(1) JSON size regardless of
/// element count.
///
/// Valid in return position only (including nested inside returned
/// structs); the type system rejects it in parameters — accept `&[T]`
/// there instead.
///
/// Outside an envelope serialization scope (e.g. if you `serde_json` a
/// value containing a `Buf` yourself), it degrades gracefully to a plain
/// JSON array, and it always deserializes from one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Buf<T: BufElem>(pub Vec<T>);

impl<T: BufElem> Buf<T> {
    pub fn new(items: Vec<T>) -> Self {
        Self(items)
    }
    pub fn into_inner(self) -> Vec<T> {
        self.0
    }
}

impl<T: BufElem> From<Vec<T>> for Buf<T> {
    fn from(items: Vec<T>) -> Self {
        Self(items)
    }
}

impl<T: BufElem> Bridged for Buf<T> {
    fn ty() -> Ty {
        Ty::Buf { dt: T::DTYPE }
    }
}

/// Schemaless JSON passthrough — the deliberate escape hatch for payloads
/// whose shape is decided at runtime (plugin parameters, free-form
/// attributes). Projects as `Any` in Python, `unknown` in TypeScript, and
/// `{}` in JSON Schema. Prefer real bridged types wherever the shape is
/// actually known.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Json(pub serde_json::Value);

impl Json {
    pub fn new(value: serde_json::Value) -> Self {
        Self(value)
    }
    pub fn into_inner(self) -> serde_json::Value {
        self.0
    }
}

impl From<serde_json::Value> for Json {
    fn from(value: serde_json::Value) -> Self {
        Self(value)
    }
}

impl Bridged for Json {
    fn ty() -> Ty {
        Ty::Json
    }
}

/// The JSON key marking a tail-buffer placeholder. Also referenced by the
/// Python and TypeScript runtimes; never change it without an ABI bump.
pub const BUF_PLACEHOLDER_KEY: &str = "__rspyts_buf__";

#[derive(Serialize)]
struct BufPlaceholderBody {
    off: usize,
    len: usize,
    dt: &'static str,
}

impl<T: BufElem> Serialize for Buf<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let bytes = T::to_le_bytes_vec(&self.0);
        match crate::envelope::tail_push(&bytes, std::mem::align_of::<T>()) {
            Some(off) => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry(
                    BUF_PLACEHOLDER_KEY,
                    &BufPlaceholderBody {
                        off,
                        len: self.0.len(),
                        dt: T::DTYPE.wire_name(),
                    },
                )?;
                map.end()
            }
            None => {
                // No envelope in progress: degrade to a plain array.
                let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
                for item in &self.0 {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
        }
    }
}

impl<'de, T: BufElem> Deserialize<'de> for Buf<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SeqVisitor<T>(std::marker::PhantomData<T>);
        impl<'de, T: BufElem> Visitor<'de> for SeqVisitor<T> {
            type Value = Buf<T>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a numeric array")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(item) = seq.next_element::<T>()? {
                    items.push(item);
                }
                Ok(Buf(items))
            }
        }
        deserializer.deserialize_seq(SeqVisitor(std::marker::PhantomData))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope;

    #[test]
    fn buf_outside_envelope_is_plain_array() {
        let json = serde_json::to_string(&Buf::new(vec![1.0f64, 2.0])).unwrap();
        assert_eq!(json, "[1.0,2.0]");
    }

    #[test]
    fn buf_inside_envelope_is_placeholder_with_tail() {
        #[derive(Serialize)]
        struct Out {
            values: Buf<f32>,
        }
        let ptr = envelope::encode_ok(&Out {
            values: Buf::new(vec![1.5f32, -2.0]),
        });
        let decoded = unsafe { envelope::decode(ptr) };
        let v: serde_json::Value = serde_json::from_slice(decoded.json).unwrap();
        assert_eq!(v["values"][BUF_PLACEHOLDER_KEY]["len"], 2);
        assert_eq!(v["values"][BUF_PLACEHOLDER_KEY]["dt"], "f32");
        assert_eq!(decoded.tail.len(), 8);
        assert_eq!(&decoded.tail[0..4], &1.5f32.to_le_bytes());
        unsafe { envelope::dealloc(ptr, envelope::total_len(ptr)) };
    }

    #[test]
    fn buf_round_trips_from_plain_array() {
        let buf: Buf<i32> = serde_json::from_str("[1,2,3]").unwrap();
        assert_eq!(buf.0, vec![1, 2, 3]);
    }

    #[test]
    fn scalar_ty_shapes() {
        assert_eq!(bool::ty(), Ty::Bool);
        assert_eq!(u8::ty(), Ty::U8);
        assert_eq!(u16::ty(), Ty::U16);
        assert_eq!(u32::ty(), Ty::U32);
        assert_eq!(i8::ty(), Ty::I8);
        assert_eq!(i16::ty(), Ty::I16);
        assert_eq!(i32::ty(), Ty::I32);
        assert_eq!(f32::ty(), Ty::F32);
        assert_eq!(f64::ty(), Ty::F64);
        assert_eq!(String::ty(), Ty::String);
        assert_eq!(<()>::ty(), Ty::Unit);
    }

    #[test]
    fn composite_ty_shapes() {
        assert_eq!(
            Option::<u8>::ty(),
            Ty::Option {
                inner: Box::new(Ty::U8),
            },
        );
        assert_eq!(
            Vec::<String>::ty(),
            Ty::List {
                inner: Box::new(Ty::String),
            },
        );
        assert_eq!(
            HashMap::<String, i32>::ty(),
            Ty::Map {
                value: Box::new(Ty::I32),
            },
        );
        assert_eq!(
            BTreeMap::<String, bool>::ty(),
            Ty::Map {
                value: Box::new(Ty::Bool),
            },
        );
        assert_eq!(Buf::<u8>::ty(), Ty::Buf { dt: Dtype::U8 });
        assert_eq!(Buf::<i16>::ty(), Ty::Buf { dt: Dtype::I16 });
        assert_eq!(Buf::<i32>::ty(), Ty::Buf { dt: Dtype::I32 });
        assert_eq!(Buf::<f32>::ty(), Ty::Buf { dt: Dtype::F32 });
        assert_eq!(Buf::<f64>::ty(), Ty::Buf { dt: Dtype::F64 });
    }

    #[test]
    fn deeply_nested_composite_ty_shape() {
        assert_eq!(
            HashMap::<String, Vec<Option<Buf<f32>>>>::ty(),
            Ty::Map {
                value: Box::new(Ty::List {
                    inner: Box::new(Ty::Option {
                        inner: Box::new(Ty::Buf { dt: Dtype::F32 }),
                    }),
                }),
            },
        );
    }

    #[test]
    fn every_dtype_serializes_byte_exact_in_envelope() {
        #[derive(Serialize)]
        struct Out {
            bytes: Buf<u8>,
            shorts: Buf<i16>,
            ints: Buf<i32>,
            floats: Buf<f32>,
            doubles: Buf<f64>,
        }
        let ptr = envelope::encode_ok(&Out {
            bytes: Buf::new(vec![0u8, 255]),
            shorts: Buf::new(vec![i16::MIN, -1, i16::MAX]),
            ints: Buf::new(vec![i32::MIN, i32::MAX]),
            floats: Buf::new(vec![f32::MIN, -0.0f32, f32::NAN]),
            doubles: Buf::new(vec![f64::NEG_INFINITY, f64::MIN_POSITIVE]),
        });
        let (status, json, tail) = envelope::decode_owned(ptr);
        assert_eq!(status, envelope::STATUS_OK);

        // Offsets: u8 x2 at 0, i16 x3 at 2, i32 x2 at 8, f32 x3 at 16,
        // then pad 4 so the f64 x2 land 8-aligned at 32.
        let mut expected = vec![0u8, 255];
        for v in [i16::MIN, -1, i16::MAX] {
            expected.extend_from_slice(&v.to_le_bytes());
        }
        for v in [i32::MIN, i32::MAX] {
            expected.extend_from_slice(&v.to_le_bytes());
        }
        for v in [f32::MIN, -0.0f32, f32::NAN] {
            expected.extend_from_slice(&v.to_le_bytes());
        }
        expected.extend_from_slice(&[0u8; 4]);
        for v in [f64::NEG_INFINITY, f64::MIN_POSITIVE] {
            expected.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(tail, expected);

        let v: serde_json::Value = serde_json::from_slice(&json).unwrap();
        for (field, dt, off, len) in [
            ("bytes", "u8", 0, 2),
            ("shorts", "i16", 2, 3),
            ("ints", "i32", 8, 2),
            ("floats", "f32", 16, 3),
            ("doubles", "f64", 32, 2),
        ] {
            assert_eq!(v[field][BUF_PLACEHOLDER_KEY]["dt"], dt);
            assert_eq!(v[field][BUF_PLACEHOLDER_KEY]["off"], off);
            assert_eq!(v[field][BUF_PLACEHOLDER_KEY]["len"], len);
        }
    }

    #[test]
    fn empty_buf_emits_placeholder_and_no_tail_bytes() {
        #[derive(Serialize)]
        struct Out {
            values: Buf<f64>,
        }
        let ptr = envelope::encode_ok(&Out {
            values: Buf::new(vec![]),
        });
        let (status, json, tail) = envelope::decode_owned(ptr);
        assert_eq!(status, envelope::STATUS_OK);
        assert!(tail.is_empty());
        let v: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(v["values"][BUF_PLACEHOLDER_KEY]["off"], 0);
        assert_eq!(v["values"][BUF_PLACEHOLDER_KEY]["len"], 0);
        assert_eq!(v["values"][BUF_PLACEHOLDER_KEY]["dt"], "f64");
    }

    #[test]
    fn empty_buf_after_data_still_pads_for_alignment() {
        #[derive(Serialize)]
        struct Out {
            a: Buf<u8>,
            b: Buf<f64>,
        }
        let ptr = envelope::encode_ok(&Out {
            a: Buf::new(vec![7u8]),
            b: Buf::new(vec![]),
        });
        let (status, json, tail) = envelope::decode_owned(ptr);
        assert_eq!(status, envelope::STATUS_OK);
        // The zero-size entry is still 8-aligned, so the tail carries the
        // padding and the placeholder points at its very end.
        assert_eq!(tail, [7, 0, 0, 0, 0, 0, 0, 0]);
        let v: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(v["b"][BUF_PLACEHOLDER_KEY]["off"], 8);
        assert_eq!(v["b"][BUF_PLACEHOLDER_KEY]["len"], 0);
    }

    #[test]
    fn buf_deserialize_rejects_json_objects() {
        let object = format!(r#"{{"{BUF_PLACEHOLDER_KEY}":{{"off":0,"len":1,"dt":"f32"}}}}"#);
        let err = serde_json::from_str::<Buf<f32>>(&object).unwrap_err();
        assert!(err.to_string().contains("a numeric array"));
        assert!(serde_json::from_str::<Buf<i32>>("{}").is_err());
        assert!(serde_json::from_str::<Buf<u8>>(r#""text""#).is_err());
    }

    #[test]
    fn buf_constructors_and_accessors_agree() {
        let buf: Buf<i32> = vec![1, 2].into();
        assert_eq!(buf, Buf::new(vec![1, 2]));
        assert_eq!(buf.into_inner(), vec![1, 2]);
    }
}
