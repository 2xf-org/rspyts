use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use rspyts::ir::{
    BufferElement, ConstantDef, DefinitionId, ErrorDef, FunctionDef, Manifest, Namespace,
    ResourceDef, TypeDef, TypeRef, TypeShape,
};

#[derive(Default)]
pub(super) struct NamespaceItems<'a> {
    pub(super) types: Vec<&'a TypeDef>,
    pub(super) errors: Vec<&'a ErrorDef>,
    pub(super) functions: Vec<&'a FunctionDef>,
    pub(super) resources: Vec<&'a ResourceDef>,
    pub(super) constants: Vec<&'a ConstantDef>,
}

pub(super) fn namespaces(manifest: &Manifest) -> BTreeMap<Namespace, NamespaceItems<'_>> {
    let mut result = BTreeMap::<Namespace, NamespaceItems<'_>>::new();
    result.entry(Namespace::root()).or_default();
    for item in &manifest.types {
        result
            .entry(manifest.namespace(&item.owner, &item.rust_module))
            .or_default()
            .types
            .push(item);
    }
    for item in &manifest.errors {
        result
            .entry(manifest.namespace(&item.owner, &item.rust_module))
            .or_default()
            .errors
            .push(item);
    }
    for item in &manifest.functions {
        result
            .entry(manifest.namespace(&item.owner, &item.rust_module))
            .or_default()
            .functions
            .push(item);
    }
    for item in &manifest.resources {
        result
            .entry(manifest.namespace(&item.owner, &item.rust_module))
            .or_default()
            .resources
            .push(item);
    }
    for item in &manifest.constants {
        result
            .entry(manifest.namespace(&item.owner, &item.rust_module))
            .or_default()
            .constants
            .push(item);
    }
    let declared = result.keys().cloned().collect::<Vec<_>>();
    for namespace in declared {
        if namespace.package.is_some() {
            result
                .entry(Namespace {
                    package: namespace.package.clone(),
                    modules: Vec::new(),
                })
                .or_default();
        }
        for length in 1..namespace.modules.len() {
            result
                .entry(Namespace {
                    package: namespace.package.clone(),
                    modules: namespace.modules[..length].to_vec(),
                })
                .or_default();
        }
    }
    result
}

pub(super) fn tagged_variant_name(type_name: &str, variant_name: &str) -> String {
    format!("{type_name}{variant_name}")
}

pub(super) fn type_namespace(identity: &DefinitionId, manifest: &Manifest) -> Result<Namespace> {
    let definition = type_definition(identity, manifest)?;
    Ok(manifest.namespace(&definition.owner, &definition.rust_module))
}

pub(super) fn type_definition<'a>(
    identity: &DefinitionId,
    manifest: &'a Manifest,
) -> Result<&'a TypeDef> {
    manifest
        .types
        .iter()
        .find(|item| item.owner == identity.owner && item.id == identity.id)
        .with_context(|| format!("missing type `{identity}`"))
}

pub(super) fn error_definition<'a>(
    identity: &DefinitionId,
    manifest: &'a Manifest,
) -> Result<&'a ErrorDef> {
    manifest
        .errors
        .iter()
        .find(|item| item.owner == identity.owner && item.id == identity.id)
        .with_context(|| format!("missing error `{identity}`"))
}

pub(super) fn definition_key(identity: &DefinitionId) -> String {
    format!("{}::{}", identity.owner, identity.id)
}

pub(super) fn buffer_elements(manifest: &Manifest) -> BTreeSet<BufferElement> {
    let mut result = BTreeSet::new();
    for reference in contract_refs(manifest) {
        collect_buffers(reference, &mut result);
    }
    result
}

pub(super) fn uses_buffer(manifest: &Manifest) -> bool {
    !buffer_elements(manifest).is_empty()
}

pub(super) fn collect_buffers(reference: &TypeRef, result: &mut BTreeSet<BufferElement>) {
    match reference {
        TypeRef::Buffer { element } => {
            result.insert(*element);
        }
        TypeRef::Option { item } | TypeRef::List { item } => collect_buffers(item, result),
        TypeRef::Map { value } => collect_buffers(value, result),
        TypeRef::Tuple { items } => {
            for item in items {
                collect_buffers(item, result);
            }
        }
        _ => {}
    }
}

pub(super) fn contract_refs(manifest: &Manifest) -> Vec<&TypeRef> {
    let mut result = Vec::new();
    for definition in &manifest.types {
        match &definition.shape {
            TypeShape::Struct { fields } => result.extend(fields.iter().map(|field| &field.ty)),
            TypeShape::TaggedEnum { variants, .. } => result.extend(
                variants
                    .iter()
                    .flat_map(|variant| variant.fields.iter().map(|field| &field.ty)),
            ),
            TypeShape::Alias { target } => result.push(target),
            TypeShape::StringEnum { .. } => {}
        }
    }
    for function in &manifest.functions {
        result.extend(function.params.iter().map(|param| &param.ty));
        result.push(&function.returns);
    }
    for resource in &manifest.resources {
        for constructor in &resource.constructors {
            result.extend(constructor.params.iter().map(|param| &param.ty));
        }
        for method in &resource.methods {
            result.extend(method.params.iter().map(|param| &param.ty));
            result.push(&method.returns);
        }
    }
    result.extend(manifest.constants.iter().map(|constant| &constant.ty));
    result
}

pub(super) fn namespace_refs<'a>(items: &'a NamespaceItems<'a>) -> Vec<&'a TypeRef> {
    let mut result = Vec::new();
    for definition in &items.types {
        result.extend(type_refs(definition));
    }
    for function in &items.functions {
        result.extend(function.params.iter().map(|param| &param.ty));
        result.push(&function.returns);
    }
    for resource in &items.resources {
        for constructor in &resource.constructors {
            result.extend(constructor.params.iter().map(|param| &param.ty));
        }
        for method in &resource.methods {
            result.extend(method.params.iter().map(|param| &param.ty));
            result.push(&method.returns);
        }
    }
    result.extend(items.constants.iter().map(|constant| &constant.ty));
    result
}

pub(super) fn type_refs(definition: &TypeDef) -> Vec<&TypeRef> {
    match &definition.shape {
        TypeShape::Struct { fields } => fields.iter().map(|field| &field.ty).collect(),
        TypeShape::TaggedEnum { variants, .. } => variants
            .iter()
            .flat_map(|variant| variant.fields.iter().map(|field| &field.ty))
            .collect(),
        TypeShape::Alias { target } => vec![target],
        TypeShape::StringEnum { .. } => Vec::new(),
    }
}

pub(super) fn named_identities<'a>(reference: &'a TypeRef, result: &mut Vec<&'a DefinitionId>) {
    match reference {
        TypeRef::Named { identity } => result.push(identity),
        TypeRef::Option { item } | TypeRef::List { item } => named_identities(item, result),
        TypeRef::Map { value } => named_identities(value, result),
        TypeRef::Tuple { items } => {
            for item in items {
                named_identities(item, result);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use rspyts::ir::{CargoPackageId, FunctionDef, IR_VERSION, Manifest, Namespace, TypeRef};

    use super::namespaces;

    #[test]
    fn includes_empty_parent_namespaces_without_flattening_children() {
        let manifest = Manifest {
            ir_version: IR_VERSION,
            package_name: "example".to_owned(),
            package_version: "1.0.1".to_owned(),
            module_name: "native".to_owned(),
            types: Vec::new(),
            errors: Vec::new(),
            functions: vec![FunctionDef {
                owner: CargoPackageId::new("example-dice"),
                rust_module: "example_dice::fair::deep::roll".to_owned(),
                rust_name: "roll".to_owned(),
                host_name: "roll".to_owned(),
                docs: None,
                params: Vec::new(),
                returns: TypeRef::Unit,
                error: None,
            }],
            resources: Vec::new(),
            constants: Vec::new(),
        };

        let views = namespaces(&manifest);
        for namespace in [
            Namespace::root(),
            Namespace {
                package: Some("dice".to_owned()),
                modules: Vec::new(),
            },
            Namespace {
                package: Some("dice".to_owned()),
                modules: vec!["fair".to_owned()],
            },
            Namespace {
                package: Some("dice".to_owned()),
                modules: vec!["fair".to_owned(), "deep".to_owned()],
            },
        ] {
            let items = views.get(&namespace).expect("parent namespace exists");
            assert!(items.functions.is_empty());
        }
        let leaf = Namespace {
            package: Some("dice".to_owned()),
            modules: vec!["fair".to_owned(), "deep".to_owned(), "roll".to_owned()],
        };
        assert_eq!(views[&leaf].functions[0].host_name, "roll");
    }
}
