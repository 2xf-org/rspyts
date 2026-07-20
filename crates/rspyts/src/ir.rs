//! The small description that the generators read.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The serialized contract format produced by this release.
pub const IR_VERSION: u32 = 2;

/// The complete application contract consumed by the package generators.
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
    /// Return the Rust-shaped public namespace for one exported declaration.
    #[must_use]
    pub fn namespace(&self, owner: &CargoPackageId, rust_module: &str) -> Namespace {
        let prefix_length = self.shared_package_prefix_length();
        let owner_parts = owner.0.split('-').collect::<Vec<_>>();
        let package = owner_parts
            .get(prefix_length..)
            .filter(|parts| !parts.is_empty())
            .map(|parts| parts.join("-"));
        let modules = rust_module.split("::").skip(1).map(str::to_owned).collect();
        Namespace { package, modules }
    }

    fn shared_package_prefix_length(&self) -> usize {
        let mut packages = BTreeSet::from([self.package_name.as_str()]);
        packages.extend(self.types.iter().map(|item| item.owner.0.as_str()));
        packages.extend(self.errors.iter().map(|item| item.owner.0.as_str()));
        packages.extend(self.functions.iter().map(|item| item.owner.0.as_str()));
        packages.extend(self.resources.iter().map(|item| item.owner.0.as_str()));
        packages.extend(self.constants.iter().map(|item| item.owner.0.as_str()));
        let parts = packages
            .into_iter()
            .map(|package| package.split('-').collect::<Vec<_>>())
            .collect::<Vec<_>>();
        let shortest = parts.iter().map(Vec::len).min().unwrap_or(0);
        (0..shortest)
            .take_while(|index| {
                parts
                    .iter()
                    .all(|package| package[*index] == parts[0][*index])
            })
            .count()
    }
}

/// A generated namespace derived from one Cargo package and Rust module.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Namespace {
    pub package: Option<String>,
    pub modules: Vec<String>,
}

impl Namespace {
    /// Return the aggregate package root.
    #[must_use]
    pub const fn root() -> Self {
        Self {
            package: None,
            modules: Vec::new(),
        }
    }

    /// Return the namespace segments used in a Python package.
    #[must_use]
    pub fn python_segments(&self) -> Vec<String> {
        self.package
            .iter()
            .map(|package| package.replace('-', "_"))
            .chain(self.modules.iter().cloned())
            .collect()
    }

    /// Return the namespace segments used in a TypeScript package subpath.
    #[must_use]
    pub fn typescript_segments(&self) -> Vec<String> {
        self.package
            .iter()
            .cloned()
            .chain(self.modules.iter().cloned())
            .collect()
    }

    /// Return a readable Rust-style namespace for diagnostics.
    #[must_use]
    pub fn display(&self) -> String {
        self.package
            .iter()
            .cloned()
            .chain(self.modules.iter().cloned())
            .collect::<Vec<_>>()
            .join("::")
    }
}

/// The Cargo package that declared an item.
///
/// rspyts uses this value to distinguish Rust items and derive their public
/// namespace inside one aggregate binding.
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

/// A type that can cross the generated Python and TypeScript boundaries.
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

/// A supported element type for a contiguous numeric buffer.
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

/// A named Rust type in the application contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TypeDef {
    pub owner: CargoPackageId,
    pub rust_module: String,
    pub id: String,
    pub name: String,
    pub docs: Option<String>,
    pub shape: TypeShape,
}

impl TypeDef {
    #[must_use]
    pub fn identity(&self) -> DefinitionId {
        DefinitionId::new(self.owner.0.clone(), self.id.clone())
    }
}

/// The host-visible shape of a named Rust type.
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

/// A field in a struct or tagged-enum variant.
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

/// A scalar value supported by field defaults and constraints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScalarValue {
    Bool(bool),
    I64(i64),
    String(String),
}

/// Validation constraints copied to generated host models.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldConstraints {
    pub literal: Option<ScalarValue>,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub ge: Option<i64>,
    pub le: Option<i64>,
}

/// A variant in a string or tagged enum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnumVariantDef {
    pub rust_name: String,
    pub wire_name: String,
    pub docs: Option<String>,
    pub fields: Vec<FieldDef>,
}

/// A named Rust error in the application contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorDef {
    pub owner: CargoPackageId,
    pub rust_module: String,
    pub id: String,
    pub name: String,
    pub docs: Option<String>,
}

impl ErrorDef {
    #[must_use]
    pub fn identity(&self) -> DefinitionId {
        DefinitionId::new(self.owner.0.clone(), self.id.clone())
    }
}

/// A free function exposed to Python and TypeScript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FunctionDef {
    pub owner: CargoPackageId,
    pub rust_module: String,
    pub rust_name: String,
    pub host_name: String,
    pub docs: Option<String>,
    pub params: Vec<ParamDef>,
    pub returns: TypeRef,
    pub error: Option<DefinitionId>,
}

/// A parameter in an exported function, constructor, or method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParamDef {
    pub rust_name: String,
    pub host_name: String,
    pub ty: TypeRef,
}

/// An exported Rust type that keeps native state between calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceDef {
    pub owner: CargoPackageId,
    pub rust_module: String,
    pub name: String,
    pub docs: Option<String>,
    pub constructors: Vec<FunctionDef>,
    pub methods: Vec<MethodDef>,
}

/// A callable method on an exported resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MethodDef {
    pub rust_name: String,
    pub host_name: String,
    pub docs: Option<String>,
    pub params: Vec<ParamDef>,
    pub returns: TypeRef,
    pub error: Option<DefinitionId>,
}

/// A Rust constant exposed to Python and TypeScript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConstantDef {
    pub owner: CargoPackageId,
    pub rust_module: String,
    pub host_name: String,
    pub docs: Option<String>,
    pub ty: TypeRef,
    pub value: Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(binding: &str, declarations: &[(&str, &str)]) -> Manifest {
        Manifest {
            ir_version: IR_VERSION,
            package_name: binding.to_owned(),
            package_version: "1.0.1".to_owned(),
            module_name: "native".to_owned(),
            types: Vec::new(),
            errors: Vec::new(),
            functions: declarations
                .iter()
                .map(|(owner, rust_module)| FunctionDef {
                    owner: CargoPackageId::new(*owner),
                    rust_module: (*rust_module).to_owned(),
                    rust_name: format!("call_{}", owner.replace('-', "_")),
                    host_name: format!("call{}", owner.replace('-', "")),
                    docs: None,
                    params: Vec::new(),
                    returns: TypeRef::Unit,
                    error: None,
                })
                .collect(),
            resources: Vec::new(),
            constants: Vec::new(),
        }
    }

    #[test]
    fn strips_the_longest_shared_cargo_prefix() {
        let manifest = manifest(
            "neurovirtual-core",
            &[
                ("neurovirtual-hardware", "neurovirtual_hardware::device"),
                ("neurovirtual-evaluation", "neurovirtual_evaluation::run"),
            ],
        );

        let namespace = manifest.namespace(
            &CargoPackageId::new("neurovirtual-hardware"),
            "neurovirtual_hardware::device::bwiii::position",
        );

        assert_eq!(
            namespace.python_segments(),
            ["hardware", "device", "bwiii", "position"]
        );
        assert_eq!(
            namespace.typescript_segments(),
            ["hardware", "device", "bwiii", "position"]
        );
    }

    #[test]
    fn keeps_an_unrelated_cargo_name_as_one_segment() {
        let manifest = manifest("application", &[("domain-models", "domain_models")]);
        let dependency = manifest.namespace(&CargoPackageId::new("domain-models"), "domain_models");
        let binding = manifest.namespace(&CargoPackageId::new("application"), "application");

        assert_eq!(dependency.python_segments(), ["domain_models"]);
        assert_eq!(dependency.typescript_segments(), ["domain-models"]);
        assert_eq!(binding.python_segments(), ["application"]);
    }

    #[test]
    fn excludes_the_crate_name_and_has_no_depth_limit() {
        let manifest = manifest("hello-world", &[("hello-world-api", "hello_world_api")]);
        let root = manifest.namespace(&CargoPackageId::new("hello-world-api"), "hello_world_api");
        let deep = manifest.namespace(
            &CargoPackageId::new("hello-world-api"),
            "hello_world_api::one::two::three::four",
        );

        assert_eq!(root.python_segments(), ["api"]);
        assert_eq!(
            deep.python_segments(),
            ["api", "one", "two", "three", "four"]
        );
    }

    #[test]
    fn crate_root_items_in_the_binding_stay_at_the_host_root() {
        let manifest = manifest("example", &[("example", "example")]);
        let namespace = manifest.namespace(&CargoPackageId::new("example"), "example");

        assert_eq!(namespace, Namespace::root());
    }
}
