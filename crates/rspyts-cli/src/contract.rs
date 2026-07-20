use std::collections::BTreeSet;

use anyhow::{Context, Result};
use rspyts::ir::{BufferElement, DefinitionId, Manifest, TypeDef, TypeRef, TypeShape};

pub(super) fn tagged_variant_name(type_name: &str, variant_name: &str) -> String {
    format!("{type_name}{variant_name}")
}

pub(super) fn type_name<'a>(identity: &DefinitionId, manifest: &'a Manifest) -> Result<&'a str> {
    Ok(type_definition(identity, manifest)?.name.as_str())
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

pub(super) fn error_name<'a>(
    identity: Option<&DefinitionId>,
    manifest: &'a Manifest,
) -> Result<&'a str> {
    let identity = identity.context("missing error identity")?;
    manifest
        .errors
        .iter()
        .find(|item| item.owner == identity.owner && item.id == identity.id)
        .map(|item| item.name.as_str())
        .with_context(|| format!("missing error `{identity}`"))
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
