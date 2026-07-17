use std::collections::{BTreeMap, BTreeSet};

use rspyts::ir::{DefinitionId, ErrorDef, Manifest, TypeDef, TypeRef, TypeShape};

use crate::resolve::ResolvedContract;

pub type TypeNames = BTreeMap<DefinitionId, String>;

pub fn type_names(contract: &ResolvedContract) -> TypeNames {
    contract
        .manifest
        .types
        .iter()
        .map(|item| {
            (
                DefinitionId {
                    owner: item.owner.clone(),
                    id: item.id.clone(),
                },
                item.name.clone(),
            )
        })
        .chain(
            contract
                .foreign_types
                .iter()
                .map(|(identity, item)| (identity.clone(), item.name.clone())),
        )
        .collect()
}

pub fn type_definition<'a>(
    contract: &'a ResolvedContract,
    identity: &DefinitionId,
) -> Option<&'a TypeDef> {
    contract
        .manifest
        .types
        .iter()
        .find(|item| item.owner == identity.owner && item.id == identity.id)
        .or_else(|| contract.foreign_types.get(identity))
}

pub fn error_definition<'a>(
    contract: &'a ResolvedContract,
    identity: &DefinitionId,
) -> Option<&'a ErrorDef> {
    contract
        .manifest
        .errors
        .iter()
        .find(|item| item.owner == identity.owner && item.id == identity.id)
        .or_else(|| contract.foreign_errors.get(identity))
}

pub fn ordered_types(manifest: &Manifest) -> Vec<&TypeDef> {
    let by_id = manifest
        .types
        .iter()
        .map(|item| {
            (
                DefinitionId {
                    owner: item.owner.clone(),
                    id: item.id.clone(),
                },
                item,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut visited = BTreeSet::new();
    let mut result = Vec::new();
    for id in by_id.keys() {
        visit(id, &by_id, &mut visited, &mut result);
    }
    result
}

fn visit<'a>(
    id: &DefinitionId,
    by_id: &BTreeMap<DefinitionId, &'a TypeDef>,
    visited: &mut BTreeSet<DefinitionId>,
    result: &mut Vec<&'a TypeDef>,
) {
    if !visited.insert(id.clone()) {
        return;
    }
    let Some(item) = by_id.get(id) else {
        return;
    };
    let mut dependencies = BTreeSet::new();
    match &item.shape {
        TypeShape::Struct { fields } => {
            for field in fields {
                collect(&field.ty, &mut dependencies);
            }
        }
        TypeShape::StringEnum { variants } | TypeShape::TaggedEnum { variants, .. } => {
            for variant in variants {
                for field in &variant.fields {
                    collect(&field.ty, &mut dependencies);
                }
            }
        }
        TypeShape::Alias { target } => collect(target, &mut dependencies),
    }
    for dependency in dependencies {
        visit(&dependency, by_id, visited, result);
    }
    result.push(item);
}

fn collect(reference: &TypeRef, output: &mut BTreeSet<DefinitionId>) {
    match reference {
        TypeRef::Named { identity } => {
            output.insert(identity.clone());
        }
        TypeRef::Option { item } | TypeRef::List { item } => collect(item, output),
        TypeRef::Map { value } => collect(value, output),
        TypeRef::Tuple { items } => {
            for item in items {
                collect(item, output);
            }
        }
        _ => {}
    }
}

pub fn pascal_case(value: &str) -> String {
    let mut result = String::new();
    let mut uppercase = true;
    for character in value.chars() {
        if !character.is_ascii_alphanumeric() {
            uppercase = true;
        } else if uppercase {
            result.extend(character.to_uppercase());
            uppercase = false;
        } else {
            result.push(character);
        }
    }
    result
}

pub fn python_doc(docs: Option<&str>, indent: &str) -> String {
    docs.map(|docs| {
        let escaped = docs.replace("\\", "\\\\").replace("\"\"\"", "\\\"\\\"\\\"");
        format!("{indent}\"\"\"{escaped}\"\"\"\n")
    })
    .unwrap_or_default()
}

pub fn ts_doc(docs: Option<&str>) -> String {
    docs.map(|docs| {
        let body = docs.replace("*/", "* /").replace('\n', "\n * ");
        format!("/** {body} */\n")
    })
    .unwrap_or_default()
}

pub fn ts_property(name: &str) -> String {
    if common_identifier(name) {
        name.to_owned()
    } else {
        serde_json::to_string(name).expect("a string is serializable")
    }
}

fn common_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}
