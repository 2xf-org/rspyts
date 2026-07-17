use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::ir::{
    CargoPackageId, ConstantDef, DefinitionId, ErrorDef, FunctionDef, IR_VERSION, ImportedPackage,
    Manifest, ResourceDef, TypeDef, TypeRef, TypeShape,
};

pub struct TypeRegistration(pub fn() -> TypeDef);
pub struct ErrorRegistration(pub fn() -> ErrorDef);
pub struct FunctionRegistration(pub fn() -> FunctionDef);
pub struct ResourceRegistration(pub fn() -> ResourceDef);
pub struct ConstantRegistration(pub fn() -> ConstantDef);

inventory::collect!(TypeRegistration);
inventory::collect!(ErrorRegistration);
inventory::collect!(FunctionRegistration);
inventory::collect!(ResourceRegistration);
inventory::collect!(ConstantRegistration);

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("duplicate {kind} identity `{identity}`")]
    Duplicate {
        kind: &'static str,
        identity: String,
    },
    #[error("{kind} `{identity}` is referenced but was not linked into the contract")]
    MissingDefinition {
        kind: &'static str,
        identity: DefinitionId,
    },
    #[error(
        "resource `{resource}` owns a nested function registered to `{actual}` instead of `{expected}`"
    )]
    NestedOwner {
        resource: String,
        expected: CargoPackageId,
        actual: CargoPackageId,
    },
}

pub fn manifest(
    crate_name: &str,
    crate_version: &str,
    module_name: &str,
) -> Result<Manifest, RegistryError> {
    resolve_manifest(
        CargoPackageId::new(crate_name),
        crate_version,
        module_name,
        type_definitions()?,
        error_definitions()?,
        inventory::iter::<FunctionRegistration>
            .into_iter()
            .map(|registration| (registration.0)())
            .collect(),
        inventory::iter::<ResourceRegistration>
            .into_iter()
            .map(|registration| (registration.0)())
            .collect(),
        inventory::iter::<ConstantRegistration>
            .into_iter()
            .map(|registration| (registration.0)())
            .collect(),
    )
}

/// Returns the complete, deterministic type graph linked into the consumer.
///
/// Generated boundary wrappers need dependency-owned definitions at runtime to
/// apply nested buffer, byte, integer-width, and enum policies. The public
/// manifest still emits only root-owned definitions.
pub fn type_definitions() -> Result<Vec<TypeDef>, RegistryError> {
    let mut types = inventory::iter::<TypeRegistration>
        .into_iter()
        .map(|registration| (registration.0)())
        .collect::<Vec<_>>();
    types.sort_by(|left, right| {
        definition_key(&left.owner, &left.id).cmp(&definition_key(&right.owner, &right.id))
    });
    unique_definitions(
        "type",
        types
            .iter()
            .map(|item| DefinitionId::new(item.owner.0.clone(), item.id.clone())),
    )?;
    Ok(types)
}

fn error_definitions() -> Result<Vec<ErrorDef>, RegistryError> {
    let mut errors = inventory::iter::<ErrorRegistration>
        .into_iter()
        .map(|registration| (registration.0)())
        .collect::<Vec<_>>();
    errors.sort_by(|left, right| {
        definition_key(&left.owner, &left.id).cmp(&definition_key(&right.owner, &right.id))
    });
    unique_definitions(
        "error",
        errors
            .iter()
            .map(|item| DefinitionId::new(item.owner.0.clone(), item.id.clone())),
    )?;
    Ok(errors)
}

#[allow(clippy::too_many_arguments)]
fn resolve_manifest(
    root: CargoPackageId,
    crate_version: &str,
    module_name: &str,
    all_types: Vec<TypeDef>,
    all_errors: Vec<ErrorDef>,
    all_functions: Vec<FunctionDef>,
    all_resources: Vec<ResourceDef>,
    all_constants: Vec<ConstantDef>,
) -> Result<Manifest, RegistryError> {
    let type_by_identity = all_types
        .iter()
        .map(|definition| {
            (
                identity(&definition.owner, &definition.id),
                definition.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let error_by_identity = all_errors
        .iter()
        .map(|definition| {
            (
                identity(&definition.owner, &definition.id),
                definition.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let resource_identities = all_resources
        .iter()
        .map(|definition| identity(&definition.owner, &definition.id))
        .collect::<BTreeSet<_>>();

    let mut types = all_types
        .into_iter()
        .filter(|definition| definition.owner == root)
        .collect::<Vec<_>>();
    let mut errors = all_errors
        .into_iter()
        .filter(|definition| definition.owner == root)
        .collect::<Vec<_>>();
    let mut functions = all_functions
        .into_iter()
        .filter(|definition| definition.owner == root)
        .collect::<Vec<_>>();
    let mut resources = all_resources
        .into_iter()
        .filter(|definition| definition.owner == root)
        .collect::<Vec<_>>();
    let mut constants = all_constants
        .into_iter()
        .filter(|definition| definition.owner == root)
        .collect::<Vec<_>>();

    types.sort_by(|left, right| left.id.cmp(&right.id));
    errors.sort_by(|left, right| left.id.cmp(&right.id));
    functions.sort_by(|left, right| left.host_name.cmp(&right.host_name));
    resources.sort_by(|left, right| left.id.cmp(&right.id));
    constants.sort_by(|left, right| left.host_name.cmp(&right.host_name));

    unique_strings("type", types.iter().map(|item| item.id.as_str()))?;
    unique_strings("error", errors.iter().map(|item| item.id.as_str()))?;
    unique_strings(
        "function",
        functions.iter().map(|item| item.host_name.as_str()),
    )?;
    unique_strings("resource", resources.iter().map(|item| item.id.as_str()))?;
    unique_strings(
        "constant",
        constants.iter().map(|item| item.host_name.as_str()),
    )?;

    for resource in &resources {
        for constructor in &resource.constructors {
            validate_nested_owner(resource, &root, &constructor.owner)?;
        }
    }

    let imports = collect_imports(
        &root,
        &types,
        &errors,
        &functions,
        &resources,
        &constants,
        &type_by_identity,
        &error_by_identity,
        &resource_identities,
    )?;

    Ok(Manifest {
        ir_version: IR_VERSION,
        crate_name: root.0,
        crate_version: crate_version.to_owned(),
        module_name: module_name.to_owned(),
        imports,
        types,
        errors,
        functions,
        resources,
        constants,
    })
}

#[allow(clippy::too_many_arguments)]
fn collect_imports(
    root: &CargoPackageId,
    types: &[TypeDef],
    errors: &[ErrorDef],
    functions: &[FunctionDef],
    resources: &[ResourceDef],
    constants: &[ConstantDef],
    type_by_identity: &BTreeMap<DefinitionId, TypeDef>,
    error_by_identity: &BTreeMap<DefinitionId, ErrorDef>,
    resource_identities: &BTreeSet<DefinitionId>,
) -> Result<Vec<ImportedPackage>, RegistryError> {
    let mut pending_types = VecDeque::new();
    let mut pending_errors = VecDeque::new();

    for definition in types {
        enqueue_shape(&definition.shape, &mut pending_types);
    }
    for definition in errors {
        for variant in &definition.variants {
            enqueue_fields(&variant.fields, &mut pending_types);
        }
    }
    for function in functions {
        enqueue_function(function, &mut pending_types, &mut pending_errors);
    }
    for resource in resources {
        for constructor in &resource.constructors {
            enqueue_function(constructor, &mut pending_types, &mut pending_errors);
        }
        for method in &resource.methods {
            for parameter in &method.params {
                enqueue_type(&parameter.ty, &mut pending_types);
            }
            enqueue_type(&method.returns, &mut pending_types);
            if let Some(error) = &method.error {
                pending_errors.push_back(error.clone());
            }
        }
    }
    for constant in constants {
        enqueue_type(&constant.ty, &mut pending_types);
    }

    let mut visited_types = BTreeSet::new();
    let mut visited_errors = BTreeSet::new();
    let mut imported_types = BTreeMap::<CargoPackageId, BTreeMap<String, TypeDef>>::new();
    let mut imported_errors = BTreeMap::<CargoPackageId, BTreeMap<String, ErrorDef>>::new();

    while let Some(identity) = pending_types.pop_front() {
        if !visited_types.insert(identity.clone()) {
            continue;
        }
        if resource_identities.contains(&identity) {
            continue;
        }
        let definition =
            type_by_identity
                .get(&identity)
                .ok_or_else(|| RegistryError::MissingDefinition {
                    kind: "type",
                    identity: identity.clone(),
                })?;
        if identity.owner != *root {
            imported_types
                .entry(identity.owner.clone())
                .or_default()
                .insert(identity.id.clone(), definition.clone());
        }
        enqueue_shape(&definition.shape, &mut pending_types);
    }

    while let Some(identity) = pending_errors.pop_front() {
        if !visited_errors.insert(identity.clone()) {
            continue;
        }
        let definition =
            error_by_identity
                .get(&identity)
                .ok_or_else(|| RegistryError::MissingDefinition {
                    kind: "error",
                    identity: identity.clone(),
                })?;
        if identity.owner != *root {
            imported_errors
                .entry(identity.owner.clone())
                .or_default()
                .insert(identity.id.clone(), definition.clone());
        }
        for variant in &definition.variants {
            enqueue_fields(&variant.fields, &mut pending_types);
        }
    }

    // Error payloads may add named types after the first type traversal.
    while let Some(identity) = pending_types.pop_front() {
        if !visited_types.insert(identity.clone()) {
            continue;
        }
        let definition =
            type_by_identity
                .get(&identity)
                .ok_or_else(|| RegistryError::MissingDefinition {
                    kind: "type",
                    identity: identity.clone(),
                })?;
        if identity.owner != *root {
            imported_types
                .entry(identity.owner.clone())
                .or_default()
                .insert(identity.id.clone(), definition.clone());
        }
        enqueue_shape(&definition.shape, &mut pending_types);
    }

    let owners = imported_types
        .keys()
        .chain(imported_errors.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    Ok(owners
        .into_iter()
        .map(|owner| ImportedPackage {
            types: imported_types
                .remove(&owner)
                .unwrap_or_default()
                .into_values()
                .collect(),
            errors: imported_errors
                .remove(&owner)
                .unwrap_or_default()
                .into_values()
                .collect(),
            owner,
        })
        .collect())
}

fn enqueue_function(
    function: &FunctionDef,
    types: &mut VecDeque<DefinitionId>,
    errors: &mut VecDeque<DefinitionId>,
) {
    for parameter in &function.params {
        enqueue_type(&parameter.ty, types);
    }
    enqueue_type(&function.returns, types);
    if let Some(error) = &function.error {
        errors.push_back(error.clone());
    }
}

fn enqueue_shape(shape: &TypeShape, pending: &mut VecDeque<DefinitionId>) {
    match shape {
        TypeShape::Struct { fields } => enqueue_fields(fields, pending),
        TypeShape::StringEnum { variants } | TypeShape::TaggedEnum { variants, .. } => {
            for variant in variants {
                enqueue_fields(&variant.fields, pending);
            }
        }
        TypeShape::Alias { target } => enqueue_type(target, pending),
    }
}

fn enqueue_fields(fields: &[crate::ir::FieldDef], pending: &mut VecDeque<DefinitionId>) {
    for field in fields {
        enqueue_type(&field.ty, pending);
    }
}

fn enqueue_type(ty: &TypeRef, pending: &mut VecDeque<DefinitionId>) {
    match ty {
        TypeRef::Option { item } | TypeRef::List { item } => enqueue_type(item, pending),
        TypeRef::Map { value } => enqueue_type(value, pending),
        TypeRef::Tuple { items } => {
            for item in items {
                enqueue_type(item, pending);
            }
        }
        TypeRef::Named { identity } => pending.push_back(identity.clone()),
        TypeRef::Unit
        | TypeRef::Bool
        | TypeRef::Int { .. }
        | TypeRef::Float { .. }
        | TypeRef::String
        | TypeRef::DateTime
        | TypeRef::Json
        | TypeRef::Bytes
        | TypeRef::Buffer { .. } => {}
    }
}

fn validate_nested_owner(
    resource: &ResourceDef,
    expected: &CargoPackageId,
    actual: &CargoPackageId,
) -> Result<(), RegistryError> {
    if actual == expected {
        return Ok(());
    }
    Err(RegistryError::NestedOwner {
        resource: resource.id.clone(),
        expected: expected.clone(),
        actual: actual.clone(),
    })
}

fn identity(owner: &CargoPackageId, id: &str) -> DefinitionId {
    DefinitionId::new(owner.0.clone(), id)
}

fn definition_key<'a>(owner: &'a CargoPackageId, id: &'a str) -> (&'a str, &'a str) {
    (owner.as_str(), id)
}

fn unique_definitions(
    kind: &'static str,
    identities: impl Iterator<Item = DefinitionId>,
) -> Result<(), RegistryError> {
    let mut seen = BTreeSet::new();
    for identity in identities {
        if !seen.insert(identity.clone()) {
            return Err(RegistryError::Duplicate {
                kind,
                identity: identity.to_string(),
            });
        }
    }
    Ok(())
}

fn unique_strings<'a>(
    kind: &'static str,
    identities: impl Iterator<Item = &'a str>,
) -> Result<(), RegistryError> {
    let mut seen = BTreeSet::new();
    for identity in identities {
        if !seen.insert(identity) {
            return Err(RegistryError::Duplicate {
                kind,
                identity: identity.to_owned(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FieldDef, ParamDef, Target};

    fn owner(value: &str) -> CargoPackageId {
        CargoPackageId::new(value)
    }

    fn identity(owner: &str, id: &str) -> DefinitionId {
        DefinitionId::new(owner, id)
    }

    fn field(name: &str, ty: TypeRef) -> FieldDef {
        FieldDef {
            rust_name: name.to_owned(),
            wire_name: name.to_owned(),
            docs: None,
            ty,
            required: true,
            default: None,
            constraints: Default::default(),
        }
    }

    fn type_definition(owner_name: &str, id: &str, shape: TypeShape) -> TypeDef {
        TypeDef {
            owner: owner(owner_name),
            id: id.to_owned(),
            name: id.to_owned(),
            docs: None,
            shape,
        }
    }

    fn function(owner_name: &str, host_name: &str, parameter: TypeRef) -> FunctionDef {
        FunctionDef {
            owner: owner(owner_name),
            rust_name: host_name.to_owned(),
            host_name: host_name.to_owned(),
            docs: None,
            target: Target::Both,
            params: vec![ParamDef {
                rust_name: "value".to_owned(),
                host_name: "value".to_owned(),
                ty: parameter,
            }],
            returns: TypeRef::Unit,
            error: None,
        }
    }

    #[test]
    fn root_manifest_excludes_dependency_definitions_and_exports() {
        let catalog_signal = type_definition(
            "catalog",
            "signal::SignalDefinition",
            TypeShape::Struct {
                fields: vec![field("id", TypeRef::String)],
            },
        );
        let reports_rule = type_definition(
            "reports",
            "rule::Rule",
            TypeShape::Struct {
                fields: vec![field(
                    "signal",
                    TypeRef::Named {
                        identity: identity("catalog", "signal::SignalDefinition"),
                    },
                )],
            },
        );
        let manifest = resolve_manifest(
            owner("reports"),
            "0.1.0",
            "native",
            vec![catalog_signal.clone(), reports_rule],
            vec![],
            vec![
                function(
                    "catalog",
                    "defineSignal",
                    TypeRef::Named {
                        identity: identity("catalog", "signal::SignalDefinition"),
                    },
                ),
                function(
                    "reports",
                    "evaluate",
                    TypeRef::Named {
                        identity: identity("catalog", "signal::SignalDefinition"),
                    },
                ),
            ],
            vec![],
            vec![],
        )
        .unwrap();

        assert_eq!(
            manifest
                .types
                .iter()
                .map(|definition| definition.id.as_str())
                .collect::<Vec<_>>(),
            ["rule::Rule"]
        );
        assert_eq!(
            manifest
                .functions
                .iter()
                .map(|definition| definition.host_name.as_str())
                .collect::<Vec<_>>(),
            ["evaluate"]
        );
        assert_eq!(
            manifest.imports,
            [ImportedPackage {
                owner: owner("catalog"),
                types: vec![catalog_signal],
                errors: vec![],
            }]
        );
        assert!(!manifest.canonical_json().unwrap().contains("defineSignal"));
    }

    #[test]
    fn equal_local_ids_from_different_packages_remain_distinct() {
        let root_type = type_definition(
            "reports",
            "model::Payload",
            TypeShape::Alias {
                target: TypeRef::String,
            },
        );
        let foreign_type = type_definition(
            "catalog",
            "model::Payload",
            TypeShape::Alias {
                target: TypeRef::Int {
                    signed: false,
                    bits: 64,
                },
            },
        );
        let manifest = resolve_manifest(
            owner("reports"),
            "0.1.0",
            "native",
            vec![root_type, foreign_type.clone()],
            vec![],
            vec![function(
                "reports",
                "readForeign",
                TypeRef::Named {
                    identity: identity("catalog", "model::Payload"),
                },
            )],
            vec![],
            vec![],
        )
        .unwrap();

        assert_eq!(manifest.types[0].owner, owner("reports"));
        assert_eq!(manifest.imports[0].owner, owner("catalog"));
        assert_eq!(manifest.imports[0].types, [foreign_type]);
    }

    #[test]
    fn missing_foreign_schema_is_rejected_by_full_identity() {
        let error = resolve_manifest(
            owner("reports"),
            "0.1.0",
            "native",
            vec![],
            vec![],
            vec![function(
                "reports",
                "evaluate",
                TypeRef::Named {
                    identity: identity("catalog", "signal::Missing"),
                },
            )],
            vec![],
            vec![],
        )
        .unwrap_err();

        assert!(matches!(
            error,
            RegistryError::MissingDefinition {
                kind: "type",
                identity
            } if identity == DefinitionId::new("catalog", "signal::Missing")
        ));
    }
}
