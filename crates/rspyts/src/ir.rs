use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

pub const IR_VERSION: u32 = 6;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub ir_version: u32,
    pub crate_name: String,
    pub crate_version: String,
    pub module_name: String,
    /// Foreign schema identities referenced by this package's public contract.
    ///
    /// Their linked definitions appear only as compiler evidence in `imports`,
    /// never in the local definition vectors. The compiler compares the snapshots
    /// with dependency locks before emitting external host-package imports.
    pub imports: Vec<ImportedPackage>,
    pub types: Vec<TypeDef>,
    pub errors: Vec<ErrorDef>,
    pub functions: Vec<FunctionDef>,
    pub resources: Vec<ResourceDef>,
    pub constants: Vec<ConstantDef>,
}

/// Stable Cargo package ownership captured in the crate where a macro expands.
///
/// This is the exact `CARGO_PKG_NAME`, not the final cdylib package. Cargo may
/// link registrations from multiple packages into one artifact, so ownership
/// cannot be inferred after linking.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CargoPackageId(pub String);

impl CargoPackageId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CargoPackageId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A definition's globally unambiguous semantic identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionId {
    pub owner: CargoPackageId,
    pub id: String,
}

impl DefinitionId {
    pub fn new(owner: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            owner: CargoPackageId::new(owner),
            id: id.into(),
        }
    }
}

impl fmt::Display for DefinitionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}::{}", self.owner, self.id)
    }
}

/// Foreign identities required to compile this package's host surface.
///
/// These linked snapshots are compiler evidence, not locally emitted models.
/// The compiler compares them byte-for-byte with the declared dependency lock,
/// then emits normal host-package imports for the dependency-owned identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportedPackage {
    pub owner: CargoPackageId,
    pub types: Vec<TypeDef>,
    pub errors: Vec<ErrorDef>,
}

impl Manifest {
    pub fn canonical_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum TypeRef {
    Unit,
    Bool,
    Int { signed: bool, bits: u16 },
    Float { bits: u16 },
    String,
    DateTime,
    Json,
    Option { item: Box<TypeRef> },
    List { item: Box<TypeRef> },
    Map { value: Box<TypeRef> },
    Tuple { items: Vec<TypeRef> },
    Named { identity: DefinitionId },
    Bytes,
    FixedBytes { length: u64 },
    Buffer { element: BufferElement },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BufferElement {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    F32,
    F64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TypeDef {
    pub owner: CargoPackageId,
    pub id: String,
    pub name: String,
    pub docs: Option<String>,
    pub shape: TypeShape,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum TypeShape {
    Struct {
        fields: Vec<FieldDef>,
    },
    StringEnum {
        variants: Vec<EnumVariantDef>,
    },
    TaggedEnum {
        tag: String,
        variants: Vec<EnumVariantDef>,
    },
    Alias {
        target: TypeRef,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldDef {
    pub rust_name: String,
    pub wire_name: String,
    pub docs: Option<String>,
    pub ty: TypeRef,
    pub required: bool,
    pub default: Option<ScalarValue>,
    pub constraints: FieldConstraints,
}

/// A host-neutral scalar used by explicit field defaults and literal constraints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScalarValue {
    Bool(bool),
    I64(i64),
    String(String),
}

/// Minimal validation rules shared exactly by Rust, Python, and TypeScript.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldConstraints {
    pub literal: Option<ScalarValue>,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub ge: Option<i64>,
    pub le: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnumVariantDef {
    pub rust_name: String,
    pub wire_name: String,
    pub docs: Option<String>,
    pub fields: Vec<FieldDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorDef {
    pub owner: CargoPackageId,
    pub id: String,
    pub name: String,
    pub docs: Option<String>,
    pub variants: Vec<ErrorVariantDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorVariantDef {
    pub rust_name: String,
    pub code: String,
    pub docs: Option<String>,
    pub fields: Vec<FieldDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FunctionDef {
    pub owner: CargoPackageId,
    pub rust_name: String,
    pub host_name: String,
    pub docs: Option<String>,
    pub target: Target,
    pub params: Vec<ParamDef>,
    pub returns: TypeRef,
    pub error: Option<DefinitionId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParamDef {
    pub rust_name: String,
    pub host_name: String,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    Both,
    Python,
    Typescript,
    Static,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceDef {
    pub owner: CargoPackageId,
    pub id: String,
    pub name: String,
    pub docs: Option<String>,
    pub target: Target,
    pub constructors: Vec<FunctionDef>,
    pub methods: Vec<MethodDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MethodDef {
    pub rust_name: String,
    pub host_name: String,
    pub docs: Option<String>,
    pub target: Target,
    pub mutable: bool,
    pub params: Vec<ParamDef>,
    pub returns: TypeRef,
    pub error: Option<DefinitionId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConstantDef {
    pub owner: CargoPackageId,
    pub rust_name: String,
    pub host_name: String,
    pub docs: Option<String>,
    pub target: Target,
    pub ty: TypeRef,
    pub value: Value,
}
