//! Collect all exports that are linked into one application binding.

use std::collections::BTreeSet;

use crate::ir::{
    CargoPackageId, ConstantDef, DefinitionId, ErrorDef, FunctionDef, IR_VERSION, Manifest,
    ResourceDef, TypeDef, TypeRef, TypeShape,
};

/// A linked type contract produced by [`crate::Model`].
pub struct TypeRegistration(pub fn() -> TypeDef);
/// A linked error contract produced by [`crate::Error`].
pub struct ErrorRegistration(pub fn() -> ErrorDef);
/// A linked function contract produced by [`crate::export`].
pub struct FunctionRegistration(pub fn() -> FunctionDef);
/// A linked resource contract produced by [`crate::export`].
pub struct ResourceRegistration(pub fn() -> ResourceDef);
/// A linked constant contract produced by [`crate::export`].
pub struct ConstantRegistration(pub fn() -> Result<ConstantDef, String>);

inventory::collect!(TypeRegistration);
inventory::collect!(ErrorRegistration);
inventory::collect!(FunctionRegistration);
inventory::collect!(ResourceRegistration);
inventory::collect!(ConstantRegistration);

/// An invalid collection of linked application exports.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("duplicate {kind} name `{name}` in namespace `{namespace}`")]
    DuplicateName {
        kind: &'static str,
        name: String,
        namespace: String,
    },
    #[error("duplicate {kind} identity `{identity}`")]
    DuplicateIdentity {
        kind: &'static str,
        identity: DefinitionId,
    },
    #[error("type `{identity}` is used but is not linked into the aggregate binding")]
    MissingType { identity: DefinitionId },
    #[error("error `{identity}` is used but is not linked into the aggregate binding")]
    MissingError { identity: DefinitionId },
    #[error("invalid exported constant: {0}")]
    InvalidConstant(String),
}

/// Collect and validate every export linked into an application binding.
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
        "function",
        functions.iter().map(|item| item.host_name.as_str()),
    )?;
    unique_global_names("resource", resources.iter().map(|item| item.name.as_str()))?;

    let type_ids = unique_ids("type", types.iter().map(TypeDef::identity))?;
    let error_ids = unique_ids("error", errors.iter().map(ErrorDef::identity))?;
    for reference in all_type_refs(&types, &functions, &resources, &constants) {
        validate_ref(reference, &type_ids)?;
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
        ir_version: IR_VERSION,
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
    Ok(manifest)
}

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
                namespace: "the aggregate binding".to_owned(),
            });
        }
    }
    Ok(())
}

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
    use super::*;

    fn empty_manifest() -> Manifest {
        Manifest {
            ir_version: IR_VERSION,
            package_name: "app".to_owned(),
            package_version: "1.2.3".to_owned(),
            module_name: "native".to_owned(),
            types: Vec::new(),
            errors: Vec::new(),
            functions: Vec::new(),
            resources: Vec::new(),
            constants: Vec::new(),
        }
    }

    #[test]
    fn rejects_duplicate_public_names() {
        let function = unique_global_names("function", ["run", "run"].into_iter()).unwrap_err();
        let resource =
            unique_global_names("resource", ["Client", "Client"].into_iter()).unwrap_err();

        assert!(
            function
                .to_string()
                .contains("duplicate function name `run`")
        );
        assert!(
            resource
                .to_string()
                .contains("duplicate resource name `Client`")
        );
    }

    #[test]
    fn permits_equal_leaf_names_in_separate_namespaces() {
        let manifest = empty_manifest();
        let first = CargoPackageId::new("app-first");
        let second = CargoPackageId::new("app-second");

        unique_scoped_names(
            &manifest,
            "type",
            [
                ("Result", &first, "app_first::model"),
                ("Result", &second, "app_second::model"),
            ]
            .into_iter(),
        )
        .expect("separate namespaces can reuse a type name");
    }

    #[test]
    fn rejects_equal_leaf_names_in_one_namespace() {
        let manifest = empty_manifest();
        let owner = CargoPackageId::new("app-domain");

        let error = unique_scoped_names(
            &manifest,
            "type",
            [
                ("Result", &owner, "app_domain::model"),
                ("Result", &owner, "app_domain::model"),
            ]
            .into_iter(),
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "duplicate type name `Result` in namespace `domain::model`"
        );
    }
}
