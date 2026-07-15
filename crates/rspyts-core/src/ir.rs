//! The intermediate representation of a bridged crate.
//!
//! A [`Manifest`] is produced inside the compiled module by walking the
//! inventory registry (see [`crate::registry`]) and is retrieved by the CLI
//! through the `rspyts_manifest()` export. Its JSON form is the contract
//! consumed by every emitter; field names are camelCase on the wire.
//!
//! The shapes here mirror `docs/design/abi.md` §7 exactly.

use serde::{Deserialize, Serialize};

/// Complete description of one bridged crate.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    /// ABI version string, e.g. `"3.0"`.
    pub abi: String,
    pub crate_name: String,
    pub crate_version: String,
    /// Data types, sorted by name.
    pub types: Vec<TypeDecl>,
    /// Bridged constants, sorted by name.
    pub constants: Vec<ConstDecl>,
    /// Free functions, sorted by name.
    pub functions: Vec<FnDecl>,
    /// Opaque classes, sorted by name.
    pub classes: Vec<ClassDecl>,
}

/// A named data or error type.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum TypeDecl {
    /// A transparent single-field Rust newtype. Foreign projections keep
    /// the name but reuse the inner wire representation.
    #[serde(rename_all = "camelCase")]
    Newtype {
        name: String,
        docs: String,
        origin: String,
        inner: Ty,
    },
    /// A struct with named fields → object on the wire.
    #[serde(rename_all = "camelCase")]
    Struct {
        name: String,
        docs: String,
        /// Name of the crate that defines this type. Emitters import
        /// instead of re-emitting when it differs from the bridged crate
        /// and an import mapping is configured (codegen.md §9).
        origin: String,
        fields: Vec<FieldDecl>,
    },
    /// A data enum (struct variants only), internally tagged.
    #[serde(rename_all = "camelCase")]
    Enum {
        name: String,
        docs: String,
        /// Name of the crate that defines this type. Emitters import
        /// instead of re-emitting when it differs from the bridged crate
        /// and an import mapping is configured (codegen.md §9).
        origin: String,
        /// The discriminator key, e.g. `"type"`.
        tag: String,
        variants: Vec<VariantDecl>,
    },
    /// An enum whose variants are all fieldless → string on the wire.
    #[serde(rename_all = "camelCase")]
    StringEnum {
        name: String,
        docs: String,
        /// Name of the crate that defines this type. Emitters import
        /// instead of re-emitting when it differs from the bridged crate
        /// and an import mapping is configured (codegen.md §9).
        origin: String,
        variants: Vec<StringVariantDecl>,
    },
    /// An error enum (`#[bridge(error)]`): projects to exception classes,
    /// never to a data shape. Variants may be fieldless or struct-like.
    #[serde(rename_all = "camelCase")]
    ErrorEnum {
        name: String,
        docs: String,
        /// Name of the crate that defines this type. Emitters import
        /// instead of re-emitting when it differs from the bridged crate
        /// and an import mapping is configured (codegen.md §9).
        origin: String,
        variants: Vec<ErrorVariantDecl>,
    },
}

impl TypeDecl {
    pub fn name(&self) -> &str {
        match self {
            TypeDecl::Newtype { name, .. }
            | TypeDecl::Struct { name, .. }
            | TypeDecl::Enum { name, .. }
            | TypeDecl::StringEnum { name, .. }
            | TypeDecl::ErrorEnum { name, .. } => name,
        }
    }

    pub fn origin(&self) -> &str {
        match self {
            TypeDecl::Newtype { origin, .. }
            | TypeDecl::Struct { origin, .. }
            | TypeDecl::Enum { origin, .. }
            | TypeDecl::StringEnum { origin, .. }
            | TypeDecl::ErrorEnum { origin, .. } => origin,
        }
    }
}

const QUALIFIED_REF_SEPARATOR: char = '\0';

impl Ty {
    /// Construct an origin-qualified named reference for inventory assembly.
    ///
    /// Macro-generated [`Bridged`](crate::Bridged) implementations use this
    /// transient representation so [`crate::registry::build_manifest`] can
    /// distinguish identically named types from different linked crates.
    /// The registry resolves and normalizes it before the manifest crosses
    /// the ABI, so emitters continue to receive ordinary `Ref { name }`
    /// values.
    #[doc(hidden)]
    pub fn qualified_ref(origin: &str, name: &str) -> Self {
        Ty::Ref {
            name: Self::qualified_ref_name(origin, name),
        }
    }

    /// Encode an inventory-only qualified reference name.
    #[doc(hidden)]
    pub fn qualified_ref_name(origin: &str, name: &str) -> String {
        debug_assert!(!origin.contains(QUALIFIED_REF_SEPARATOR));
        debug_assert!(!name.contains(QUALIFIED_REF_SEPARATOR));
        format!("{origin}{QUALIFIED_REF_SEPARATOR}{name}")
    }

    pub(crate) fn split_qualified_ref(name: &str) -> Option<(&str, &str)> {
        name.split_once(QUALIFIED_REF_SEPARATOR)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldDecl {
    /// Rust (snake_case) field name.
    pub name: String,
    /// Name on the wire (camelCase unless overridden).
    pub wire_name: String,
    pub docs: String,
    pub ty: Ty,
    /// Whether the object key must be present. A direct `Option<T>` field is
    /// omittable by default; `#[bridge(required)]` makes the key mandatory
    /// while retaining null as a valid value.
    pub required: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VariantDecl {
    /// Rust (PascalCase) variant name.
    pub name: String,
    /// Discriminator value on the wire (camelCase unless overridden).
    pub wire_name: String,
    pub docs: String,
    pub fields: Vec<FieldDecl>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StringVariantDecl {
    pub name: String,
    pub wire_name: String,
    pub docs: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorVariantDecl {
    pub name: String,
    /// The `code` value carried by the bridge error object.
    pub wire_code: String,
    pub docs: String,
    /// Named fields serialized into the error's `data` object (camelCase).
    pub fields: Vec<FieldDecl>,
}

/// A reference to a type in the portable type system.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Ty {
    Bool,
    U8,
    U16,
    U32,
    I8,
    I16,
    I32,
    /// Exact signed 64-bit integer, encoded as a canonical decimal string.
    I64,
    /// Exact unsigned 64-bit integer, encoded as a canonical decimal string.
    U64,
    F32,
    F64,
    String,
    /// Opaque binary bytes transported through an envelope attachment.
    Bytes,
    /// `()` — valid in return position only.
    Unit,
    /// The one-value JSON null type (`()`) for data positions.
    Null,
    Option {
        inner: Box<Ty>,
    },
    List {
        inner: Box<Ty>,
    },
    /// String-keyed map.
    Map {
        value: Box<Ty>,
    },
    /// Fixed-length heterogeneous tuple (supported arities: 2 through 12).
    Tuple {
        items: Vec<Ty>,
    },
    /// A named type declared in `Manifest::types`.
    Ref {
        name: String,
    },
    /// Transparent schemaless JSON passthrough (`serde_json::Value`):
    /// `typing.Any` in Python, `unknown` in TypeScript, and an unconstrained
    /// value in JSON Schema.
    Json,
    /// Owned numeric buffer transported via an envelope attachment.
    Buf {
        dt: Dtype,
    },
    /// Borrowed numeric slice passed as (ptr, len) (param only).
    Slice {
        dt: Dtype,
    },
}

/// Element type of a raw numeric buffer or slice.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dtype {
    #[serde(rename = "u8")]
    U8,
    #[serde(rename = "i8")]
    I8,
    #[serde(rename = "u16")]
    U16,
    #[serde(rename = "i16")]
    I16,
    #[serde(rename = "u32")]
    U32,
    #[serde(rename = "i32")]
    I32,
    #[serde(rename = "u64")]
    U64,
    #[serde(rename = "i64")]
    I64,
    #[serde(rename = "f32")]
    F32,
    #[serde(rename = "f64")]
    F64,
}

impl Dtype {
    pub fn wire_name(self) -> &'static str {
        match self {
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

    pub fn from_wire_name(name: &str) -> Option<Self> {
        match name {
            "u8" => Some(Dtype::U8),
            "i8" => Some(Dtype::I8),
            "u16" => Some(Dtype::U16),
            "i16" => Some(Dtype::I16),
            "u32" => Some(Dtype::U32),
            "i32" => Some(Dtype::I32),
            "u64" => Some(Dtype::U64),
            "i64" => Some(Dtype::I64),
            "f32" => Some(Dtype::F32),
            "f64" => Some(Dtype::F64),
            _ => None,
        }
    }

    pub fn byte_width(self) -> usize {
        match self {
            Dtype::U8 | Dtype::I8 => 1,
            Dtype::U16 | Dtype::I16 => 2,
            Dtype::U32 | Dtype::I32 | Dtype::F32 => 4,
            Dtype::U64 | Dtype::I64 | Dtype::F64 => 8,
        }
    }

    pub fn alignment(self) -> usize {
        self.byte_width()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParamDecl {
    /// Rust (snake_case) parameter name.
    pub name: String,
    /// Key in the args JSON object (camelCase). Unused for slice params.
    pub wire_name: String,
    pub ty: Ty,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FnDecl {
    /// Rust (snake_case) function name; the symbol is `rspyts_fn__{name}`.
    pub name: String,
    pub docs: String,
    pub params: Vec<ParamDecl>,
    pub ret: Ty,
    /// Name of the `ErrorEnum` this function may fail with, if fallible.
    pub err: Option<String>,
    /// Projections this function appears in. The shim always exists; the
    /// emitters skip targets not listed here.
    pub targets: Vec<Target>,
}

/// A code-generation target.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    #[serde(rename = "python")]
    Python,
    #[serde(rename = "typescript")]
    Typescript,
}

impl Target {
    pub fn all() -> Vec<Target> {
        vec![Target::Python, Target::Typescript]
    }
}

/// A bridged constant: the manifest carries its fully serialized value,
/// captured inside the compiled module at generate time.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConstDecl {
    /// Rust (SCREAMING_SNAKE_CASE) constant name, kept verbatim in both
    /// projections.
    pub name: String,
    pub docs: String,
    pub origin: String,
    pub ty: Ty,
    /// The constant's value, serialized exactly as it would cross the
    /// wire (camelCase struct fields, tagged enums, and so on).
    pub value: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClassDecl {
    pub name: String,
    pub docs: String,
    /// `None` for factory-only classes (constructed exclusively through
    /// static methods returning `Self`).
    pub constructor: Option<CtorDecl>,
    pub methods: Vec<MethodDecl>,
    /// Static methods (`#[bridge(static)]`), in declaration order.
    pub statics: Vec<StaticDecl>,
}

/// A static method on a class. When `returns_self` is true it is a
/// factory: the envelope carries a fresh handle, exactly like the
/// constructor's.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StaticDecl {
    /// Rust method name; the symbol is `rspyts_cls__{Class}__{name}`.
    pub name: String,
    pub docs: String,
    pub params: Vec<ParamDecl>,
    /// Return type; ignored when `returns_self` is true.
    pub ret: Ty,
    pub err: Option<String>,
    pub returns_self: bool,
    pub targets: Vec<Target>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CtorDecl {
    pub docs: String,
    pub params: Vec<ParamDecl>,
    pub err: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MethodDecl {
    /// Rust method name; the symbol is `rspyts_cls__{Class}__{name}`.
    pub name: String,
    pub docs: String,
    /// True for `&mut self` methods (documentation only; locking is uniform).
    pub mutable: bool,
    pub params: Vec<ParamDecl>,
    pub ret: Ty,
    pub err: Option<String>,
    /// Projections this method appears in (see [`FnDecl::targets`]).
    pub targets: Vec<Target>,
}

#[cfg(test)]
mod tests {
    use super::{Dtype, Ty};

    #[test]
    fn every_dtype_has_stable_wire_name_width_and_alignment() {
        for (dtype, name, width) in [
            (Dtype::U8, "u8", 1),
            (Dtype::I8, "i8", 1),
            (Dtype::U16, "u16", 2),
            (Dtype::I16, "i16", 2),
            (Dtype::U32, "u32", 4),
            (Dtype::I32, "i32", 4),
            (Dtype::U64, "u64", 8),
            (Dtype::I64, "i64", 8),
            (Dtype::F32, "f32", 4),
            (Dtype::F64, "f64", 8),
        ] {
            assert_eq!(dtype.wire_name(), name);
            assert_eq!(Dtype::from_wire_name(name), Some(dtype));
            assert_eq!(dtype.byte_width(), width);
            assert_eq!(dtype.alignment(), width);
            assert_eq!(
                serde_json::to_string(&dtype).unwrap(),
                format!(r#""{name}""#)
            );
            assert_eq!(
                serde_json::from_str::<Dtype>(&format!(r#""{name}""#)).unwrap(),
                dtype
            );
        }
        assert_eq!(Dtype::from_wire_name("usize"), None);
    }

    #[test]
    fn null_has_a_stable_manifest_shape() {
        assert_eq!(
            serde_json::to_value(Ty::Null).unwrap(),
            serde_json::json!({"kind": "null"})
        );
    }
}
