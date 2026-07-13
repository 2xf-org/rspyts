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
    /// ABI version string, e.g. `"0.1"`.
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
            TypeDecl::Struct { name, .. }
            | TypeDecl::Enum { name, .. }
            | TypeDecl::StringEnum { name, .. }
            | TypeDecl::ErrorEnum { name, .. } => name,
        }
    }

    pub fn origin(&self) -> &str {
        match self {
            TypeDecl::Struct { origin, .. }
            | TypeDecl::Enum { origin, .. }
            | TypeDecl::StringEnum { origin, .. }
            | TypeDecl::ErrorEnum { origin, .. } => origin,
        }
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
    /// True for `Option<T>` fields: emitters give these a null default.
    pub optional: bool,
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
    F32,
    F64,
    String,
    /// `()` — valid in return position only.
    Unit,
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
    /// A named type declared in `Manifest::types`.
    Ref {
        name: String,
    },
    /// Schemaless JSON passthrough (`rspyts::Json`): `Any` in Python,
    /// `unknown` in TypeScript, `{}` in JSON Schema.
    Json,
    /// Owned numeric buffer returned via the envelope tail (return only).
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
    #[serde(rename = "i16")]
    I16,
    #[serde(rename = "i32")]
    I32,
    #[serde(rename = "f32")]
    F32,
    #[serde(rename = "f64")]
    F64,
}

impl Dtype {
    pub fn wire_name(self) -> &'static str {
        match self {
            Dtype::U8 => "u8",
            Dtype::I16 => "i16",
            Dtype::I32 => "i32",
            Dtype::F32 => "f32",
            Dtype::F64 => "f64",
        }
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
