//! Per-language `exclude` filtering (codegen.md §1).
//!
//! The `[python]`/`[typescript]` config sections take an optional
//! `exclude = ["glob", …]` list. Matching items are dropped from that
//! language's projection before its emitters run — exactly like target
//! filtering, the compiled shims are untouched. Patterns are simple
//! globs: `*` matches any run of characters within one dot-separated
//! segment. They match item names: functions, classes, named types, and
//! constants by name; methods and statics as `Class.method`.
//!
//! Dropping a named type that surviving items still reference would emit
//! dangling code, so that is rejected with an error naming both sides.

use anyhow::{Result, bail};
use rspyts_core::ir::{Manifest, ParamDecl, Ty, TypeDecl};
use std::collections::BTreeSet;
use std::fmt::Write as _;

/// Filter `manifest` for one language. `section` names the config
/// section (`[python]`/`[typescript]`) in error messages.
pub fn apply(manifest: &Manifest, exclude: &[String], section: &str) -> Result<Manifest> {
    if exclude.is_empty() {
        return Ok(manifest.clone());
    }
    let excluded = |name: &str| exclude.iter().any(|pattern| glob_match(pattern, name));

    let mut filtered = manifest.clone();
    let mut removed_types: BTreeSet<String> = BTreeSet::new();
    filtered.types.retain(|t| {
        let keep = !excluded(t.name());
        if !keep {
            removed_types.insert(t.name().to_string());
        }
        keep
    });
    filtered.constants.retain(|c| !excluded(&c.name));
    filtered.functions.retain(|f| !excluded(&f.name));
    filtered.classes.retain(|c| !excluded(&c.name));
    for class in &mut filtered.classes {
        let class_name = class.name.clone();
        class
            .methods
            .retain(|m| !excluded(&format!("{class_name}.{}", m.name)));
        class
            .statics
            .retain(|s| !excluded(&format!("{class_name}.{}", s.name)));
    }

    let dangling = dangling_references(&filtered, &removed_types);
    if !dangling.is_empty() {
        let mut msg = format!(
            "{section} exclude: excluded type(s) are still referenced by emitted items — \
             exclude the referencing items too, or keep the type:"
        );
        for (ty, ctx) in &dangling {
            write!(msg, "\n  - `{ty}` is excluded but {ctx} references it")
                .expect("writing to String cannot fail");
        }
        bail!(msg);
    }
    Ok(filtered)
}

/// `pattern` against `name`, both split on `.`: segment counts must
/// match, and within a segment `*` matches any run of characters.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let patterns: Vec<&str> = pattern.split('.').collect();
    let names: Vec<&str> = name.split('.').collect();
    patterns.len() == names.len()
        && patterns
            .iter()
            .zip(&names)
            .all(|(p, n)| segment_match(p.as_bytes(), n.as_bytes()))
}

fn segment_match(pattern: &[u8], name: &[u8]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some((b'*', rest)) => {
            segment_match(rest, name) || (!name.is_empty() && segment_match(pattern, &name[1..]))
        }
        Some((c, rest)) => name
            .split_first()
            .is_some_and(|(n, ns)| n == c && segment_match(rest, ns)),
    }
}

/// Accumulates `(excluded type, referencing item)` pairs.
struct Dangling<'m> {
    removed_types: &'m BTreeSet<String>,
    found: Vec<(String, String)>,
}

impl Dangling<'_> {
    fn check_ty(&mut self, ty: &Ty, ctx: &str) {
        let mut refs = BTreeSet::new();
        collect_refs(ty, &mut refs);
        for name in refs {
            if self.removed_types.contains(&name) {
                self.found.push((name, ctx.to_string()));
            }
        }
    }

    fn check_err(&mut self, err: Option<&String>, ctx: &str) {
        if let Some(name) = err {
            if self.removed_types.contains(name) {
                self.found.push((name.clone(), ctx.to_string()));
            }
        }
    }

    fn check_params(&mut self, params: &[ParamDecl], ctx: &str) {
        for p in params {
            self.check_ty(&p.ty, ctx);
        }
    }
}

/// Every `(excluded type, referencing item)` pair left in `manifest`.
fn dangling_references(
    manifest: &Manifest,
    removed_types: &BTreeSet<String>,
) -> Vec<(String, String)> {
    let mut d = Dangling {
        removed_types,
        found: Vec::new(),
    };

    for t in &manifest.types {
        let fields: Vec<&rspyts_core::ir::FieldDecl> = match t {
            TypeDecl::Newtype { inner, .. } => {
                d.check_ty(inner, &format!("newtype `{}` inner type", t.name()));
                Vec::new()
            }
            TypeDecl::Struct { fields, .. } => fields.iter().collect(),
            TypeDecl::Enum { variants, .. } => {
                variants.iter().flat_map(|v| v.fields.iter()).collect()
            }
            TypeDecl::ErrorEnum { variants, .. } => {
                variants.iter().flat_map(|v| v.fields.iter()).collect()
            }
            TypeDecl::StringEnum { .. } => Vec::new(),
        };
        for f in fields {
            d.check_ty(&f.ty, &format!("type `{}` field `{}`", t.name(), f.name));
        }
    }
    for c in &manifest.constants {
        d.check_ty(&c.ty, &format!("constant `{}`", c.name));
    }
    for f in &manifest.functions {
        let ctx = format!("function `{}`", f.name);
        d.check_params(&f.params, &ctx);
        d.check_ty(&f.ret, &ctx);
        d.check_err(f.err.as_ref(), &ctx);
    }
    for class in &manifest.classes {
        if let Some(ctor) = &class.constructor {
            let ctx = format!("class `{}` constructor", class.name);
            d.check_params(&ctor.params, &ctx);
            d.check_err(ctor.err.as_ref(), &ctx);
        }
        for m in &class.methods {
            let ctx = format!("class `{}` method `{}`", class.name, m.name);
            d.check_params(&m.params, &ctx);
            d.check_ty(&m.ret, &ctx);
            d.check_err(m.err.as_ref(), &ctx);
        }
        for s in &class.statics {
            let ctx = format!("class `{}` static `{}`", class.name, s.name);
            d.check_params(&s.params, &ctx);
            if !s.returns_self {
                d.check_ty(&s.ret, &ctx);
            }
            d.check_err(s.err.as_ref(), &ctx);
        }
    }
    let mut found = d.found;
    found.sort();
    found.dedup();
    found
}

/// Named type references anywhere inside `ty`.
fn collect_refs(ty: &Ty, refs: &mut BTreeSet<String>) {
    match ty {
        Ty::Ref { name } => {
            refs.insert(name.clone());
        }
        Ty::Option { inner } | Ty::List { inner } => collect_refs(inner, refs),
        Ty::Map { value } => collect_refs(value, refs),
        Ty::Tuple { items } => {
            for item in items {
                collect_refs(item, refs);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emit::test_manifest::manifest;

    fn apply_py(patterns: &[&str]) -> Result<Manifest> {
        let patterns: Vec<String> = patterns.iter().map(|p| p.to_string()).collect();
        apply(&manifest(), &patterns, "[python]")
    }

    #[test]
    fn glob_matching_is_segmented_with_star_within_segments() {
        assert!(glob_match("render_summary", "render_summary"));
        assert!(glob_match("render_*", "render_summary"));
        assert!(glob_match("*", "render_summary"));
        assert!(glob_match("Session.warm_up", "Session.warm_up"));
        assert!(glob_match("Session.*", "Session.warm_up"));
        assert!(glob_match("*.warm_up", "Session.warm_up"));
        assert!(glob_match("S*n.wa*", "Session.warm_up"));

        // `*` never crosses a `.` boundary, and segment counts must match.
        assert!(!glob_match("*", "Session.warm_up"));
        assert!(!glob_match("Session.warm_up", "Session"));
        assert!(!glob_match("Session", "Session.warm_up"));
        assert!(!glob_match("render_*", "process_values"));
        assert!(!glob_match("", "x"));
    }

    #[test]
    fn empty_exclude_changes_nothing() {
        let m = manifest();
        assert_eq!(apply(&m, &[], "[python]").unwrap(), m);
    }

    #[test]
    fn functions_classes_methods_and_constants_are_excluded_by_name() {
        let filtered = apply_py(&["render_summary"]).unwrap();
        assert!(
            !filtered
                .functions
                .iter()
                .any(|f| f.name == "render_summary")
        );
        assert_eq!(filtered.functions.len(), manifest().functions.len() - 1);

        let filtered = apply_py(&["Session"]).unwrap();
        assert!(!filtered.classes.iter().any(|c| c.name == "Session"));
        assert!(filtered.classes.iter().any(|c| c.name == "RunningStats"));

        let filtered = apply_py(&["Session.warm_up", "Session.default_extension"]).unwrap();
        let session = filtered
            .classes
            .iter()
            .find(|c| c.name == "Session")
            .unwrap();
        assert!(!session.methods.iter().any(|m| m.name == "warm_up"));
        assert!(session.methods.iter().any(|m| m.name == "progress"));
        assert!(
            !session
                .statics
                .iter()
                .any(|s| s.name == "default_extension")
        );
        assert!(session.statics.iter().any(|s| s.name == "open"));

        let filtered = apply_py(&["DEFAULT_*"]).unwrap();
        assert!(
            !filtered
                .constants
                .iter()
                .any(|c| c.name.starts_with("DEFAULT_"))
        );
        assert!(
            filtered
                .constants
                .iter()
                .any(|c| c.name == "PROCESSOR_NAME")
        );
    }

    #[test]
    fn excluding_a_type_still_referenced_is_an_error() {
        // `QueryOptions` is referenced by `process_values`, by the
        // `DEFAULT_OPTIONS` constant, and by `RunningStats` members.
        let msg = apply_py(&["QueryOptions"]).unwrap_err().to_string();
        assert!(msg.contains("[python] exclude"), "{msg}");
        assert!(
            msg.contains("`QueryOptions` is excluded but function `process_values` references it"),
            "{msg}"
        );
        assert!(msg.contains("constant `DEFAULT_OPTIONS`"), "{msg}");

        // Excluding an error enum that a kept fallible item uses dangles too.
        let msg = apply_py(&["QueryError"]).unwrap_err().to_string();
        assert!(msg.contains("function `process_values`"), "{msg}");
        assert!(msg.contains("class `Session` static `open`"), "{msg}");
    }

    #[test]
    fn excluding_the_referencing_items_alongside_the_type_is_fine() {
        // Dropping every user of `SourceInfo` lets the type go with it —
        // an excluded class's references no longer count.
        let filtered = apply_py(&["SourceInfo", "Session.info"]).unwrap();
        assert!(!filtered.types.iter().any(|t| t.name() == "SourceInfo"));
        let session = filtered
            .classes
            .iter()
            .find(|c| c.name == "Session")
            .unwrap();
        assert!(!session.methods.iter().any(|m| m.name == "info"));
    }
}
