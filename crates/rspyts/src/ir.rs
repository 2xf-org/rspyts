//! Stable, host-neutral contract consumed by the package generators.
//!
//! The IR contains only information required to validate and render public
//! Python and TypeScript APIs. It intentionally excludes generator state and
//! backend-specific syntax. All collections are sorted before publication so
//! serialized contracts and generated packages remain deterministic.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The complete application contract consumed by the package generators.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    /// Cargo package that owns the application entry point.
    pub package_name: String,
    /// Version shared by the Cargo, Python, and npm packages.
    pub package_version: String,
    /// Basename of the native Python and WebAssembly bridge module.
    pub module_name: String,
    /// Linked model declarations.
    pub types: Vec<TypeDef>,
    /// Linked typed-error declarations.
    pub errors: Vec<ErrorDef>,
    /// Linked free functions.
    pub functions: Vec<FunctionDef>,
    /// Linked stateful resources.
    pub resources: Vec<ResourceDef>,
    /// Linked constants and statics.
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
    /// Cargo-derived package segment, omitted for the application package.
    pub package: Option<String>,
    /// Rust module segments below the declaring crate root.
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
/// namespace inside one application.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CargoPackageId(
    /// Cargo package name used as the ownership identity.
    pub String,
);

impl CargoPackageId {
    /// Create a package identity from a Cargo package name.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl fmt::Display for CargoPackageId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A Rust item identity inside the application.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionId {
    /// Cargo package that owns the definition.
    pub owner: CargoPackageId,
    /// Stable Rust declaration identity, including its module path.
    pub id: String,
}

impl DefinitionId {
    /// Create a definition identity from its owner and Rust declaration path.
    #[must_use]
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
    /// Rust's unit type, represented as null by host languages.
    Unit,
    /// A Boolean value.
    Bool,
    /// A fixed-width signed or unsigned integer.
    Int {
        /// Whether the integer is signed.
        signed: bool,
        /// Width in bits.
        bits: u16,
    },
    /// An IEEE-754 floating-point value.
    Float {
        /// Width in bits.
        bits: u16,
    },
    /// A UTF-8 string.
    String,
    /// An RFC 3339 date-time value.
    DateTime,
    /// An arbitrary JSON value.
    Json,
    /// A nullable value.
    Option {
        /// Inner non-null value type.
        item: Box<TypeRef>,
    },
    /// A variable-length sequence.
    List {
        /// Element type.
        item: Box<TypeRef>,
    },
    /// A string-keyed map.
    Map {
        /// Mapped value type.
        value: Box<TypeRef>,
    },
    /// A fixed-length heterogeneous sequence.
    Tuple {
        /// Element types in positional order.
        items: Vec<TypeRef>,
    },
    /// A reference to a linked model declaration.
    Named {
        /// Identity of the referenced declaration.
        identity: DefinitionId,
    },
    /// An owned or borrowed byte sequence using the direct binary ABI.
    Bytes,
    /// A fixed-length byte sequence using the direct binary ABI.
    FixedBytes {
        /// Required byte length.
        length: u64,
    },
    /// A contiguous numeric sequence using the direct typed-buffer ABI.
    Buffer {
        /// Primitive buffer element type.
        element: BufferElement,
    },
}

/// A supported element type for a contiguous numeric buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BufferElement {
    /// Unsigned 8-bit integer.
    U8,
    /// Signed 8-bit integer.
    I8,
    /// Unsigned 16-bit integer.
    U16,
    /// Signed 16-bit integer.
    I16,
    /// Unsigned 32-bit integer.
    U32,
    /// Signed 32-bit integer.
    I32,
    /// Unsigned 64-bit integer.
    U64,
    /// Signed 64-bit integer.
    I64,
    /// 32-bit floating-point value.
    F32,
    /// 64-bit floating-point value.
    F64,
}

/// A named Rust type in the application contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TypeDef {
    /// Cargo package that owns the type.
    pub owner: CargoPackageId,
    /// Rust module where the type is declared.
    pub rust_module: String,
    /// Stable Rust declaration identity.
    pub id: String,
    /// Public host-language type name.
    pub name: String,
    /// Normalized Rust documentation, when present.
    pub docs: Option<String>,
    /// Serializable host-language representation.
    pub shape: TypeShape,
}

impl TypeDef {
    /// Return this type's globally unique definition identity.
    #[must_use]
    pub fn identity(&self) -> DefinitionId {
        DefinitionId::new(self.owner.0.clone(), self.id.clone())
    }
}

/// The host-visible shape of a named Rust type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum TypeShape {
    /// An object with named fields.
    Struct {
        /// Fields in Rust declaration order.
        fields: Vec<FieldDef>,
    },
    /// An enum represented by string literals.
    StringEnum {
        /// Variants in Rust declaration order.
        variants: Vec<EnumVariantDef>,
    },
    /// A discriminated union represented by tagged objects.
    TaggedEnum {
        /// Serialized discriminator field name.
        tag: String,
        /// Variants in Rust declaration order.
        variants: Vec<EnumVariantDef>,
    },
    /// A transparent named alias around another contract type.
    Alias {
        /// Host representation wrapped by the alias.
        target: TypeRef,
    },
}

/// A field in a struct or tagged-enum variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldDef {
    /// Original Rust field name.
    pub rust_name: String,
    /// Serialized and host-visible field name.
    pub wire_name: String,
    /// Normalized Rust documentation, when present.
    pub docs: Option<String>,
    /// Host-neutral field type.
    pub ty: TypeRef,
    /// Whether callers must supply the field.
    pub required: bool,
    /// Host-side default for an omitted field.
    pub default: Option<ScalarValue>,
    /// Host-side validation constraints.
    pub constraints: FieldConstraints,
}

/// A scalar value supported by field defaults and constraints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScalarValue {
    /// Boolean scalar.
    Bool(bool),
    /// Signed integer scalar.
    I64(i64),
    /// UTF-8 string scalar.
    String(String),
}

/// Validation constraints copied to generated host models.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldConstraints {
    /// Exact literal accepted by the field.
    pub literal: Option<ScalarValue>,
    /// Inclusive minimum string or collection length.
    pub min_length: Option<u64>,
    /// Inclusive maximum string or collection length.
    pub max_length: Option<u64>,
    /// Inclusive numeric lower bound.
    pub ge: Option<i64>,
    /// Inclusive numeric upper bound.
    pub le: Option<i64>,
}

/// A variant in a string or tagged enum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnumVariantDef {
    /// Original Rust variant name.
    pub rust_name: String,
    /// Serialized variant name.
    pub wire_name: String,
    /// Normalized Rust documentation, when present.
    pub docs: Option<String>,
    /// Fields carried by a tagged variant; empty for string enums.
    pub fields: Vec<FieldDef>,
}

/// A named Rust error in the application contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorDef {
    /// Cargo package that owns the error.
    pub owner: CargoPackageId,
    /// Rust module where the error is declared.
    pub rust_module: String,
    /// Stable Rust declaration identity.
    pub id: String,
    /// Public host-language error class name.
    pub name: String,
    /// Normalized Rust documentation, when present.
    pub docs: Option<String>,
}

impl ErrorDef {
    /// Return this error's globally unique definition identity.
    #[must_use]
    pub fn identity(&self) -> DefinitionId {
        DefinitionId::new(self.owner.0.clone(), self.id.clone())
    }
}

/// A free function exposed to Python and TypeScript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FunctionDef {
    /// Cargo package that owns the function.
    pub owner: CargoPackageId,
    /// Rust module where the function is declared.
    pub rust_module: String,
    /// Original Rust function name.
    pub rust_name: String,
    /// Public host-language function name.
    pub host_name: String,
    /// The globally unique name of the native bridge target.
    pub native_name: String,
    /// Normalized Rust documentation, when present.
    pub docs: Option<String>,
    /// Parameters in declaration order.
    pub params: Vec<ParamDef>,
    /// Successful return type.
    pub returns: TypeRef,
    /// Typed error returned by the function, when any.
    pub error: Option<DefinitionId>,
}

/// A parameter in an exported function, constructor, or method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParamDef {
    /// Original Rust parameter name.
    pub rust_name: String,
    /// Public host-language parameter name.
    pub host_name: String,
    /// Host-neutral parameter type.
    pub ty: TypeRef,
}

/// An exported Rust type that keeps native state between calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceDef {
    /// Cargo package that owns the resource.
    pub owner: CargoPackageId,
    /// Rust module where the resource is declared.
    pub rust_module: String,
    /// Public host-language resource class name.
    pub name: String,
    /// The globally unique name of the native bridge class.
    pub native_name: String,
    /// Normalized Rust documentation, when present.
    pub docs: Option<String>,
    /// Public constructors, including named factories.
    pub constructors: Vec<FunctionDef>,
    /// Public instance methods.
    pub methods: Vec<MethodDef>,
}

/// A callable method on an exported resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MethodDef {
    /// Original Rust method name.
    pub rust_name: String,
    /// Public host-language method name.
    pub host_name: String,
    /// Normalized Rust documentation, when present.
    pub docs: Option<String>,
    /// Parameters following the receiver.
    pub params: Vec<ParamDef>,
    /// Successful return type.
    pub returns: TypeRef,
    /// Typed error returned by the method, when any.
    pub error: Option<DefinitionId>,
}

/// A Rust constant exposed to Python and TypeScript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConstantDef {
    /// Cargo package that owns the constant.
    pub owner: CargoPackageId,
    /// Rust module where the constant is declared.
    pub rust_module: String,
    /// Public host-language constant name.
    pub host_name: String,
    /// Normalized Rust documentation, when present.
    pub docs: Option<String>,
    /// Host-neutral constant type.
    pub ty: TypeRef,
    /// Serialized constant value.
    pub value: Value,
}
