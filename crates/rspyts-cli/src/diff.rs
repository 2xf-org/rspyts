use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use rspyts::ir::Manifest;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Compatibility {
    Breaking,
    Additive,
    NonSemantic,
}

impl fmt::Display for Compatibility {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Breaking => "breaking",
            Self::Additive => "additive",
            Self::NonSemantic => "non-semantic",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub compatibility: Compatibility,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContractDiff {
    pub changes: Vec<Change>,
}

impl ContractDiff {
    pub fn between(old: &Manifest, new: &Manifest) -> Self {
        let mut changes = Vec::new();
        compare_scalar(
            &mut changes,
            "IR version",
            &old.ir_version,
            &new.ir_version,
            Compatibility::Breaking,
        );
        compare_scalar(
            &mut changes,
            "crate name",
            &old.crate_name,
            &new.crate_name,
            Compatibility::Breaking,
        );
        compare_scalar(
            &mut changes,
            "crate version",
            &old.crate_version,
            &new.crate_version,
            Compatibility::NonSemantic,
        );
        compare_scalar(
            &mut changes,
            "module name",
            &old.module_name,
            &new.module_name,
            Compatibility::Breaking,
        );

        compare_items(
            &mut changes,
            "type",
            old.types
                .iter()
                .map(|item| (format!("{}::{}", item.owner, item.id), item)),
            new.types
                .iter()
                .map(|item| (format!("{}::{}", item.owner, item.id), item)),
        );
        compare_items(
            &mut changes,
            "error",
            old.errors
                .iter()
                .map(|item| (format!("{}::{}", item.owner, item.id), item)),
            new.errors
                .iter()
                .map(|item| (format!("{}::{}", item.owner, item.id), item)),
        );
        compare_items(
            &mut changes,
            "function",
            old.functions
                .iter()
                .map(|item| (format!("{}::{}", item.owner, item.host_name), item)),
            new.functions
                .iter()
                .map(|item| (format!("{}::{}", item.owner, item.host_name), item)),
        );
        compare_items(
            &mut changes,
            "resource",
            old.resources
                .iter()
                .map(|item| (format!("{}::{}", item.owner, item.id), item)),
            new.resources
                .iter()
                .map(|item| (format!("{}::{}", item.owner, item.id), item)),
        );
        compare_items(
            &mut changes,
            "constant",
            old.constants
                .iter()
                .map(|item| (format!("{}::{}", item.owner, item.host_name), item)),
            new.constants
                .iter()
                .map(|item| (format!("{}::{}", item.owner, item.host_name), item)),
        );
        changes.sort_by(|left, right| {
            left.compatibility
                .cmp(&right.compatibility)
                .then(left.description.cmp(&right.description))
        });
        Self { changes }
    }
}

impl fmt::Display for ContractDiff {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.changes.is_empty() {
            return formatter.write_str("no semantic changes");
        }
        for compatibility in [
            Compatibility::Breaking,
            Compatibility::Additive,
            Compatibility::NonSemantic,
        ] {
            let matching = self
                .changes
                .iter()
                .filter(|change| change.compatibility == compatibility)
                .collect::<Vec<_>>();
            if matching.is_empty() {
                continue;
            }
            writeln!(formatter, "{compatibility}:")?;
            for change in matching {
                writeln!(formatter, "  - {}", change.description)?;
            }
        }
        Ok(())
    }
}

fn compare_scalar<T: PartialEq + fmt::Debug>(
    changes: &mut Vec<Change>,
    name: &str,
    old: &T,
    new: &T,
    compatibility: Compatibility,
) {
    if old != new {
        changes.push(Change {
            compatibility,
            description: format!("changed {name} from {old:?} to {new:?}"),
        });
    }
}

fn compare_items<'a, T: serde::Serialize + 'a>(
    changes: &mut Vec<Change>,
    kind: &str,
    old: impl Iterator<Item = (String, &'a T)>,
    new: impl Iterator<Item = (String, &'a T)>,
) {
    let old = old.collect::<BTreeMap<_, _>>();
    let new = new.collect::<BTreeMap<_, _>>();
    let identities = old
        .keys()
        .chain(new.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for identity in identities {
        match (old.get(&identity), new.get(&identity)) {
            (None, Some(_)) => changes.push(Change {
                compatibility: Compatibility::Additive,
                description: format!("added {kind} `{identity}`"),
            }),
            (Some(_), None) => changes.push(Change {
                compatibility: Compatibility::Breaking,
                description: format!("removed {kind} `{identity}`"),
            }),
            (Some(old), Some(new)) => {
                let old = serde_json::to_value(old).expect("IR is serializable");
                let new = serde_json::to_value(new).expect("IR is serializable");
                compare_value(changes, &format!("{kind} `{identity}`"), &old, &new);
            }
            (None, None) => unreachable!(),
        }
    }
}

fn compare_value(changes: &mut Vec<Change>, path: &str, old: &Value, new: &Value) {
    match (old, new) {
        (Value::Object(old), Value::Object(new)) => {
            let keys = old
                .keys()
                .chain(new.keys())
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            for key in keys {
                let child = format!("{path}.{key}");
                match (old.get(key), new.get(key)) {
                    (Some(old), Some(new)) => compare_value(changes, &child, old, new),
                    (None, Some(value)) => changes.push(Change {
                        compatibility: classify_added(&child, value),
                        description: format!("added {child}"),
                    }),
                    (Some(value), None) => changes.push(Change {
                        compatibility: classify_removed(&child, value),
                        description: format!("removed {child}"),
                    }),
                    (None, None) => unreachable!(),
                }
            }
        }
        (Value::Array(old), Value::Array(new)) => {
            let shared = old.len().min(new.len());
            for index in 0..shared {
                compare_value(
                    changes,
                    &format!("{path}[{index}]"),
                    &old[index],
                    &new[index],
                );
            }
            for (index, value) in new.iter().enumerate().skip(shared) {
                let child = format!("{path}[{index}]");
                changes.push(Change {
                    compatibility: classify_added(&child, value),
                    description: format!("added {child}"),
                });
            }
            for index in shared..old.len() {
                changes.push(Change {
                    compatibility: Compatibility::Breaking,
                    description: format!("removed {path}[{index}]"),
                });
            }
        }
        _ if old != new => changes.push(Change {
            compatibility: classify_changed(path, old, new),
            description: format!("changed {path}"),
        }),
        _ => {}
    }
}

fn classify_added(path: &str, value: &Value) -> Compatibility {
    if path.ends_with(".docs") {
        Compatibility::NonSemantic
    } else if path.contains(".fields[")
        && value
            .as_object()
            .and_then(|field| field.get("required"))
            .and_then(Value::as_bool)
            == Some(false)
    {
        Compatibility::Additive
    } else {
        Compatibility::Breaking
    }
}

fn classify_removed(path: &str, _value: &Value) -> Compatibility {
    if path.ends_with(".docs") {
        Compatibility::NonSemantic
    } else {
        Compatibility::Breaking
    }
}

fn classify_changed(path: &str, old: &Value, new: &Value) -> Compatibility {
    if path.ends_with(".docs") {
        return Compatibility::NonSemantic;
    }
    if path.ends_with(".required") && new.as_bool() == Some(false) {
        return Compatibility::Additive;
    }
    if path.ends_with(".constraints.literal") {
        return if new.is_null() {
            Compatibility::Additive
        } else {
            Compatibility::Breaking
        };
    }
    if path.ends_with(".constraints.minLength") || path.ends_with(".constraints.ge") {
        return classify_lower_bound(old, new);
    }
    if path.ends_with(".constraints.maxLength") {
        return classify_upper_bound(old, new);
    }
    Compatibility::Breaking
}

fn classify_lower_bound(old: &Value, new: &Value) -> Compatibility {
    match (integer_value(old), integer_value(new)) {
        (_, None) if new.is_null() => Compatibility::Additive,
        (Some(old), Some(new)) if new <= old => Compatibility::Additive,
        _ => Compatibility::Breaking,
    }
}

fn integer_value(value: &Value) -> Option<i128> {
    value
        .as_i64()
        .map(i128::from)
        .or_else(|| value.as_u64().map(i128::from))
}

fn classify_upper_bound(old: &Value, new: &Value) -> Compatibility {
    match (old.as_u64(), new.as_u64()) {
        (_, None) if new.is_null() => Compatibility::Additive,
        (Some(old), Some(new)) if new >= old => Compatibility::Additive,
        _ => Compatibility::Breaking,
    }
}

#[cfg(test)]
mod tests {
    use rspyts::ir::*;

    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            ir_version: IR_VERSION,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "sample".into(),
            imports: vec![],
            types: vec![],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![],
        }
    }

    fn manifest_with_field(field: FieldDef) -> Manifest {
        let mut manifest = manifest();
        manifest.types.push(TypeDef {
            owner: CargoPackageId::new("sample"),
            id: "sample::Item".into(),
            name: "Item".into(),
            docs: None,
            shape: TypeShape::Struct {
                fields: vec![field],
            },
        });
        manifest
    }

    fn string_field() -> FieldDef {
        FieldDef {
            rust_name: "value".into(),
            wire_name: "value".into(),
            docs: None,
            ty: TypeRef::String,
            required: true,
            default: None,
            constraints: FieldConstraints::default(),
        }
    }

    #[test]
    fn classifies_top_level_additions_and_removals() {
        let old = manifest();
        let mut new = manifest();
        new.constants.push(ConstantDef {
            owner: CargoPackageId::new("sample"),
            rust_name: "VALUE".into(),
            host_name: "VALUE".into(),
            docs: None,
            target: Target::Static,
            ty: TypeRef::String,
            value: Value::String("value".into()),
        });
        let added = ContractDiff::between(&old, &new);
        assert_eq!(added.changes[0].compatibility, Compatibility::Additive);
        let removed = ContractDiff::between(&new, &old);
        assert_eq!(removed.changes[0].compatibility, Compatibility::Breaking);
    }

    #[test]
    fn documentation_is_non_semantic() {
        let mut old = manifest();
        old.types.push(TypeDef {
            owner: CargoPackageId::new("sample"),
            id: "sample::Item".into(),
            name: "Item".into(),
            docs: None,
            shape: TypeShape::Struct { fields: vec![] },
        });
        let mut new = old.clone();
        new.types[0].docs = Some("Useful documentation".into());
        assert_eq!(
            ContractDiff::between(&old, &new).changes[0].compatibility,
            Compatibility::NonSemantic
        );
    }

    #[test]
    fn constraint_relaxation_is_additive_and_tightening_is_breaking() {
        let mut old_field = string_field();
        old_field.constraints.min_length = Some(2);
        old_field.constraints.max_length = Some(10);
        let old = manifest_with_field(old_field);

        let mut relaxed_field = string_field();
        relaxed_field.constraints.min_length = Some(1);
        relaxed_field.constraints.max_length = Some(12);
        let relaxed = ContractDiff::between(&old, &manifest_with_field(relaxed_field));
        assert!(
            relaxed
                .changes
                .iter()
                .all(|change| change.compatibility == Compatibility::Additive)
        );

        let mut tightened_field = string_field();
        tightened_field.constraints.min_length = Some(3);
        tightened_field.constraints.max_length = Some(8);
        let tightened = ContractDiff::between(&old, &manifest_with_field(tightened_field));
        assert!(
            tightened
                .changes
                .iter()
                .all(|change| change.compatibility == Compatibility::Breaking)
        );
    }

    #[test]
    fn removing_a_literal_is_additive_but_changing_a_default_is_breaking() {
        let mut literal_field = string_field();
        literal_field.constraints.literal = Some(ScalarValue::String("v1".into()));
        let removed = ContractDiff::between(
            &manifest_with_field(literal_field),
            &manifest_with_field(string_field()),
        );
        assert_eq!(removed.changes[0].compatibility, Compatibility::Additive);

        let mut defaulted = string_field();
        defaulted.required = false;
        defaulted.default = Some(ScalarValue::String("v1".into()));
        let changed = ContractDiff::between(
            &manifest_with_field(string_field()),
            &manifest_with_field(defaulted),
        );
        assert!(
            changed
                .changes
                .iter()
                .any(|change| change.compatibility == Compatibility::Breaking)
        );
    }

    #[test]
    fn changing_to_datetime_is_breaking() {
        let old = manifest_with_field(string_field());
        let mut datetime = string_field();
        datetime.ty = TypeRef::DateTime;
        assert_eq!(
            ContractDiff::between(&old, &manifest_with_field(datetime)).changes[0].compatibility,
            Compatibility::Breaking
        );
    }
}
