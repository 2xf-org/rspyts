//! The small description that the generators read.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const IR_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub ir_version: u32,
    pub package_name: String,
    pub package_version: String,
    pub module_name: String,
    pub types: Vec<TypeDef>,
    pub errors: Vec<ErrorDef>,
    pub functions: Vec<FunctionDef>,
    pub resources: Vec<ResourceDef>,
    pub constants: Vec<ConstantDef>,
}

impl Manifest {
    pub fn canonical_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// The Cargo package that declared an item.
///
/// rspyts keeps this value only to distinguish Rust items in one aggregate
/// binding. It does not generate a host package for each Cargo package.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CargoPackageId(pub String);

impl CargoPackageId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl fmt::Display for CargoPackageId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A Rust item identity inside the aggregate binding.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

impl TypeDef {
    pub fn identity(&self) -> DefinitionId {
        DefinitionId::new(self.owner.0.clone(), self.id.clone())
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScalarValue {
    Bool(bool),
    I64(i64),
    String(String),
}

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

impl ErrorDef {
    pub fn identity(&self) -> DefinitionId {
        DefinitionId::new(self.owner.0.clone(), self.id.clone())
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceDef {
    pub owner: CargoPackageId,
    pub id: String,
    pub name: String,
    pub docs: Option<String>,
    pub constructors: Vec<FunctionDef>,
    pub methods: Vec<MethodDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MethodDef {
    pub rust_name: String,
    pub host_name: String,
    pub docs: Option<String>,
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
    pub ty: TypeRef,
    pub value: Value,
}
