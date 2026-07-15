//! The [`Bridged`] trait — membership in the portable type system — and
//! the [`Buf`] zero-JSON-copy numeric buffer.
//!
//! A Rust type can cross the bridge if and only if it implements
//! [`Bridged`]. Domain code keeps ordinary Serde representations: `i64` and
//! `u64` are JSON numbers in Rust and become canonical decimal strings only
//! at the FFI boundary; `serde_json::Value` stays transparent JSON.

use crate::ir::{Dtype, Ty};
use serde::de::{MapAccess, SeqAccess, Visitor};
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

    /// IR shape used only while assembling inventory registrations.
    ///
    /// Named macro-generated types override this to retain their defining
    /// crate identity until registry reachability has been resolved. Public
    /// callers should use [`Bridged::ty`], which always returns the stable
    /// manifest shape.
    #[doc(hidden)]
    fn inventory_ty() -> Ty {
        Self::ty()
    }

    /// IR shape when this type is the top-level function return.
    ///
    /// Data `()` is JSON null, while a top-level `()` return projects to
    /// host-language void. All other types use their ordinary inventory
    /// shape, including aliases of `()` through Rust's trait resolution.
    #[doc(hidden)]
    fn inventory_return_ty() -> Ty {
        Self::inventory_ty()
    }
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
    i64 => I64,
    u64 => U64,
    f32 => F32,
    f64 => F64,
    String => String,
}

impl Bridged for () {
    fn ty() -> Ty {
        Ty::Null
    }

    fn inventory_return_ty() -> Ty {
        Ty::Unit
    }
}

impl Bridged for serde_json::Value {
    fn ty() -> Ty {
        Ty::Json
    }
}

impl<T: Bridged> Bridged for Option<T> {
    fn ty() -> Ty {
        Ty::Option {
            inner: Box::new(T::ty()),
        }
    }

    fn inventory_ty() -> Ty {
        Ty::Option {
            inner: Box::new(T::inventory_ty()),
        }
    }
}

impl<T: Bridged> Bridged for Vec<T> {
    fn ty() -> Ty {
        Ty::List {
            inner: Box::new(T::ty()),
        }
    }

    fn inventory_ty() -> Ty {
        Ty::List {
            inner: Box::new(T::inventory_ty()),
        }
    }
}

impl<T: Bridged> Bridged for HashMap<String, T> {
    fn ty() -> Ty {
        Ty::Map {
            value: Box::new(T::ty()),
        }
    }

    fn inventory_ty() -> Ty {
        Ty::Map {
            value: Box::new(T::inventory_ty()),
        }
    }
}

impl<T: Bridged> Bridged for BTreeMap<String, T> {
    fn ty() -> Ty {
        Ty::Map {
            value: Box::new(T::ty()),
        }
    }

    fn inventory_ty() -> Ty {
        Ty::Map {
            value: Box::new(T::inventory_ty()),
        }
    }
}

macro_rules! impl_bridged_tuple {
    ($(($($ty:ident),+)),+ $(,)?) => {
        $(
            impl<$($ty: Bridged),+> Bridged for ($($ty,)+) {
                fn ty() -> Ty {
                    Ty::Tuple {
                        items: vec![$($ty::ty()),+],
                    }
                }

                fn inventory_ty() -> Ty {
                    Ty::Tuple {
                        items: vec![$($ty::inventory_ty()),+],
                    }
                }
            }
        )+
    };
}

impl_bridged_tuple! {
    (A, B),
    (A, B, C),
    (A, B, C, D),
    (A, B, C, D, E),
    (A, B, C, D, E, F),
    (A, B, C, D, E, F, G),
    (A, B, C, D, E, F, G, H),
    (A, B, C, D, E, F, G, H, I),
    (A, B, C, D, E, F, G, H, I, J),
    (A, B, C, D, E, F, G, H, I, J, K),
    (A, B, C, D, E, F, G, H, I, J, K, L),
}

/// Element types allowed in `&[T]` slice parameters (ABI §6).
pub trait SliceElem: Copy + 'static {
    const DTYPE: Dtype;
}

/// Element types allowed in [`Buf<T>`] attachments (ABI §6).
pub trait BufElem: Copy + Serialize + for<'de> Deserialize<'de> + 'static {
    const DTYPE: Dtype;
    fn to_le_bytes_vec(items: &[Self]) -> Vec<u8>;
    fn from_le_bytes_vec(bytes: &[u8]) -> Option<Vec<Self>>;
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
                fn from_le_bytes_vec(bytes: &[u8]) -> Option<Vec<Self>> {
                    let width = std::mem::size_of::<Self>();
                    if bytes.len() % width != 0 {
                        return None;
                    }
                    Some(
                        bytes
                            .chunks_exact(width)
                            .map(|chunk| Self::from_le_bytes(chunk.try_into().unwrap()))
                            .collect(),
                    )
                }
            }
        )*
    };
}

impl_elem! {
    u8 => U8,
    i8 => I8,
    u16 => U16,
    i16 => I16,
    u32 => U32,
    i32 => I32,
    u64 => U64,
    i64 => I64,
    f32 => F32,
    f64 => F64,
}

/// An owned numeric vector that crosses through the
/// envelope's raw tail instead of JSON — O(1) JSON size regardless of
/// element count.
///
/// Valid in parameters and returns, including nested positions. Use a
/// top-level `&[T]` parameter when borrowing without a Rust-side copy is
/// more important than ownership or nesting.
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

/// Owned binary data, distinct from the numeric `Buf<u8>` contract.
///
/// In an envelope it uses a `dt: "bytes"` attachment. Outside an envelope
/// Serde scope it degrades to a JSON array of byte values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bytes(pub Vec<u8>);

impl Bytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl From<&[u8]> for Bytes {
    fn from(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Bridged for Bytes {
    fn ty() -> Ty {
        Ty::Bytes
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

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match crate::envelope::tail_push(&self.0, 1) {
            Some(off) => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry(
                    BUF_PLACEHOLDER_KEY,
                    &BufPlaceholderBody {
                        off,
                        len: self.0.len(),
                        dt: "bytes",
                    },
                )?;
                map.end()
            }
            None => {
                let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
                for byte in &self.0 {
                    seq.serialize_element(byte)?;
                }
                seq.end()
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedPlaceholderBody {
    off: usize,
    len: usize,
    dt: String,
}

struct AttachmentVisitor<T: BufElem> {
    dtype: &'static str,
    description: &'static str,
    marker: std::marker::PhantomData<T>,
}

impl<'de, T: BufElem> Visitor<'de> for AttachmentVisitor<T> {
    type Value = Vec<T>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(self.description)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(item) = seq.next_element::<T>()? {
            items.push(item);
        }
        Ok(items)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let key = map
            .next_key::<String>()?
            .ok_or_else(|| serde::de::Error::custom("empty buffer placeholder object"))?;
        if key != BUF_PLACEHOLDER_KEY {
            return Err(serde::de::Error::custom(format!(
                "expected {BUF_PLACEHOLDER_KEY:?}, found {key:?}"
            )));
        }
        let body = map.next_value::<OwnedPlaceholderBody>()?;
        if map.next_key::<serde::de::IgnoredAny>()?.is_some() {
            return Err(serde::de::Error::custom(
                "buffer placeholder object must contain no sibling fields",
            ));
        }
        if body.dt != self.dtype {
            return Err(serde::de::Error::custom(format!(
                "buffer dtype mismatch: expected {:?}, found {:?}",
                self.dtype, body.dt
            )));
        }
        crate::envelope::request_tail_read::<T>(body.off, body.len)
            .map_err(serde::de::Error::custom)
    }
}

fn deserialize_attachment<'de, D, T>(
    deserializer: D,
    dtype: &'static str,
    description: &'static str,
) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: BufElem,
{
    deserializer.deserialize_any(AttachmentVisitor {
        dtype,
        description,
        marker: std::marker::PhantomData,
    })
}

impl<'de, T: BufElem> Deserialize<'de> for Buf<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_attachment(
            deserializer,
            T::DTYPE.wire_name(),
            "a numeric array or buffer placeholder",
        )
        .map(Buf)
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_attachment(deserializer, "bytes", "a byte array or bytes placeholder")
            .map(Bytes)
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
    fn json_is_transparent_and_preserves_attachment_shaped_content() {
        let inner = serde_json::json!({
            BUF_PLACEHOLDER_KEY: {"off": 0, "len": 1, "dt": "u8"}
        });
        let encoded = serde_json::to_value(&inner).unwrap();
        assert_eq!(encoded, inner);
        assert_eq!(
            serde_json::from_value::<serde_json::Value>(encoded).unwrap(),
            inner
        );

        let ptr = envelope::encode_ok(&inner);
        let (status, json, tail) = envelope::decode_owned(ptr);
        assert_eq!(status, envelope::STATUS_OK);
        assert!(tail.is_empty());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&json).unwrap(),
            inner
        );

        let request_tail = [99_u8];
        let decoded = envelope::with_request_tail(&request_tail, || {
            serde_json::from_value::<serde_json::Value>(inner.clone()).unwrap()
        });
        assert_eq!(decoded, inner);
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
    fn buf_deserializes_owned_little_endian_request_attachments() {
        let tail = [0x34, 0x12, 0xfe, 0xff];
        let json = format!(r#"{{"{BUF_PLACEHOLDER_KEY}":{{"off":0,"len":2,"dt":"i16"}}}}"#);
        let buf =
            envelope::with_request_tail(&tail, || serde_json::from_str::<Buf<i16>>(&json).unwrap());
        assert_eq!(buf.into_inner(), vec![0x1234, -2]);

        let mismatch = envelope::with_request_tail(&tail, || {
            serde_json::from_str::<Buf<u16>>(&json).unwrap_err()
        });
        assert!(mismatch.to_string().contains("dtype mismatch"));
    }

    #[test]
    fn bytes_are_distinct_attachments_and_fall_back_to_json_arrays() {
        let bytes = Bytes::from(&[0x00, 0x7f, 0xff][..]);
        assert_eq!(bytes.as_slice(), &[0x00, 0x7f, 0xff]);
        assert_eq!(bytes.len(), 3);
        assert!(!bytes.is_empty());
        assert_eq!(serde_json::to_string(&bytes).unwrap(), "[0,127,255]");
        assert_eq!(Bytes::ty(), Ty::Bytes);

        let ptr = envelope::encode_ok(&bytes);
        let (status, json, tail) = envelope::decode_owned(ptr);
        assert_eq!(status, envelope::STATUS_OK);
        assert_eq!(tail, [0x00, 0x7f, 0xff]);
        let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(value[BUF_PLACEHOLDER_KEY]["dt"], "bytes");

        let decoded =
            envelope::with_request_tail(&tail, || serde_json::from_slice::<Bytes>(&json).unwrap());
        assert_eq!(decoded.into_inner(), vec![0x00, 0x7f, 0xff]);
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
        assert_eq!(i64::ty(), Ty::I64);
        assert_eq!(u64::ty(), Ty::U64);
        assert_eq!(f32::ty(), Ty::F32);
        assert_eq!(f64::ty(), Ty::F64);
        assert_eq!(String::ty(), Ty::String);
        assert_eq!(<()>::ty(), Ty::Null);
        assert_eq!(<()>::inventory_return_ty(), Ty::Unit);
        assert_eq!(serde_json::Value::ty(), Ty::Json);
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
        assert_eq!(Buf::<i8>::ty(), Ty::Buf { dt: Dtype::I8 });
        assert_eq!(Buf::<u16>::ty(), Ty::Buf { dt: Dtype::U16 });
        assert_eq!(Buf::<i16>::ty(), Ty::Buf { dt: Dtype::I16 });
        assert_eq!(Buf::<u32>::ty(), Ty::Buf { dt: Dtype::U32 });
        assert_eq!(Buf::<i32>::ty(), Ty::Buf { dt: Dtype::I32 });
        assert_eq!(Buf::<u64>::ty(), Ty::Buf { dt: Dtype::U64 });
        assert_eq!(Buf::<i64>::ty(), Ty::Buf { dt: Dtype::I64 });
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
        assert_eq!(
            <(u8, String, Option<i64>)>::ty(),
            Ty::Tuple {
                items: vec![
                    Ty::U8,
                    Ty::String,
                    Ty::Option {
                        inner: Box::new(Ty::I64),
                    },
                ],
            }
        );
    }

    #[test]
    fn tuple_arity_twelve_has_a_fixed_ir_shape() {
        type Twelve = (u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8);
        assert_eq!(
            Twelve::ty(),
            Ty::Tuple {
                items: vec![Ty::U8; 12]
            }
        );
    }

    #[test]
    fn every_dtype_serializes_byte_exact_in_envelope() {
        #[derive(Serialize)]
        struct Out {
            u8s: Buf<u8>,
            i8s: Buf<i8>,
            u16s: Buf<u16>,
            i16s: Buf<i16>,
            u32s: Buf<u32>,
            i32s: Buf<i32>,
            u64s: Buf<u64>,
            i64s: Buf<i64>,
            f32s: Buf<f32>,
            f64s: Buf<f64>,
        }
        let ptr = envelope::encode_ok(&Out {
            u8s: Buf::new(vec![0, u8::MAX]),
            i8s: Buf::new(vec![i8::MIN, i8::MAX]),
            u16s: Buf::new(vec![0, u16::MAX]),
            i16s: Buf::new(vec![i16::MIN, i16::MAX]),
            u32s: Buf::new(vec![0, u32::MAX]),
            i32s: Buf::new(vec![i32::MIN, i32::MAX]),
            u64s: Buf::new(vec![0, u64::MAX]),
            i64s: Buf::new(vec![i64::MIN, i64::MAX]),
            f32s: Buf::new(vec![f32::MIN, -0.0f32, f32::NAN]),
            f64s: Buf::new(vec![f64::NEG_INFINITY, f64::MIN_POSITIVE]),
        });
        let (status, json, tail) = envelope::decode_owned(ptr);
        assert_eq!(status, envelope::STATUS_OK);

        fn le<T: BufElem>(values: &[T]) -> Vec<u8> {
            T::to_le_bytes_vec(values)
        }

        let v: serde_json::Value = serde_json::from_slice(&json).unwrap();
        let mut cursor = 0usize;
        for (field, dt, align, expected_bytes) in [
            ("u8s", "u8", 1, le(&[0u8, u8::MAX])),
            ("i8s", "i8", 1, le(&[i8::MIN, i8::MAX])),
            ("u16s", "u16", 2, le(&[0u16, u16::MAX])),
            ("i16s", "i16", 2, le(&[i16::MIN, i16::MAX])),
            ("u32s", "u32", 4, le(&[0u32, u32::MAX])),
            ("i32s", "i32", 4, le(&[i32::MIN, i32::MAX])),
            ("u64s", "u64", 8, le(&[0u64, u64::MAX])),
            ("i64s", "i64", 8, le(&[i64::MIN, i64::MAX])),
            ("f32s", "f32", 4, le(&[f32::MIN, -0.0, f32::NAN])),
            (
                "f64s",
                "f64",
                8,
                le(&[f64::NEG_INFINITY, f64::MIN_POSITIVE]),
            ),
        ] {
            cursor += (align - (cursor % align)) % align;
            assert_eq!(v[field][BUF_PLACEHOLDER_KEY]["dt"], dt);
            assert_eq!(v[field][BUF_PLACEHOLDER_KEY]["off"], cursor);
            assert_eq!(
                v[field][BUF_PLACEHOLDER_KEY]["len"],
                expected_bytes.len() / align
            );
            assert_eq!(&tail[cursor..cursor + expected_bytes.len()], expected_bytes);
            cursor += expected_bytes.len();
        }
        assert_eq!(cursor, tail.len());
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
        assert!(err.to_string().contains("outside a request-tail scope"));
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
