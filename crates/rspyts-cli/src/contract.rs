//! Read-only indexing and traversal helpers for the linked contract IR.
//!
//! Renderers consume declarations by generated namespace rather than by Rust
//! package. Helpers in this module centralize that indexing and recursively
//! resolve references when a decision depends on an alias's underlying type.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use rspyts::ir::{
    BufferElement, ConstantDef, DefinitionId, ErrorDef, FunctionDef, Manifest, Namespace,
    ResourceDef, TypeDef, TypeRef, TypeShape,
};

/// Borrowed declarations grouped into one generated namespace.
#[derive(Default)]
pub(super) struct NamespaceItems<'a> {
    /// Model declarations in this namespace.
    pub(super) types: Vec<&'a TypeDef>,
    /// Typed-error declarations in this namespace.
    pub(super) errors: Vec<&'a ErrorDef>,
    /// Free functions in this namespace.
    pub(super) functions: Vec<&'a FunctionDef>,
    /// Stateful resources in this namespace.
    pub(super) resources: Vec<&'a ResourceDef>,
    /// Constants in this namespace.
    pub(super) constants: Vec<&'a ConstantDef>,
}

/// Index every declaration and required ancestor package by namespace.
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

/// Return the generated class/interface name for a tagged-enum variant.
pub(super) fn tagged_variant_name(type_name: &str, variant_name: &str) -> String {
    format!("{type_name}{variant_name}")
}

/// Resolve a named type identity to its generated namespace.
pub(super) fn type_namespace(identity: &DefinitionId, manifest: &Manifest) -> Result<Namespace> {
    let definition = type_definition(identity, manifest)?;
    Ok(manifest.namespace(&definition.owner, &definition.rust_module))
}

/// Resolve a named type identity to its linked definition.
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

/// Resolve a named error identity to its linked definition.
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

/// Return the canonical runtime-schema key for a named definition.
pub(super) fn definition_key(identity: &DefinitionId) -> String {
    format!("{}::{}", identity.owner, identity.id)
}

/// Collect every numeric buffer element used anywhere in the manifest.
pub(super) fn buffer_elements(manifest: &Manifest) -> BTreeSet<BufferElement> {
    let mut result = BTreeSet::new();
    for reference in contract_refs(manifest) {
        collect_buffers_resolved(reference, manifest, &mut result)
            .expect("linked contract references were validated during discovery");
    }
    result
}

/// Collect buffers after recursively resolving named aliases.
pub(super) fn collect_buffers_resolved(
    reference: &TypeRef,
    manifest: &Manifest,
    result: &mut BTreeSet<BufferElement>,
) -> Result<()> {
    collect_buffers_resolved_inner(reference, manifest, &mut BTreeSet::new(), result)
}

fn collect_buffers_resolved_inner(
    reference: &TypeRef,
    manifest: &Manifest,
    visiting: &mut BTreeSet<DefinitionId>,
    result: &mut BTreeSet<BufferElement>,
) -> Result<()> {
    match reference {
        TypeRef::Buffer { element } => {
            result.insert(*element);
        }
        TypeRef::Option { item } | TypeRef::List { item } => {
            collect_buffers_resolved_inner(item, manifest, visiting, result)?;
        }
        TypeRef::Map { value } => {
            collect_buffers_resolved_inner(value, manifest, visiting, result)?;
        }
        TypeRef::Tuple { items } => {
            for item in items {
                collect_buffers_resolved_inner(item, manifest, visiting, result)?;
            }
        }
        TypeRef::Named { identity } if visiting.insert(identity.clone()) => {
            let definition = type_definition(identity, manifest)?;
            match &definition.shape {
                TypeShape::Struct { fields } => {
                    for field in fields {
                        collect_buffers_resolved_inner(&field.ty, manifest, visiting, result)?;
                    }
                }
                TypeShape::TaggedEnum { variants, .. } => {
                    for field in variants.iter().flat_map(|variant| &variant.fields) {
                        collect_buffers_resolved_inner(&field.ty, manifest, visiting, result)?;
                    }
                }
                TypeShape::Alias { target } => {
                    collect_buffers_resolved_inner(target, manifest, visiting, result)?;
                }
                TypeShape::StringEnum { .. } => {}
            }
            visiting.remove(identity);
        }
        _ => {}
    }
    Ok(())
}

/// Return whether the contract requires NumPy/typed-array support.
pub(super) fn uses_buffer(manifest: &Manifest) -> bool {
    !buffer_elements(manifest).is_empty()
}

/// Collect buffers directly contained by a type reference.
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

/// Return whether any node in a reference tree matches a predicate.
pub(super) fn reference_contains(
    reference: &TypeRef,
    predicate: &impl Fn(&TypeRef) -> bool,
) -> bool {
    if predicate(reference) {
        return true;
    }
    match reference {
        TypeRef::Option { item } | TypeRef::List { item } => reference_contains(item, predicate),
        TypeRef::Map { value } => reference_contains(value, predicate),
        TypeRef::Tuple { items } => items.iter().any(|item| reference_contains(item, predicate)),
        _ => false,
    }
}

/// Return every type reference reachable from the complete contract.
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

/// Return every type reference reachable from one namespace's declarations.
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

/// Return every type reference carried by one model definition.
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

/// Append all named identities nested in a type reference.
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
    use super::*;

    #[test]
    fn buffer_collection_resolves_named_aliases() {
        let definition = TypeDef {
            owner: rspyts::ir::CargoPackageId::new("test"),
            rust_module: "test".to_owned(),
            id: "test::Samples".to_owned(),
            name: "Samples".to_owned(),
            docs: None,
            shape: TypeShape::Alias {
                target: TypeRef::Buffer {
                    element: BufferElement::F64,
                },
            },
        };
        let reference = TypeRef::Named {
            identity: definition.identity(),
        };
        let manifest = Manifest {
            package_name: "test".to_owned(),
            package_version: "0.0.0".to_owned(),
            module_name: "native".to_owned(),
            types: vec![definition],
            errors: Vec::new(),
            functions: Vec::new(),
            resources: Vec::new(),
            constants: Vec::new(),
        };
        let mut buffers = BTreeSet::new();

        collect_buffers_resolved(&reference, &manifest, &mut buffers).expect("resolve named alias");

        assert_eq!(buffers, BTreeSet::from([BufferElement::F64]));
    }
}
