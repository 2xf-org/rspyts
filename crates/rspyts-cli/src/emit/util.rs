use std::collections::{BTreeMap, BTreeSet};

use rspyts::ir::{DefinitionId, EnumVariantDef, ErrorDef, Manifest, TypeDef, TypeRef, TypeShape};

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

pub fn type_allows_null(reference: &TypeRef, contract: &ResolvedContract) -> bool {
    fn resolve(
        reference: &TypeRef,
        contract: &ResolvedContract,
        visited: &mut BTreeSet<DefinitionId>,
    ) -> bool {
        match reference {
            TypeRef::Option { .. } => true,
            TypeRef::Named { identity } if visited.insert(identity.clone()) => {
                match type_definition(contract, identity).map(|definition| &definition.shape) {
                    Some(TypeShape::Alias { target }) => resolve(target, contract, visited),
                    _ => false,
                }
            }
            _ => false,
        }
    }

    resolve(reference, contract, &mut BTreeSet::new())
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

pub fn python_identifier(value: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "False", "None", "True", "and", "as", "assert", "async", "await", "break", "case", "class",
        "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global",
        "if", "import", "in", "is", "lambda", "match", "nonlocal", "not", "or", "pass", "raise",
        "return", "try", "while", "with", "yield",
    ];
    let mut characters = value.chars();
    characters.next().is_some_and(|character| {
        character.is_ascii_alphabetic()
            && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
    }) && !KEYWORDS.contains(&value)
}

pub fn python_model_field_identifier(value: &str) -> bool {
    python_identifier(value) && !value.starts_with("model_")
}

pub fn python_tag_field(tag: &str, variants: &[EnumVariantDef]) -> String {
    let field_names = variants
        .iter()
        .flat_map(|variant| &variant.fields)
        .map(|field| field.rust_name.as_str())
        .collect::<BTreeSet<_>>();
    if python_model_field_identifier(tag) && !field_names.contains(tag) {
        return tag.to_owned();
    }

    let mut candidate = "variant".to_owned();
    while field_names.contains(candidate.as_str()) {
        candidate.push_str("_tag");
    }
    candidate
}

pub fn python_tagged_variant_name(type_name: &str, variant_name: &str) -> String {
    format!("{type_name}{}", pascal_case(variant_name))
}

/// Renders a deterministic Python string literal.
///
/// `serde_json` emits double-quoted strings with escapes Python recognizes.
/// Using it here avoids Rust-only escapes such as `\u{7f}` while preserving
/// control characters, quotes, backslashes, and all Unicode scalar values
/// exactly.
pub fn python_string_literal(value: &str) -> String {
    serde_json::to_string(value).expect("a string is serializable")
}

pub fn python_doc(docs: Option<&str>, indent: &str) -> String {
    docs.map(|docs| format!("{indent}{}\n", python_string_literal(docs)))
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

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::python_string_literal;

    #[test]
    fn python_string_literals_round_trip_every_escape_class() {
        let value = "quote=\" slash=\\ controls=\0\u{1f}\u{7f} lines=\n\r\t separators=\u{2028}\u{2029} unicode=é🦀";
        let literal = python_string_literal(value);

        assert!(!literal.contains("\\u{"));
        assert!(literal.starts_with('"') && literal.ends_with('"'));

        let expected = serde_json::to_string(value).unwrap();
        let expected = python_string_literal(&expected);
        let script =
            format!("import json\nvalue = {literal}\nassert value == json.loads({expected})\n");
        let result = Command::new("python3").arg("-c").arg(script).output();
        if let Ok(result) = result {
            assert!(
                result.status.success(),
                "Python rejected the generated literal:\n{}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }
}
