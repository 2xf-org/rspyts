//! Conservative compatibility comparison for two complete manifests.
//!
//! The policy deliberately has only one safe additive unit: a whole new
//! top-level declaration. Once a declaration exists, any non-documentation
//! change to it is breaking because generated consumers may depend on its
//! exact closed wire shape.

use anyhow::{Context, Result};
use rspyts_core::ir::{Manifest, TypeDecl};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Breaking,
    Additive,
    Informational,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Breaking => "BREAKING",
            Self::Additive => "ADDITIVE",
            Self::Informational => "INFORMATIONAL",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub severity: Severity,
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub changes: Vec<Change>,
}

impl Report {
    pub fn has_breaking(&self) -> bool {
        self.changes
            .iter()
            .any(|change| change.severity == Severity::Breaking)
    }

    pub fn has_changes(&self) -> bool {
        !self.changes.is_empty()
    }

    pub fn count(&self, severity: Severity) -> usize {
        self.changes
            .iter()
            .filter(|change| change.severity == severity)
            .count()
    }

    pub fn render(&self) -> String {
        if self.changes.is_empty() {
            return "no manifest changes\n".to_string();
        }
        let mut output = String::new();
        for change in &self.changes {
            output.push_str(&format!(
                "{} {}: {}\n",
                change.severity, change.path, change.message
            ));
        }
        output.push_str(&format!(
            "summary: {} breaking, {} additive, {} informational\n",
            self.count(Severity::Breaking),
            self.count(Severity::Additive),
            self.count(Severity::Informational)
        ));
        output
    }
}

pub fn read_manifest(path: &Path) -> Result<Manifest> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("cannot read manifest `{}`", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("`{}` is not a valid rspyts manifest", path.display()))
}

pub fn compare(old: &Manifest, new: &Manifest) -> Report {
    let mut report = Report::default();

    // ABI is deliberately first even when many declarations also changed.
    if old.abi != new.abi {
        report.changes.push(Change {
            severity: Severity::Breaking,
            path: "abi".into(),
            message: format!("changed from `{}` to `{}`", old.abi, new.abi),
        });
    }
    if old.crate_name != new.crate_name {
        report.changes.push(Change {
            severity: Severity::Breaking,
            path: "crateName".into(),
            message: format!("changed from `{}` to `{}`", old.crate_name, new.crate_name),
        });
    }
    if old.crate_version != new.crate_version {
        report.changes.push(Change {
            severity: Severity::Informational,
            path: "crateVersion".into(),
            message: format!(
                "changed from `{}` to `{}`",
                old.crate_version, new.crate_version
            ),
        });
    }

    compare_declarations(&mut report, "type", &old.types, &new.types, TypeDecl::name);
    compare_declarations(
        &mut report,
        "constant",
        &old.constants,
        &new.constants,
        |decl| &decl.name,
    );
    compare_declarations(
        &mut report,
        "function",
        &old.functions,
        &new.functions,
        |decl| &decl.name,
    );
    compare_declarations(&mut report, "class", &old.classes, &new.classes, |decl| {
        &decl.name
    });

    report
}

fn compare_declarations<T, F>(report: &mut Report, kind: &str, old: &[T], new: &[T], name: F)
where
    T: Serialize,
    F: Fn(&T) -> &str,
{
    let old_by_name: BTreeMap<&str, &T> = old.iter().map(|decl| (name(decl), decl)).collect();
    let new_by_name: BTreeMap<&str, &T> = new.iter().map(|decl| (name(decl), decl)).collect();

    for (decl_name, old_decl) in &old_by_name {
        let path = format!("{kind}.{decl_name}");
        let Some(new_decl) = new_by_name.get(decl_name) else {
            report.changes.push(Change {
                severity: Severity::Breaking,
                path,
                message: "removed".into(),
            });
            continue;
        };

        let old_value = serde_json::to_value(old_decl).expect("manifest declarations serialize");
        let new_value = serde_json::to_value(new_decl).expect("manifest declarations serialize");
        if old_value == new_value {
            continue;
        }
        if without_docs(old_value) == without_docs(new_value) {
            report.changes.push(Change {
                severity: Severity::Informational,
                path,
                message: "documentation changed".into(),
            });
        } else {
            report.changes.push(Change {
                severity: Severity::Breaking,
                path,
                message: "declaration changed".into(),
            });
        }
    }

    for decl_name in new_by_name.keys() {
        if !old_by_name.contains_key(decl_name) {
            report.changes.push(Change {
                severity: Severity::Additive,
                path: format!("{kind}.{decl_name}"),
                message: "added".into(),
            });
        }
    }
}

fn without_docs(mut value: Value) -> Value {
    remove_docs(&mut value);
    value
}

fn remove_docs(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("docs");
            for child in object.values_mut() {
                remove_docs(child);
            }
        }
        Value::Array(array) => {
            for child in array {
                remove_docs(child);
            }
        }
        _ => {}
    }
}

pub fn exit_code(report: &Report, fail_on_any: bool) -> u8 {
    u8::from(report.has_breaking() || (fail_on_any && report.has_changes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest_value() -> Value {
        json!({
            "abi": "2.0",
            "crateName": "demo",
            "crateVersion": "1.0.0",
            "types": [
                {"kind": "struct", "name": "Item", "docs": "item docs", "origin": "demo", "fields": [
                    {"name": "value", "wireName": "value", "docs": "field docs", "ty": {"kind": "u32"}, "optional": false}
                ]},
                {"kind": "enum", "name": "Choice", "docs": "", "origin": "demo", "tag": "type", "variants": [
                    {"name": "One", "wireName": "one", "docs": "", "fields": []}
                ]},
                {"kind": "errorEnum", "name": "Failure", "docs": "", "origin": "demo", "variants": [
                    {"name": "Bad", "wireCode": "bad", "docs": "", "fields": []}
                ]}
            ],
            "constants": [
                {"name": "LIMIT", "docs": "", "origin": "demo", "ty": {"kind": "u32"}, "value": 3}
            ],
            "functions": [
                {"name": "run", "docs": "", "params": [
                    {"name": "item", "wireName": "item", "ty": {"kind": "ref", "name": "Item"}}
                ], "ret": {"kind": "unit"}, "err": "Failure", "targets": ["python", "typescript"]}
            ],
            "classes": [
                {"name": "Session", "docs": "", "constructor": {"docs": "", "params": [], "err": null},
                 "methods": [{"name": "close", "docs": "", "mutable": true, "params": [], "ret": {"kind": "unit"}, "err": null, "targets": ["python", "typescript"]}],
                 "statics": [{"name": "open", "docs": "", "params": [], "ret": {"kind": "unit"}, "err": null, "returnsSelf": true, "targets": ["python", "typescript"]}]}
            ]
        })
    }

    fn manifest(value: Value) -> Manifest {
        serde_json::from_value(value).unwrap()
    }

    fn severity_after(pointer: &str, replacement: Value) -> Severity {
        let old = manifest_value();
        let mut new = old.clone();
        *new.pointer_mut(pointer).unwrap() = replacement;
        compare(&manifest(old), &manifest(new)).changes[0].severity
    }

    #[test]
    fn identical_manifests_have_stable_empty_output() {
        let manifest = manifest(manifest_value());
        let report = compare(&manifest, &manifest);
        assert_eq!(report.render(), "no manifest changes\n");
        assert!(!report.has_breaking());
        assert!(!report.has_changes());
    }

    #[test]
    fn every_inside_declaration_rule_is_breaking() {
        let cases = [
            ("/types/0/fields/0/wireName", json!("wire_value")),
            ("/types/0/fields/0/ty/kind", json!("i32")),
            ("/types/0/fields/0/optional", json!(true)),
            ("/types/1/tag", json!("kind")),
            ("/types/1/variants/0/wireName", json!("first")),
            ("/types/2/variants/0/wireCode", json!("veryBad")),
            ("/constants/0/value", json!(4)),
            ("/functions/0/params/0/wireName", json!("input")),
            ("/functions/0/ret/kind", json!("bool")),
            ("/classes/0/methods/0/mutable", json!(false)),
            ("/classes/0/statics/0/returnsSelf", json!(false)),
        ];
        for (pointer, replacement) in cases {
            assert_eq!(
                severity_after(pointer, replacement),
                Severity::Breaking,
                "{pointer}"
            );
        }
    }

    #[test]
    fn added_or_removed_nested_items_are_breaking() {
        for pointer in [
            "/types/0/fields",
            "/types/1/variants",
            "/functions/0/params",
            "/classes/0/methods",
            "/classes/0/statics",
        ] {
            let old = manifest_value();
            let mut new = old.clone();
            new.pointer_mut(pointer)
                .unwrap()
                .as_array_mut()
                .unwrap()
                .clear();
            assert!(
                compare(&manifest(old), &manifest(new)).has_breaking(),
                "{pointer}"
            );
        }
    }

    #[test]
    fn only_whole_new_top_level_declarations_are_additive() {
        let old = manifest_value();
        let mut new = old.clone();
        new["types"].as_array_mut().unwrap().push(json!({
            "kind": "stringEnum", "name": "Mode", "docs": "", "origin": "demo", "variants": []
        }));
        new["constants"].as_array_mut().unwrap().push(json!({
            "name": "ENABLED", "docs": "", "origin": "demo", "ty": {"kind": "bool"}, "value": true
        }));
        new["functions"].as_array_mut().unwrap().push(json!({
            "name": "ping", "docs": "", "params": [], "ret": {"kind": "unit"}, "err": null, "targets": ["python", "typescript"]
        }));
        new["classes"].as_array_mut().unwrap().push(json!({
            "name": "Clock", "docs": "", "constructor": null, "methods": [], "statics": []
        }));
        let report = compare(&manifest(old), &manifest(new));
        assert_eq!(report.count(Severity::Additive), 4);
        assert_eq!(report.count(Severity::Breaking), 0);
        assert_eq!(
            report
                .changes
                .iter()
                .map(|change| change.path.as_str())
                .collect::<Vec<_>>(),
            [
                "type.Mode",
                "constant.ENABLED",
                "function.ping",
                "class.Clock"
            ]
        );
    }

    #[test]
    fn removals_are_breaking_and_sorted_by_name() {
        let old = manifest_value();
        let mut new = old.clone();
        new["types"] = json!([]);
        let report = compare(&manifest(old), &manifest(new));
        assert_eq!(report.count(Severity::Breaking), 3);
        assert_eq!(
            report
                .changes
                .iter()
                .map(|change| change.path.as_str())
                .collect::<Vec<_>>(),
            ["type.Choice", "type.Failure", "type.Item"]
        );
    }

    #[test]
    fn docs_are_informational_even_when_nested_and_version_is_informational() {
        let old = manifest_value();
        let mut new = old.clone();
        new["crateVersion"] = json!("1.1.0");
        new["types"][0]["fields"][0]["docs"] = json!("better field docs");
        let report = compare(&manifest(old), &manifest(new));
        assert_eq!(report.count(Severity::Informational), 2);
        assert_eq!(report.count(Severity::Breaking), 0);
    }

    #[test]
    fn abi_is_reported_first_and_crate_identity_is_breaking() {
        let old = manifest_value();
        let mut new = old.clone();
        new["abi"] = json!("3.0");
        new["crateName"] = json!("renamed");
        new["types"] = json!([]);
        let report = compare(&manifest(old), &manifest(new));
        assert_eq!(report.changes[0].path, "abi");
        assert_eq!(report.changes[1].path, "crateName");
        assert!(report.has_breaking());
    }

    #[test]
    fn fail_policy_exit_codes_are_ci_friendly() {
        let none = Report::default();
        assert_eq!(exit_code(&none, false), 0);
        assert_eq!(exit_code(&none, true), 0);
        let additive = Report {
            changes: vec![Change {
                severity: Severity::Additive,
                path: "function.ping".into(),
                message: "added".into(),
            }],
        };
        assert_eq!(exit_code(&additive, false), 0);
        assert_eq!(exit_code(&additive, true), 1);
        let breaking = Report {
            changes: vec![Change {
                severity: Severity::Breaking,
                path: "abi".into(),
                message: "changed".into(),
            }],
        };
        assert_eq!(exit_code(&breaking, false), 1);
        assert_eq!(exit_code(&breaking, true), 1);
    }

    fn exit_code(report: &Report, fail_on_any: bool) -> u8 {
        super::exit_code(report, fail_on_any)
    }
}
