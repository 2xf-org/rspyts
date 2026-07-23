//! Collect and validate all exports linked into one application.
//!
//! Procedural macros submit lazy registrations through `inventory`. Discovery
//! materializes those records, validates identities, references, host names,
//! and lossless type semantics, then sorts the resulting manifest so generated
//! packages are deterministic across link orders.

use std::collections::{BTreeMap, BTreeSet};

use crate::ir::{
    CargoPackageId, ConstantDef, DefinitionId, ErrorDef, FunctionDef, Manifest, ResourceDef,
    TypeDef, TypeRef, TypeShape,
};

/// A linked type contract produced by [`crate::Model`].
pub struct TypeRegistration(
    /// Construct the type definition at discovery time.
    pub fn() -> TypeDef,
);
/// A linked error contract produced by [`crate::Error`].
pub struct ErrorRegistration(
    /// Construct the error definition at discovery time.
    pub fn() -> ErrorDef,
);
/// A linked function contract produced by [`crate::export`].
pub struct FunctionRegistration(
    /// Construct the function definition at discovery time.
    pub fn() -> FunctionDef,
);
/// A linked resource contract produced by [`crate::export`].
pub struct ResourceRegistration(
    /// Construct the resource definition at discovery time.
    pub fn() -> ResourceDef,
);
/// A linked constant contract produced by [`crate::export`].
pub struct ConstantRegistration(
    /// Serialize and construct the constant definition at discovery time.
    pub fn() -> Result<ConstantDef, String>,
);

inventory::collect!(TypeRegistration);
inventory::collect!(ErrorRegistration);
inventory::collect!(FunctionRegistration);
inventory::collect!(ResourceRegistration);
inventory::collect!(ConstantRegistration);

/// An invalid collection of linked application exports.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// Two declarations publish the same name in one generated namespace.
    #[error("duplicate {kind} name `{name}` in namespace `{namespace}`")]
    DuplicateName {
        /// Kind of declaration that collided.
        kind: &'static str,
        /// Colliding public name.
        name: String,
        /// Generated namespace containing the collision.
        namespace: String,
    },
    /// Two linked model or error declarations share one internal identity.
    #[error("duplicate {kind} identity `{identity}`")]
    DuplicateIdentity {
        /// Kind of declaration that collided.
        kind: &'static str,
        /// Repeated internal identity.
        identity: DefinitionId,
    },
    /// A contract references a model that is not linked into the application.
    #[error("type `{identity}` is used but is not linked into the application")]
    MissingType {
        /// Referenced but unregistered model identity.
        identity: DefinitionId,
    },
    /// A callable references an error that is not linked into the application.
    #[error("error `{identity}` is used but is not linked into the application")]
    MissingError {
        /// Referenced but unregistered error identity.
        identity: DefinitionId,
    },
    /// An exported constant could not be serialized into the contract.
    #[error("invalid exported constant: {0}")]
    InvalidConstant(String),
    /// A linked type has an ambiguous or lossy host representation.
    #[error("invalid type contract: {0}")]
    InvalidType(String),
}

/// Collect and validate every export linked into an application.
///
/// # Errors
///
/// Returns [`RegistryError`] when exports conflict, reference an unlinked type
/// or error, or contain a constant that cannot be serialized.
pub fn manifest(
    package_name: &str,
    package_version: &str,
    module_name: &str,
) -> Result<Manifest, RegistryError> {
    let types = inventory::iter::<TypeRegistration>
        .into_iter()
        .map(|item| (item.0)())
        .collect::<Vec<_>>();
    let errors = inventory::iter::<ErrorRegistration>
        .into_iter()
        .map(|item| (item.0)())
        .collect::<Vec<_>>();
    let functions = inventory::iter::<FunctionRegistration>
        .into_iter()
        .map(|item| (item.0)())
        .collect::<Vec<_>>();
    let resources = inventory::iter::<ResourceRegistration>
        .into_iter()
        .map(|item| (item.0)())
        .collect::<Vec<_>>();
    let constants = inventory::iter::<ConstantRegistration>
        .into_iter()
        .map(|item| (item.0)().map_err(RegistryError::InvalidConstant))
        .collect::<Result<Vec<_>, _>>()?;

    unique_global_names(
        "native export",
        functions
            .iter()
            .map(|item| item.native_name.as_str())
            .chain(resources.iter().map(|item| item.native_name.as_str())),
    )?;

    let type_ids = unique_ids("type", types.iter().map(TypeDef::identity))?;
    let error_ids = unique_ids("error", errors.iter().map(ErrorDef::identity))?;
    validate_type_definitions(&types)?;
    let type_definitions = types
        .iter()
        .map(|definition| (definition.identity(), definition))
        .collect::<BTreeMap<_, _>>();
    for reference in all_type_refs(&types, &functions, &resources, &constants) {
        validate_ref(reference, &type_ids)?;
        validate_lossless_ref(reference, &type_definitions)?;
    }
    for identity in functions
        .iter()
        .filter_map(|item| item.error.as_ref())
        .chain(resources.iter().flat_map(|item| {
            item.constructors
                .iter()
                .filter_map(|method| method.error.as_ref())
                .chain(
                    item.methods
                        .iter()
                        .filter_map(|method| method.error.as_ref()),
                )
        }))
    {
        if !error_ids.contains(identity) {
            return Err(RegistryError::MissingError {
                identity: identity.clone(),
            });
        }
    }

    let mut manifest = Manifest {
        package_name: package_name.to_owned(),
        package_version: package_version.to_owned(),
        module_name: module_name.to_owned(),
        types,
        errors,
        functions,
        resources,
        constants,
    };
    let namespace_basis = manifest.clone();
    manifest.types.sort_by_key(|item| {
        (
            namespace_basis.namespace(&item.owner, &item.rust_module),
            item.name.clone(),
            item.id.clone(),
        )
    });
    manifest.errors.sort_by_key(|item| {
        (
            namespace_basis.namespace(&item.owner, &item.rust_module),
            item.name.clone(),
            item.id.clone(),
        )
    });
    manifest.functions.sort_by_key(|item| {
        (
            namespace_basis.namespace(&item.owner, &item.rust_module),
            item.host_name.clone(),
        )
    });
    manifest.resources.sort_by_key(|item| {
        (
            namespace_basis.namespace(&item.owner, &item.rust_module),
            item.name.clone(),
        )
    });
    manifest.constants.sort_by_key(|item| {
        (
            namespace_basis.namespace(&item.owner, &item.rust_module),
            item.host_name.clone(),
        )
    });
    unique_scoped_names(
        &manifest,
        "type",
        manifest
            .types
            .iter()
            .map(|item| (item.name.as_str(), &item.owner, item.rust_module.as_str())),
    )?;
    unique_scoped_names(
        &manifest,
        "error",
        manifest
            .errors
            .iter()
            .map(|item| (item.name.as_str(), &item.owner, item.rust_module.as_str())),
    )?;
    unique_scoped_names(
        &manifest,
        "constant",
        manifest.constants.iter().map(|item| {
            (
                item.host_name.as_str(),
                &item.owner,
                item.rust_module.as_str(),
            )
        }),
    )?;
    unique_scoped_names(
        &manifest,
        "function",
        manifest.functions.iter().map(|item| {
            (
                item.host_name.as_str(),
                &item.owner,
                item.rust_module.as_str(),
            )
        }),
    )?;
    unique_scoped_names(
        &manifest,
        "resource",
        manifest
            .resources
            .iter()
            .map(|item| (item.name.as_str(), &item.owner, item.rust_module.as_str())),
    )?;
    Ok(manifest)
}

// Model-shape validation ---------------------------------------------------

/// Validate wire-name uniqueness and tagged-enum discriminator invariants.
fn validate_type_definitions(types: &[TypeDef]) -> Result<(), RegistryError> {
    for definition in types {
        match &definition.shape {
            TypeShape::Struct { fields } => validate_unique_wire_names(
                &definition.name,
                "field",
                fields.iter().map(|field| field.wire_name.as_str()),
            )?,
            TypeShape::StringEnum { variants } => validate_unique_wire_names(
                &definition.name,
                "variant",
                variants.iter().map(|variant| variant.wire_name.as_str()),
            )?,
            TypeShape::TaggedEnum { tag, variants } => {
                if variants.is_empty() {
                    return Err(RegistryError::InvalidType(format!(
                        "tagged enum `{}` must declare at least one variant",
                        definition.name
                    )));
                }
                validate_unique_wire_names(
                    &definition.name,
                    "variant",
                    variants.iter().map(|variant| variant.wire_name.as_str()),
                )?;
                for variant in variants {
                    validate_unique_wire_names(
                        &format!("{}::{}", definition.name, variant.rust_name),
                        "field",
                        variant.fields.iter().map(|field| field.wire_name.as_str()),
                    )?;
                    if variant.fields.iter().any(|field| field.wire_name == *tag) {
                        return Err(RegistryError::InvalidType(format!(
                            "tagged enum variant `{}::{}` has a field named `{tag}` that conflicts with its discriminator",
                            definition.name, variant.rust_name
                        )));
                    }
                }
            }
            TypeShape::Alias { .. } => {}
        }
    }
    Ok(())
}

/// Reject duplicate serialized names within a model scope.
fn validate_unique_wire_names<'a>(
    owner: &str,
    kind: &str,
    names: impl Iterator<Item = &'a str>,
) -> Result<(), RegistryError> {
    let mut seen = BTreeSet::new();
    for name in names {
        if !seen.insert(name) {
            return Err(RegistryError::InvalidType(format!(
                "`{owner}` declares duplicate {kind} wire name `{name}`"
            )));
        }
    }
    Ok(())
}

/// Reject references whose distinct Rust states collapse in JSON-shaped hosts.
fn validate_lossless_ref(
    reference: &TypeRef,
    types: &BTreeMap<DefinitionId, &TypeDef>,
) -> Result<(), RegistryError> {
    match reference {
        TypeRef::Option { item } => {
            if admits_null(item, types, &mut BTreeSet::new()) {
                return Err(RegistryError::InvalidType(format!(
                    "`Option<{item:?}>` cannot preserve its distinct Rust states because the inner type also admits null"
                )));
            }
            validate_lossless_ref(item, types)
        }
        TypeRef::List { item } => validate_lossless_ref(item, types),
        TypeRef::Map { value } => validate_lossless_ref(value, types),
        TypeRef::Tuple { items } => {
            for item in items {
                validate_lossless_ref(item, types)?;
            }
            Ok(())
        }
        TypeRef::Named { identity } => {
            if let Some(TypeDef {
                shape: TypeShape::Alias { target },
                ..
            }) = types.get(identity).copied()
            {
                validate_lossless_ref(target, types)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Return whether a type can produce a JSON null after resolving aliases.
fn admits_null(
    reference: &TypeRef,
    types: &BTreeMap<DefinitionId, &TypeDef>,
    visiting: &mut BTreeSet<DefinitionId>,
) -> bool {
    match reference {
        TypeRef::Unit | TypeRef::Json | TypeRef::Option { .. } => true,
        TypeRef::Named { identity } if visiting.insert(identity.clone()) => {
            let result = matches!(
                types.get(identity).map(|definition| &definition.shape),
                Some(TypeShape::Alias { target }) if admits_null(target, types, visiting)
            );
            visiting.remove(identity);
            result
        }
        _ => false,
    }
}

// Identity and namespace validation ----------------------------------------

/// Require names to be unique across the complete native module.
fn unique_global_names<'a>(
    kind: &'static str,
    values: impl Iterator<Item = &'a str>,
) -> Result<(), RegistryError> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(RegistryError::DuplicateName {
                kind,
                name: value.to_owned(),
                namespace: "the application".to_owned(),
            });
        }
    }
    Ok(())
}

/// Require names to be unique inside each generated namespace.
fn unique_scoped_names<'a>(
    manifest: &Manifest,
    kind: &'static str,
    values: impl Iterator<Item = (&'a str, &'a CargoPackageId, &'a str)>,
) -> Result<(), RegistryError> {
    let mut seen = BTreeSet::new();
    for (name, owner, rust_module) in values {
        let namespace = manifest.namespace(owner, rust_module);
        if !seen.insert((namespace.clone(), name)) {
            let namespace = namespace.display();
            return Err(RegistryError::DuplicateName {
                kind,
                name: name.to_owned(),
                namespace: if namespace.is_empty() {
                    "<root>".to_owned()
                } else {
                    namespace
                },
            });
        }
    }
    Ok(())
}

/// Collect identities while rejecting duplicates.
fn unique_ids(
    kind: &'static str,
    values: impl Iterator<Item = DefinitionId>,
) -> Result<BTreeSet<DefinitionId>, RegistryError> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value.clone()) {
            return Err(RegistryError::DuplicateIdentity {
                kind,
                identity: value,
            });
        }
    }
    Ok(seen)
}

/// Validate every named type reference recursively.
fn validate_ref(reference: &TypeRef, types: &BTreeSet<DefinitionId>) -> Result<(), RegistryError> {
    match reference {
        TypeRef::Named { identity } if !types.contains(identity) => {
            Err(RegistryError::MissingType {
                identity: identity.clone(),
            })
        }
        TypeRef::Option { item } | TypeRef::List { item } => validate_ref(item, types),
        TypeRef::Map { value } => validate_ref(value, types),
        TypeRef::Tuple { items } => {
            for item in items {
                validate_ref(item, types)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Return every type reference reachable from linked public declarations.
fn all_type_refs<'a>(
    types: &'a [TypeDef],
    functions: &'a [FunctionDef],
    resources: &'a [ResourceDef],
    constants: &'a [ConstantDef],
) -> Vec<&'a TypeRef> {
    let mut result = Vec::new();
    for definition in types {
        match &definition.shape {
            TypeShape::Struct { fields } => {
                result.extend(fields.iter().map(|field| &field.ty));
            }
            TypeShape::TaggedEnum { variants, .. } => {
                result.extend(
                    variants
                        .iter()
                        .flat_map(|variant| variant.fields.iter().map(|field| &field.ty)),
                );
            }
            TypeShape::Alias { target } => result.push(target),
            TypeShape::StringEnum { .. } => {}
        }
    }
    for function in functions {
        result.extend(function.params.iter().map(|param| &param.ty));
        result.push(&function.returns);
    }
    for resource in resources {
        for method in &resource.constructors {
            result.extend(method.params.iter().map(|param| &param.ty));
        }
        for method in &resource.methods {
            result.extend(method.params.iter().map(|param| &param.ty));
            result.push(&method.returns);
        }
    }
    for constant in constants {
        result.push(&constant.ty);
    }
    result
}

#[cfg(test)]
mod tests {
    use crate::ir::{EnumVariantDef, FieldConstraints, FieldDef};

    use super::*;

    fn definition(name: &str, shape: TypeShape) -> TypeDef {
        TypeDef {
            owner: CargoPackageId::new("test"),
            rust_module: "test".to_owned(),
            id: format!("test::{name}"),
            name: name.to_owned(),
            docs: None,
            shape,
        }
    }

    fn field(name: &str) -> FieldDef {
        FieldDef {
            rust_name: name.to_owned(),
            wire_name: name.to_owned(),
            docs: None,
            ty: TypeRef::String,
            required: true,
            default: None,
            constraints: FieldConstraints::default(),
        }
    }

    #[test]
    fn nullable_inner_options_are_rejected_after_alias_resolution() {
        let alias = definition(
            "Nullable",
            TypeShape::Alias {
                target: TypeRef::Option {
                    item: Box::new(TypeRef::String),
                },
            },
        );
        let types = BTreeMap::from([(alias.identity(), &alias)]);
        for inner in [
            TypeRef::Unit,
            TypeRef::Json,
            TypeRef::Option {
                item: Box::new(TypeRef::String),
            },
            TypeRef::Named {
                identity: alias.identity(),
            },
        ] {
            let reference = TypeRef::Option {
                item: Box::new(inner),
            };
            assert!(validate_lossless_ref(&reference, &types).is_err());
        }
        assert!(
            validate_lossless_ref(
                &TypeRef::Option {
                    item: Box::new(TypeRef::String),
                },
                &types,
            )
            .is_ok()
        );
    }

    #[test]
    fn ambiguous_named_wire_shapes_are_rejected() {
        let duplicate = definition(
            "Duplicate",
            TypeShape::Struct {
                fields: vec![field("value"), field("value")],
            },
        );
        let empty = definition(
            "Empty",
            TypeShape::TaggedEnum {
                tag: "kind".to_owned(),
                variants: Vec::new(),
            },
        );
        let collision = definition(
            "Collision",
            TypeShape::TaggedEnum {
                tag: "kind".to_owned(),
                variants: vec![EnumVariantDef {
                    rust_name: "Value".to_owned(),
                    wire_name: "value".to_owned(),
                    docs: None,
                    fields: vec![field("kind")],
                }],
            },
        );

        assert!(validate_type_definitions(&[duplicate]).is_err());
        assert!(validate_type_definitions(&[empty]).is_err());
        assert!(validate_type_definitions(&[collision]).is_err());
    }
}
