//! The Python emitter (codegen.md §4, §7).
//!
//! Produces the seven files of the wholly-owned `generated` package.
//! Output targets Python ≥ 3.13, 4-space indent, double quotes, and
//! passes `ruff check --select E,F,I,N,UP,RUF` at line-length 120.
//! Models are emitted in dependency order (recursion is impossible in
//! the type system), so forward references only appear in the cycle
//! fallback. Types whose origin crate has an entry in `[python.imports]`
//! are imported from that package instead of re-emitted (codegen.md §9).

use super::util::{
    collect_refs, doc_lines, find_type, int_bounds, py_alias_roundtrips, py_docstring, py_header,
    py_name, py_type,
};
use rspyts_core::ir::{
    ClassDecl, Dtype, FieldDecl, FnDecl, Manifest, ParamDecl, StaticDecl, Target, Ty, TypeDecl,
    VariantDecl,
};
use std::collections::{BTreeMap, BTreeSet};

const LINE_LIMIT: usize = 100;

/// Emit the whole package: `(file name, content)` pairs, name-sorted.
pub fn emit(
    m: &Manifest,
    hash: &str,
    library_search: &[String],
    imports: &BTreeMap<String, String>,
) -> Vec<(&'static str, String)> {
    vec![
        ("__init__.py", init_py(m, hash)),
        ("classes.py", classes_py(m, hash)),
        ("constants.py", constants_py(m, hash)),
        ("errors.py", errors_py(m, hash, imports)),
        ("functions.py", functions_py(m, hash)),
        ("library.py", library_py(m, hash, library_search)),
        ("models.py", models_py(m, hash, imports)),
    ]
}

/// Is `decl` imported from another crate's generated package instead of
/// emitted here? Foreign-origin types without a mapping stay local, so
/// the output is always self-contained.
fn is_imported(m: &Manifest, decl: &TypeDecl, imports: &BTreeMap<String, String>) -> bool {
    decl.origin() != m.crate_name && imports.contains_key(decl.origin())
}

/// The projection names a type declaration contributes: the type itself,
/// plus one variant class per data-enum variant.
fn projected_names(decl: &TypeDecl) -> Vec<String> {
    match decl {
        TypeDecl::Struct { name, .. } | TypeDecl::StringEnum { name, .. } => vec![name.clone()],
        TypeDecl::Enum { name, variants, .. } => std::iter::once(name.clone())
            .chain(variants.iter().map(|v| format!("{name}{}", v.name)))
            .collect(),
        TypeDecl::ErrorEnum { name, variants, .. } => std::iter::once(name.clone())
            .chain(variants.iter().map(|v| format!("{name}{}", v.name)))
            .collect(),
    }
}

// ---------------------------------------------------------------- models.py

fn models_py(m: &Manifest, hash: &str, imports: &BTreeMap<String, String>) -> String {
    let mut body = String::new();
    let mut defined: BTreeSet<String> = BTreeSet::new();
    // Imported foreign types count as defined from the start: local
    // types may reference them without forward-quoting.
    for decl in &m.types {
        if is_imported(m, decl, imports) {
            defined.insert(decl.name().to_string());
        }
    }
    let mut uses_any = false;
    for decl in data_types_in_dependency_order(m, imports) {
        let quote = |name: &str| {
            // Only quote refs to data types that are declared but not yet
            // emitted (cycle fallback); unknown names never reach here
            // (validated) and error enums are never data refs.
            !defined.contains(name) && find_type(m, name).is_some()
        };
        body.push_str("\n\n");
        match decl {
            TypeDecl::Struct {
                name, docs, fields, ..
            } => {
                uses_any |= fields.iter().any(|f| uses_json(&f.ty));
                body.push_str(&format!("class {name}(Contract):\n"));
                body.push_str(&py_docstring(docs, "    "));
                if fields.is_empty() && doc_lines(docs).is_empty() {
                    body.push_str("    pass\n");
                }
                // ruff format separates a class docstring from the first
                // member with one blank line.
                if !doc_lines(docs).is_empty() && !fields.is_empty() {
                    body.push('\n');
                }
                for field in fields {
                    body.push_str(&field_line(field, &quote));
                }
            }
            TypeDecl::StringEnum {
                name,
                docs,
                variants,
                ..
            } => {
                body.push_str(&format!("class {name}(StrEnum):\n"));
                body.push_str(&py_docstring(docs, "    "));
                if !doc_lines(docs).is_empty() && !variants.is_empty() {
                    body.push('\n');
                }
                for v in variants {
                    body.push_str(&format!(
                        "    {} = \"{}\"\n",
                        heck::ToShoutySnakeCase::to_shouty_snake_case(v.name.as_str()),
                        v.wire_name
                    ));
                }
            }
            TypeDecl::Enum {
                name,
                docs,
                tag,
                variants,
                ..
            } => {
                uses_any |= variants
                    .iter()
                    .flat_map(|v| v.fields.iter())
                    .any(|f| uses_json(&f.ty));
                for v in variants {
                    body.push_str(&variant_class(name, tag, v, &quote));
                    body.push_str("\n\n");
                }
                body.push_str(&union_alias(name, tag, variants));
                if !doc_lines(docs).is_empty() {
                    body.push_str(&py_docstring(docs, ""));
                }
            }
            TypeDecl::ErrorEnum { .. } => unreachable!("error enums are not data types"),
        }
        defined.insert(decl.name().to_string());
    }

    let mut out = py_header(hash);
    out.push_str(&format!(
        "\"\"\"\nData models bridged from `{}`.\n\"\"\"\n",
        m.crate_name
    ));
    out.push_str(&models_imports(
        &body,
        uses_any,
        &foreign_import_lines(m, imports),
    ));
    out.push_str(&body);
    out
}

/// One `from {module} import …` line per foreign package that data
/// types are imported from, keyed by module for section sorting. Lines
/// whose names are not all referenced by the emitted body carry a
/// `noqa: F401` — they exist for `__init__` to re-export.
fn foreign_import_lines(m: &Manifest, imports: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let mut by_module: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for decl in &m.types {
        if matches!(decl, TypeDecl::ErrorEnum { .. }) || !is_imported(m, decl, imports) {
            continue;
        }
        let module = imports[decl.origin()].as_str();
        by_module
            .entry(module)
            .or_default()
            .extend(projected_names(decl));
    }
    // A name is "used" when the emitted body mentions it (annotations
    // and union aliases mention types by their exact name).
    let mut refs: BTreeSet<String> = BTreeSet::new();
    for decl in &m.types {
        if is_imported(m, decl, imports) {
            continue;
        }
        let fields: Vec<&FieldDecl> = match decl {
            TypeDecl::Struct { fields, .. } => fields.iter().collect(),
            TypeDecl::Enum { variants, .. } => {
                variants.iter().flat_map(|v| v.fields.iter()).collect()
            }
            _ => Vec::new(),
        };
        for f in fields {
            collect_refs(&f.ty, &mut refs);
        }
    }
    by_module
        .into_iter()
        .map(|(module, mut names)| {
            names.sort();
            let unused = names.iter().any(|n| !refs.contains(n));
            let mut line = format!("from {module} import {}", names.join(", "));
            if unused {
                line.push_str("  # noqa: F401");
            }
            (module.to_string(), line)
        })
        .collect()
}

/// Data types (error enums and imported foreign types excluded) in
/// dependency order: a type is emitted only after every type it
/// references, ties broken by name so output is deterministic. The type
/// system forbids recursion, so the name-ordered fallback for leftovers
/// only runs on malformed input.
fn data_types_in_dependency_order<'m>(
    m: &'m Manifest,
    imports: &BTreeMap<String, String>,
) -> Vec<&'m TypeDecl> {
    let data: Vec<&TypeDecl> = m
        .types
        .iter()
        .filter(|t| !matches!(t, TypeDecl::ErrorEnum { .. }) && !is_imported(m, t, imports))
        .collect();
    let names: BTreeSet<&str> = data.iter().map(|t| t.name()).collect();
    let deps = |t: &TypeDecl| -> BTreeSet<String> {
        let mut refs = BTreeSet::new();
        let fields: Vec<&FieldDecl> = match t {
            TypeDecl::Struct { fields, .. } => fields.iter().collect(),
            TypeDecl::Enum { variants, .. } => {
                variants.iter().flat_map(|v| v.fields.iter()).collect()
            }
            _ => Vec::new(),
        };
        for f in fields {
            collect_refs(&f.ty, &mut refs);
        }
        refs.retain(|r| names.contains(r.as_str()) && r != t.name());
        refs
    };

    let mut remaining: Vec<&TypeDecl> = data;
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut order = Vec::new();
    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .position(|t| deps(t).iter().all(|d| done.contains(d)));
        // `remaining` stays name-sorted (manifest order), so `position`
        // picks the lexicographically-first ready type.
        let idx = ready.unwrap_or(0);
        let decl = remaining.remove(idx);
        done.insert(decl.name().to_string());
        order.push(decl);
    }
    order
}

/// One pydantic model field line (4-space indent).
fn field_line(field: &FieldDecl, quote: &dyn Fn(&str) -> bool) -> String {
    let name = py_name(&field.wire_name);
    let is_opt = field.optional || matches!(field.ty, Ty::Option { .. });
    // Bounded integers at the top level of a field move their bounds
    // into the `Field(...)` default instead of `Annotated[...]`.
    let top_bounds = match &field.ty {
        Ty::Option { inner } => int_bounds(inner),
        other => int_bounds(other),
    };
    let annotation = match (&field.ty, top_bounds) {
        (Ty::Option { .. }, Some(_)) => "int | None".to_string(),
        (_, Some(_)) => "int".to_string(),
        (ty, None) => py_type(ty, true, quote),
    };

    let mut args: Vec<String> = Vec::new();
    if is_opt {
        args.push("default=None".to_string());
    }
    if !py_alias_roundtrips(&name, &field.wire_name) {
        args.push(format!("alias=\"{}\"", field.wire_name));
    }
    if let Some((lo, hi)) = top_bounds {
        args.push(format!("ge={lo}"));
        args.push(format!("le={hi}"));
    }
    let default = match args.as_slice() {
        [] => String::new(),
        [only] if only == "default=None" => " = None".to_string(),
        _ => format!(" = Field({})", args.join(", ")),
    };
    format!("    {name}: {annotation}{default}\n")
}

/// One data-enum variant model, e.g. `class ThresholdEventCrossed(Contract)`.
fn variant_class(
    enum_name: &str,
    tag: &str,
    v: &VariantDecl,
    quote: &dyn Fn(&str) -> bool,
) -> String {
    let mut out = format!("class {enum_name}{}(Contract):\n", v.name);
    out.push_str(&py_docstring(&v.docs, "    "));
    // ruff format separates a class docstring from the first member with
    // one blank line.
    if !doc_lines(&v.docs).is_empty() {
        out.push('\n');
    }
    let tag_attr = py_name(tag);
    if py_alias_roundtrips(&tag_attr, tag) {
        out.push_str(&format!(
            "    {tag_attr}: Literal[\"{w}\"] = \"{w}\"\n",
            w = v.wire_name
        ));
    } else {
        out.push_str(&format!(
            "    {tag_attr}: Literal[\"{w}\"] = Field(default=\"{w}\", alias=\"{tag}\")\n",
            w = v.wire_name
        ));
    }
    for field in &v.fields {
        out.push_str(&field_line(field, quote));
    }
    // Trim the final newline: the caller controls inter-class spacing.
    out.truncate(out.trim_end_matches('\n').len());
    out.push('\n');
    out
}

/// The discriminated-union alias for a data enum.
fn union_alias(name: &str, tag: &str, variants: &[VariantDecl]) -> String {
    let members: Vec<String> = variants
        .iter()
        .map(|v| format!("{name}{}", v.name))
        .collect();
    if members.len() == 1 {
        // A single-variant union is not a union; alias the class directly.
        return format!("{name} = {}\n", members[0]);
    }
    let joined = members.join(" | ");
    let mut out = format!("{name} = Annotated[\n");
    // 4-space indent plus the trailing comma must stay on one line.
    if 4 + joined.len() < LINE_LIMIT {
        out.push_str(&format!("    {joined},\n"));
    } else {
        out.push_str(&format!("    {}\n", members[0]));
        for member in &members[1..] {
            out.push_str(&format!("    | {member}\n"));
        }
        // Re-render with the trailing comma on the last member.
        out.truncate(out.trim_end_matches('\n').len());
        out.push_str(",\n");
    }
    out.push_str(&format!(
        "    Field(discriminator=\"{}\"),\n]\n",
        py_name(tag)
    ));
    out
}

/// Imports for models.py, derived from what the body actually uses plus
/// the foreign `(module, line)` imports, section-sorted for isort.
fn models_imports(body: &str, uses_any: bool, foreign: &[(String, String)]) -> String {
    let mut stdlib: Vec<String> = Vec::new();
    if body.contains("StrEnum") {
        stdlib.push("from enum import StrEnum".to_string());
    }
    let mut typing: Vec<&str> = Vec::new();
    if body.contains("Annotated[") {
        typing.push("Annotated");
    }
    if uses_any {
        typing.push("Any");
    }
    if body.contains("Literal[") {
        typing.push("Literal");
    }
    if !typing.is_empty() {
        stdlib.push(format!("from typing import {}", typing.join(", ")));
    }
    let mut third: Vec<(String, String)> = foreign.to_vec();
    if body.contains("np.ndarray") {
        third.push(("numpy".to_string(), "import numpy as np".to_string()));
    }
    if body.contains("Field(") {
        third.push((
            "pydantic".to_string(),
            "from pydantic import Field".to_string(),
        ));
    }
    if body.contains("(Contract)") {
        third.push((
            "rspyts".to_string(),
            "from rspyts import Contract".to_string(),
        ));
    }
    third.sort();
    let mut out = String::new();
    if !stdlib.is_empty() {
        out.push('\n');
        out.push_str(&stdlib.join("\n"));
        out.push('\n');
    }
    if !third.is_empty() {
        out.push('\n');
        let lines: Vec<&str> = third.iter().map(|(_, l)| l.as_str()).collect();
        out.push_str(&lines.join("\n"));
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------- errors.py

fn errors_py(m: &Manifest, hash: &str, imports: &BTreeMap<String, String>) -> String {
    let mut out = py_header(hash);
    out.push_str(&format!(
        "\"\"\"\nException classes bridged from `{}`.\n\"\"\"\n",
        m.crate_name
    ));

    let mut local: Vec<(&String, &String, &Vec<rspyts_core::ir::ErrorVariantDecl>)> = Vec::new();
    let mut foreign: Vec<(String, String)> = Vec::new();
    for t in &m.types {
        let TypeDecl::ErrorEnum {
            name,
            docs,
            variants,
            ..
        } = t
        else {
            continue;
        };
        if is_imported(m, t, imports) {
            // The foreign package's own errors module registers these
            // classes with the runtime on import; re-import for export.
            let module = imports[t.origin()].as_str();
            let names = projected_names(t);
            foreign.push((
                module.to_string(),
                format!("from {module} import {}  # noqa: F401", names.join(", ")),
            ));
        } else {
            local.push((name, docs, variants));
        }
    }
    if local.is_empty() && foreign.is_empty() {
        return out;
    }

    let mut third: Vec<(String, String)> = foreign;
    if !local.is_empty() {
        third.push(("rspyts".to_string(), "import rspyts".to_string()));
    }
    third.sort();
    out.push('\n');
    let lines: Vec<&str> = third.iter().map(|(_, l)| l.as_str()).collect();
    out.push_str(&lines.join("\n"));
    out.push('\n');

    for (name, docs, variants) in &local {
        out.push_str("\n\n");
        out.push_str(&error_class(name, "rspyts.BridgeError", docs, &[]));
        for v in *variants {
            out.push_str("\n\n");
            out.push_str(&error_class(
                &format!("{name}{}", v.name),
                name,
                &v.docs,
                &v.fields,
            ));
        }
    }
    if !local.is_empty() {
        out.push_str("\n\n");
    }
    for (name, _, variants) in &local {
        for v in *variants {
            out.push_str(&format!(
                "rspyts.register_error(\"{}\", {name}{})\n",
                v.wire_code, v.name
            ));
        }
    }
    out
}

fn error_class(name: &str, base: &str, docs: &str, data_fields: &[FieldDecl]) -> String {
    let data_note = if data_fields.is_empty() {
        String::new()
    } else {
        let entries: Vec<String> = data_fields
            .iter()
            .map(|f| format!("\"{}\": {}", f.wire_name, py_type(&f.ty, false, &|_| false)))
            .collect();
        format!("# .data: {{{}}}", entries.join(", "))
    };
    // pep8-naming wants exception class names to end in `Error`; variant
    // classes are named `{Enum}{Variant}` by design, so suppress N818.
    let noqa = if name.ends_with("Error") {
        ""
    } else {
        "  # noqa: N818"
    };
    let docstring = py_docstring(docs, "    ");
    if docstring.is_empty() {
        if data_note.is_empty() {
            format!("class {name}({base}): ...{noqa}\n")
        } else {
            format!("class {name}({base}): ...{noqa}  {data_note}\n")
        }
    } else if data_note.is_empty() {
        format!("class {name}({base}):{noqa}\n{docstring}")
    } else {
        // The blank line after the docstring matches ruff format.
        format!("class {name}({base}):{noqa}\n{docstring}\n    {data_note}\n")
    }
}

// ---------------------------------------------------------------- library.py

fn library_py(m: &Manifest, hash: &str, library_search: &[String]) -> String {
    let mut out = py_header(hash);
    out.push_str(&format!(
        "\"\"\"\nLoader for the compiled `{}` bridge library.\n\"\"\"\n",
        m.crate_name
    ));
    out.push_str("\nfrom pathlib import Path\n\nfrom rspyts import Library\n\n");
    out.push_str("LIB = Library(\n");
    out.push_str(&format!(
        "    name=\"{}\",\n",
        m.crate_name.replace('-', "_")
    ));
    if library_search.is_empty() {
        out.push_str("    search=[],\n");
    } else {
        out.push_str("    search=[\n");
        for dir in library_search {
            out.push_str(&format!("        \"{dir}\",\n"));
        }
        out.push_str("    ],\n");
    }
    // Relative search entries must resolve against the generated package
    // directory, not the process cwd (codegen.md §4.1).
    out.push_str("    anchor=Path(__file__).parent,\n");
    out.push_str(")\n");
    out
}

// -------------------------------------------------------------- constants.py

fn constants_py(m: &Manifest, hash: &str) -> String {
    let mut body = String::new();
    let mut model_imports: BTreeSet<String> = BTreeSet::new();
    let mut uses_any = false;
    for c in &m.constants {
        body.push('\n');
        for line in doc_lines(&c.docs) {
            if line.trim().is_empty() {
                body.push_str("#\n");
            } else {
                body.push_str(&format!("# {line}\n"));
            }
        }
        uses_any |= uses_json(&c.ty);
        let mut refs = BTreeSet::new();
        collect_refs(&c.ty, &mut refs);
        model_imports.extend(refs);
        let annotation = py_type(&c.ty, false, &|_| false);
        let expr = py_const_expr(m, &c.ty, &c.value, &mut model_imports);
        let head = format!("{}: Final[{annotation}] = ", c.name);
        let one = format!("{head}{expr}");
        if one.len() <= LINE_LIMIT {
            body.push_str(&one);
            body.push('\n');
        } else if let Some((callee, arg)) = split_call(&expr) {
            body.push_str(&format!("{head}{callee}(\n    {arg}\n)\n"));
        } else {
            body.push_str(&format!("{head}(\n    {expr}\n)\n"));
        }
    }

    let mut out = py_header(hash);
    out.push_str(&format!(
        "\"\"\"\nConstants bridged from `{}`.\n\"\"\"\n",
        m.crate_name
    ));
    if !m.constants.is_empty() {
        let typing = if uses_any { "Any, Final" } else { "Final" };
        out.push_str(&format!("\nfrom typing import {typing}\n"));
        if !model_imports.is_empty() {
            let names: Vec<String> = model_imports.into_iter().collect();
            out.push('\n');
            out.push_str(&wrap_from_import(".models", &names));
        }
    }
    out.push_str(&body);
    out
}

/// The Python expression for a constant's value: plain literals for
/// data shapes, `Model.model_validate({…})` for struct/enum refs, and
/// the enum constructor for string enums.
fn py_const_expr(
    m: &Manifest,
    ty: &Ty,
    value: &serde_json::Value,
    model_imports: &mut BTreeSet<String>,
) -> String {
    match ty {
        Ty::Ref { name } => match find_type(m, name) {
            Some(TypeDecl::Struct { .. }) => {
                format!("{name}.model_validate({})", py_json(value))
            }
            Some(TypeDecl::Enum { tag, variants, .. }) => {
                let tag_value = value.get(tag).and_then(|t| t.as_str()).unwrap_or_default();
                match variants.iter().find(|v| v.wire_name == tag_value) {
                    Some(v) => {
                        model_imports.insert(format!("{name}{}", v.name));
                        format!("{name}{}.model_validate({})", v.name, py_json(value))
                    }
                    None => py_json(value),
                }
            }
            Some(TypeDecl::StringEnum { .. }) => format!("{name}({})", py_json(value)),
            _ => py_json(value),
        },
        Ty::Option { inner } => {
            if value.is_null() {
                "None".to_string()
            } else {
                py_const_expr(m, inner, value, model_imports)
            }
        }
        Ty::List { inner } => match value.as_array() {
            Some(items) => {
                let rendered: Vec<String> = items
                    .iter()
                    .map(|v| py_const_expr(m, inner, v, model_imports))
                    .collect();
                format!("[{}]", rendered.join(", "))
            }
            None => py_json(value),
        },
        Ty::Map { value: value_ty } => match value.as_object() {
            Some(entries) => {
                let rendered: Vec<String> = entries
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "{}: {}",
                            py_json(&serde_json::Value::String(k.clone())),
                            py_const_expr(m, value_ty, v, model_imports)
                        )
                    })
                    .collect();
                format!("{{{}}}", rendered.join(", "))
            }
            None => py_json(value),
        },
        _ => py_json(value),
    }
}

/// A JSON value as a Python literal. JSON string escapes are a subset
/// of Python's, so serde's rendering is reused for strings and keys.
fn py_json(value: &serde_json::Value) -> String {
    use serde_json::Value;
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => serde_json::to_string(s).expect("string serializes"),
        Value::Array(items) => {
            let rendered: Vec<String> = items.iter().map(py_json).collect();
            format!("[{}]", rendered.join(", "))
        }
        Value::Object(entries) => {
            let rendered: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{}: {}",
                        serde_json::to_string(k).expect("key serializes"),
                        py_json(v)
                    )
                })
                .collect();
            format!("{{{}}}", rendered.join(", "))
        }
    }
}

/// Split `Callee(arg)` into its callee and argument when `expr` is one
/// top-level call on a dotted name — the wrap point for long constants.
fn split_call(expr: &str) -> Option<(&str, &str)> {
    let open = expr.find('(')?;
    let callee = &expr[..open];
    if callee.is_empty()
        || !callee
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        || !expr.ends_with(')')
    {
        return None;
    }
    Some((callee, &expr[open + 1..expr.len() - 1]))
}

// -------------------------------------------------------------- functions.py

fn functions_py(m: &Manifest, hash: &str) -> String {
    let fns: Vec<&FnDecl> = m
        .functions
        .iter()
        .filter(|f| f.targets.contains(&Target::Python))
        .collect();
    let mut body = String::new();
    let mut model_imports: BTreeSet<String> = BTreeSet::new();
    let mut uses_np = false;
    let uses_any = fns
        .iter()
        .any(|f| uses_json(&f.ret) || f.params.iter().any(|p| uses_json(&p.ty)));
    for f in &fns {
        body.push_str("\n\n");
        body.push_str(&function_def(m, f, &mut model_imports, &mut uses_np));
    }

    let mut out = py_header(hash);
    out.push_str("\"\"\"\nTyped wrappers for bridged free functions.\n\"\"\"\n");
    if !fns.is_empty() {
        if uses_any {
            out.push_str("\nfrom typing import Any\n");
        }
        if uses_np {
            out.push_str("\nimport numpy as np\n");
        }
        out.push('\n');
        // `errors` is imported for its import-time register_error calls.
        out.push_str("from . import errors, library  # noqa: F401\n");
        if !model_imports.is_empty() {
            let names: Vec<String> = model_imports.into_iter().collect();
            out.push_str(&wrap_from_import(".models", &names));
        }
    }
    out.push_str(&body);
    out
}

fn function_def(
    m: &Manifest,
    f: &FnDecl,
    model_imports: &mut BTreeSet<String>,
    uses_np: &mut bool,
) -> String {
    let params = py_params(m, &f.params, model_imports, uses_np);
    let ret = py_ret_annotation(m, &f.ret, model_imports, uses_np);
    let mut out = def_line("    ", &f.name, &params, &ret, 0);
    out.push_str(&py_docstring(&f.docs, "    "));
    out.push_str(&call_block(
        "    ",
        &f.ret,
        &format!("rspyts_fn__{}", f.name),
        &args_entries(m, &f.params),
        &slice_params(&f.params),
        false,
    ));
    out.push_str(&return_stmts(m, &f.ret, "    ", model_imports));
    out
}

// ---------------------------------------------------------------- classes.py

fn classes_py(m: &Manifest, hash: &str) -> String {
    let mut body = String::new();
    let mut model_imports: BTreeSet<String> = BTreeSet::new();
    let mut uses_np = false;
    for class in &m.classes {
        body.push_str("\n\n");
        body.push_str(&class_def(m, class, &mut model_imports, &mut uses_np));
    }

    let mut out = py_header(hash);
    out.push_str("\"\"\"\nHandle classes for bridged Rust objects.\n\"\"\"\n");
    if !m.classes.is_empty() {
        if m.classes.iter().any(class_uses_json) {
            out.push_str("\nfrom typing import Any, Self\n");
        } else {
            out.push_str("\nfrom typing import Self\n");
        }
        if uses_np {
            out.push_str("\nimport numpy as np\n");
        }
        out.push('\n');
        // `errors` is imported for its import-time register_error calls.
        out.push_str("from . import errors, library  # noqa: F401\n");
        if !model_imports.is_empty() {
            let names: Vec<String> = model_imports.into_iter().collect();
            out.push_str(&wrap_from_import(".models", &names));
        }
    }
    out.push_str(&body);
    out
}

fn class_def(
    m: &Manifest,
    class: &ClassDecl,
    model_imports: &mut BTreeSet<String>,
    uses_np: &mut bool,
) -> String {
    let name = &class.name;
    let statics: Vec<&StaticDecl> = class
        .statics
        .iter()
        .filter(|s| s.targets.contains(&Target::Python))
        .collect();
    let factories: Vec<&&StaticDecl> = statics.iter().filter(|s| s.returns_self).collect();

    let mut out = format!("class {name}:\n");
    out.push_str(&py_docstring(&class.docs, "    "));
    if !out.ends_with(":\n") {
        out.push('\n');
    }

    // Factories assign the handle on an instance made with `__new__`,
    // so it is declared at class level; a plain constructor annotates
    // its own assignment instead.
    let class_level_handle = class.constructor.is_none() || !factories.is_empty();
    if class_level_handle {
        out.push_str("    handle: int\n\n");
    }

    match &class.constructor {
        Some(ctor) => {
            let mut params = vec![("self".to_string(), String::new())];
            params.extend(py_params(m, &ctor.params, model_imports, uses_np));
            out.push_str(&def_line("        ", "__init__", &params, "None", 4));
            out.push_str(&py_docstring(&ctor.docs, "        "));
            out.push_str(&call_block(
                "        ",
                &Ty::U32, // any non-unit type: the handle is assigned below
                &format!("rspyts_cls__{name}__new"),
                &args_entries(m, &ctor.params),
                &slice_params(&ctor.params),
                false,
            ));
            if class_level_handle {
                out.push_str("        self.handle = raw\n");
            } else {
                out.push_str("        self.handle: int = raw\n");
            }
        }
        None => {
            let hint = match factories.as_slice() {
                [] => String::new(),
                many => format!(
                    "; use {}",
                    many.iter()
                        .map(|s| format!("{name}.{}(...)", s.name))
                        .collect::<Vec<_>>()
                        .join(" or ")
                ),
            };
            let message = format!("{name} cannot be constructed directly{hint}");
            out.push_str("    def __init__(self) -> None:\n");
            let one = format!("        raise TypeError(\"{message}\")");
            if one.len() <= LINE_LIMIT {
                out.push_str(&one);
                out.push('\n');
            } else {
                out.push_str(&format!(
                    "        raise TypeError(\n            \"{message}\"\n        )\n"
                ));
            }
        }
    }

    // Statics: factories as classmethods, the rest as staticmethods.
    for s in &statics {
        out.push('\n');
        if s.returns_self {
            out.push_str("    @classmethod\n");
            let mut params = vec![("cls".to_string(), String::new())];
            params.extend(py_params(m, &s.params, model_imports, uses_np));
            out.push_str(&def_line("        ", &s.name, &params, "Self", 4));
            out.push_str(&py_docstring(&s.docs, "        "));
            out.push_str(&call_block(
                "        ",
                &Ty::U32, // any non-unit type: the handle is assigned below
                &format!("rspyts_cls__{name}__{}", s.name),
                &args_entries(m, &s.params),
                &slice_params(&s.params),
                false,
            ));
            out.push_str("        obj = cls.__new__(cls)\n");
            out.push_str("        obj.handle = raw\n");
            out.push_str("        return obj\n");
        } else {
            out.push_str("    @staticmethod\n");
            let params = py_params(m, &s.params, model_imports, uses_np);
            let ret = py_ret_annotation(m, &s.ret, model_imports, uses_np);
            out.push_str(&def_line("        ", &s.name, &params, &ret, 4));
            out.push_str(&py_docstring(&s.docs, "        "));
            out.push_str(&call_block(
                "        ",
                &s.ret,
                &format!("rspyts_cls__{name}__{}", s.name),
                &args_entries(m, &s.params),
                &slice_params(&s.params),
                false,
            ));
            out.push_str(&return_stmts(m, &s.ret, "        ", model_imports));
        }
    }

    // Methods.
    for method in &class.methods {
        if !method.targets.contains(&Target::Python) {
            continue;
        }
        out.push('\n');
        let mut params = vec![("self".to_string(), String::new())];
        params.extend(py_params(m, &method.params, model_imports, uses_np));
        let ret = py_ret_annotation(m, &method.ret, model_imports, uses_np);
        out.push_str(&def_line("        ", &method.name, &params, &ret, 4));
        out.push_str(&py_docstring(&method.docs, "        "));
        out.push_str(&call_block(
            "        ",
            &method.ret,
            &format!("rspyts_cls__{name}__{}", method.name),
            &args_entries(m, &method.params),
            &slice_params(&method.params),
            true,
        ));
        out.push_str(&return_stmts(m, &method.ret, "        ", model_imports));
    }

    // Lifecycle plumbing (codegen.md §4.2).
    out.push_str(&format!(
        "
    def close(self) -> None:
        \"\"\"
        Release the underlying Rust object. Safe to call more than once.
        \"\"\"
        library.LIB.call_drop(\"rspyts_cls__{name}__drop\", self.handle)

    def __enter__(self) -> Self:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    def __del__(self) -> None:
        try:
            self.close()
        except Exception:
            pass
"
    ));
    out
}

// ---------------------------------------------------------------- __init__.py

fn init_py(m: &Manifest, hash: &str) -> String {
    let mut model_names: Vec<String> = Vec::new();
    let mut error_names: Vec<String> = Vec::new();
    for t in &m.types {
        match t {
            TypeDecl::ErrorEnum { .. } => error_names.extend(projected_names(t)),
            _ => model_names.extend(projected_names(t)),
        }
    }
    let class_names: Vec<String> = m.classes.iter().map(|c| c.name.clone()).collect();
    let const_names: Vec<String> = m.constants.iter().map(|c| c.name.clone()).collect();
    let fn_names: Vec<String> = m
        .functions
        .iter()
        .filter(|f| f.targets.contains(&Target::Python))
        .map(|f| f.name.clone())
        .collect();

    let mut out = py_header(hash);
    out.push_str(&format!(
        "\"\"\"\nGenerated bridge surface for `{}`.\n\"\"\"\n",
        m.crate_name
    ));

    let mut sections: Vec<(&str, Vec<String>)> = vec![
        (".classes", class_names),
        (".constants", const_names),
        (".errors", error_names),
        (".functions", fn_names),
        (".models", model_names),
    ];
    let mut any = false;
    for (module, names) in &mut sections {
        if names.is_empty() {
            continue;
        }
        if !any {
            out.push('\n');
            any = true;
        }
        names.sort();
        out.push_str(&wrap_from_import(module, names));
    }

    let mut all: Vec<String> = sections.into_iter().flat_map(|(_, names)| names).collect();
    // RUF022's isort-style `__all__` order: SCREAMING_SNAKE_CASE
    // constants first, everything else after, byte-ordered within each.
    let screaming =
        |n: &str| !n.chars().any(|c| c.is_ascii_lowercase()) && n.chars().any(|c| c.is_uppercase());
    all.sort_by(|a, b| (!screaming(a), a.as_str()).cmp(&(!screaming(b), b.as_str())));
    out.push('\n');
    if all.is_empty() {
        out.push_str("__all__: list[str] = []\n");
    } else {
        out.push_str("__all__ = [\n");
        for name in &all {
            out.push_str(&format!("    \"{name}\",\n"));
        }
        out.push_str("]\n");
    }
    out
}

// ------------------------------------------------------------------ shared

/// Names the wrapper bodies reference at module scope: the sibling
/// modules imported at the top of `functions.py`/`classes.py` and the
/// `np` alias. A parameter binding with one of these names would shadow
/// the import, so it gets a trailing underscore (the args-dict wire key
/// stays exact).
const PY_MODULE_NAMES: &[&str] = &["errors", "library", "models", "np"];

/// The Python binding name for a parameter: keyword-escaped via
/// [`py_name`], then underscore-escaped when it would shadow a generated
/// module import.
fn py_param_name(name: &str) -> String {
    let escaped = py_name(name);
    if PY_MODULE_NAMES.contains(&escaped.as_str()) {
        format!("{escaped}_")
    } else {
        escaped
    }
}

/// Python `(name, annotation)` pairs for a parameter list.
fn py_params(
    m: &Manifest,
    params: &[ParamDecl],
    model_imports: &mut BTreeSet<String>,
    uses_np: &mut bool,
) -> Vec<(String, String)> {
    params
        .iter()
        .map(|p| {
            record_ty_uses(m, &p.ty, model_imports, uses_np);
            (py_param_name(&p.name), py_type(&p.ty, false, &|_| false))
        })
        .collect()
}

fn py_ret_annotation(
    m: &Manifest,
    ret: &Ty,
    model_imports: &mut BTreeSet<String>,
    uses_np: &mut bool,
) -> String {
    record_ty_uses(m, ret, model_imports, uses_np);
    py_type(ret, false, &|_| false)
}

fn record_ty_uses(m: &Manifest, ty: &Ty, model_imports: &mut BTreeSet<String>, uses_np: &mut bool) {
    let mut refs = BTreeSet::new();
    collect_refs(ty, &mut refs);
    for name in refs {
        if find_type(m, &name).is_some() {
            model_imports.insert(name);
        }
    }
    if uses_ndarray(ty) {
        *uses_np = true;
    }
}

fn uses_ndarray(ty: &Ty) -> bool {
    match ty {
        Ty::Buf { .. } | Ty::Slice { .. } => true,
        Ty::Option { inner } | Ty::List { inner } => uses_ndarray(inner),
        Ty::Map { value } => uses_ndarray(value),
        _ => false,
    }
}

fn uses_json(ty: &Ty) -> bool {
    match ty {
        Ty::Json => true,
        Ty::Option { inner } | Ty::List { inner } => uses_json(inner),
        Ty::Map { value } => uses_json(value),
        _ => false,
    }
}

/// True when any Python-projected part of `class` needs `typing.Any`.
fn class_uses_json(class: &ClassDecl) -> bool {
    let in_params = |params: &[ParamDecl]| params.iter().any(|p| uses_json(&p.ty));
    class
        .constructor
        .as_ref()
        .is_some_and(|c| in_params(&c.params))
        || class
            .statics
            .iter()
            .filter(|s| s.targets.contains(&Target::Python))
            .any(|s| in_params(&s.params) || (!s.returns_self && uses_json(&s.ret)))
        || class
            .methods
            .iter()
            .filter(|method| method.targets.contains(&Target::Python))
            .any(|method| in_params(&method.params) || uses_json(&method.ret))
}

/// `def name(params) -> ret:` wrapped when past the line limit.
/// `extra_indent` accounts for the def keyword's own indentation.
fn def_line(
    body_indent: &str,
    name: &str,
    params: &[(String, String)],
    ret: &str,
    def_indent: usize,
) -> String {
    let indent = " ".repeat(def_indent);
    let rendered: Vec<String> = params
        .iter()
        .map(|(n, a)| {
            if a.is_empty() {
                n.clone()
            } else {
                format!("{n}: {a}")
            }
        })
        .collect();
    let one = format!("{indent}def {name}({}) -> {ret}:", rendered.join(", "));
    if one.len() <= LINE_LIMIT {
        return one + "\n";
    }
    let mut out = format!("{indent}def {name}(\n");
    for p in &rendered {
        out.push_str(body_indent);
        out.push_str(p);
        out.push_str(",\n");
    }
    out.push_str(&format!("{indent}) -> {ret}:\n"));
    out
}

/// `"wireName": <converted arg>` entries for the plain parameters.
fn args_entries(m: &Manifest, params: &[ParamDecl]) -> Vec<String> {
    params
        .iter()
        .filter(|p| !matches!(p.ty, Ty::Slice { .. }))
        .map(|p| {
            let expr = py_param_name(&p.name);
            let converted = py_conv(&expr, &p.ty, m, Dir::Dump, 0).unwrap_or(expr);
            format!("\"{}\": {converted}", p.wire_name)
        })
        .collect()
}

fn slice_params(params: &[ParamDecl]) -> Vec<(String, Dtype)> {
    params
        .iter()
        .filter_map(|p| match p.ty {
            Ty::Slice { dt } => Some((py_param_name(&p.name), dt)),
            _ => None,
        })
        .collect()
}

/// The `library.LIB.call(...)` statement. `ret` decides whether the
/// result is bound to `raw`.
fn call_block(
    indent: &str,
    ret: &Ty,
    symbol: &str,
    args: &[String],
    slices: &[(String, Dtype)],
    handle: bool,
) -> String {
    let assign = if matches!(ret, Ty::Unit) {
        ""
    } else {
        "raw = "
    };
    let mut out = format!("{indent}{assign}library.LIB.call(\n");
    out.push_str(&format!("{indent}    \"{symbol}\",\n"));
    out.push_str(&args_dict(args, &format!("{indent}    ")));
    if !slices.is_empty() {
        let rendered: Vec<String> = slices
            .iter()
            .map(|(name, dt)| format!("({name}, \"{}\")", dt.wire_name()))
            .collect();
        let trailing = if rendered.len() == 1 { "," } else { "" };
        out.push_str(&format!(
            "{indent}    slices=({}{trailing}),\n",
            rendered.join(", ")
        ));
    }
    if handle {
        out.push_str(&format!("{indent}    handle=self.handle,\n"));
    }
    out.push_str(&format!("{indent})\n"));
    out
}

/// A dict literal line (or block) at `indent`, with the trailing comma.
fn args_dict(entries: &[String], indent: &str) -> String {
    if entries.is_empty() {
        return format!("{indent}{{}},\n");
    }
    let one = format!("{indent}{{{}}},", entries.join(", "));
    if one.len() <= LINE_LIMIT {
        return one + "\n";
    }
    let mut out = format!("{indent}{{\n");
    for e in entries {
        out.push_str(&format!("{indent}    {e},\n"));
    }
    out.push_str(&format!("{indent}}},\n"));
    out
}

/// The statements that turn `raw` into the typed return value.
fn return_stmts(
    m: &Manifest,
    ret: &Ty,
    indent: &str,
    model_imports: &mut BTreeSet<String>,
) -> String {
    if matches!(ret, Ty::Unit) {
        return String::new();
    }
    // A direct data-enum return gets a readable variant-dispatch block
    // instead of one very long expression.
    if let Ty::Ref { name } = ret {
        if let Some(TypeDecl::Enum { tag, variants, .. }) = find_type(m, name) {
            let mut out = format!("{indent}variants = {{\n");
            for v in variants {
                model_imports.insert(format!("{name}{}", v.name));
                out.push_str(&format!(
                    "{indent}    \"{}\": {name}{},\n",
                    v.wire_name, v.name
                ));
            }
            out.push_str(&format!("{indent}}}\n"));
            out.push_str(&format!(
                "{indent}return variants[raw[\"{}\"]].model_validate(raw)\n",
                tag
            ));
            return out;
        }
    }
    match py_conv("raw", ret, m, Dir::Validate, 0) {
        Some(expr) => {
            record_conv_imports(m, ret, model_imports);
            format!("{indent}return {expr}\n")
        }
        None => format!("{indent}return raw\n"),
    }
}

/// Refs that a validate-direction conversion mentions by class name.
fn record_conv_imports(m: &Manifest, ty: &Ty, model_imports: &mut BTreeSet<String>) {
    let mut refs = BTreeSet::new();
    collect_refs(ty, &mut refs);
    for name in refs {
        if let Some(TypeDecl::Enum { variants, .. }) = find_type(m, &name) {
            for v in variants {
                model_imports.insert(format!("{name}{}", v.name));
            }
        }
    }
}

/// Direction of a conversion between wire JSON and typed Python values.
#[derive(Clone, Copy)]
enum Dir {
    /// Typed value → JSON-serializable (argument encoding).
    Dump,
    /// Decoded JSON → typed value (return decoding).
    Validate,
}

/// The conversion expression for `expr: ty`, or `None` when the value
/// passes through unchanged (scalars, strings, buffers, `Json`, string
/// enums on dump).
fn py_conv(expr: &str, ty: &Ty, m: &Manifest, dir: Dir, depth: usize) -> Option<String> {
    match ty {
        Ty::Ref { name } => match (find_type(m, name), dir) {
            (Some(TypeDecl::Struct { .. }), Dir::Dump) => {
                Some(format!("{expr}.model_dump(by_alias=True, mode=\"json\")"))
            }
            (Some(TypeDecl::Enum { .. }), Dir::Dump) => {
                Some(format!("{expr}.model_dump(by_alias=True, mode=\"json\")"))
            }
            (Some(TypeDecl::StringEnum { .. }), Dir::Dump) => None,
            (Some(TypeDecl::Struct { .. }), Dir::Validate) => {
                Some(format!("{name}.model_validate({expr})"))
            }
            (Some(TypeDecl::Enum { tag, variants, .. }), Dir::Validate) => {
                let pairs: Vec<String> = variants
                    .iter()
                    .map(|v| format!("\"{}\": {name}{}", v.wire_name, v.name))
                    .collect();
                Some(format!(
                    "{{{}}}[{expr}[\"{tag}\"]].model_validate({expr})",
                    pairs.join(", ")
                ))
            }
            (Some(TypeDecl::StringEnum { .. }), Dir::Validate) => Some(format!("{name}({expr})")),
            _ => None,
        },
        Ty::Option { inner } => py_conv(expr, inner, m, dir, depth)
            .map(|conv| format!("None if {expr} is None else {conv}")),
        Ty::List { inner } => {
            let var = if depth == 0 {
                "item".to_string()
            } else {
                format!("item{depth}")
            };
            py_conv(&var, inner, m, dir, depth + 1)
                .map(|conv| format!("[{conv} for {var} in {expr}]"))
        }
        Ty::Map { value } => {
            let (k, v) = if depth == 0 {
                ("key".to_string(), "value".to_string())
            } else {
                (format!("key{depth}"), format!("value{depth}"))
            };
            py_conv(&v, value, m, dir, depth + 1)
                .map(|conv| format!("{{{k}: {conv} for {k}, {v} in {expr}.items()}}"))
        }
        _ => None,
    }
}

/// `from {module} import a, b, c`, parenthesized past the line limit.
fn wrap_from_import(module: &str, names: &[String]) -> String {
    let one = format!("from {module} import {}", names.join(", "));
    if one.len() <= LINE_LIMIT {
        return one + "\n";
    }
    let mut out = format!("from {module} import (\n");
    for name in names {
        out.push_str(&format!("    {name},\n"));
    }
    out.push_str(")\n");
    out
}

#[cfg(test)]
mod tests {
    use super::super::test_manifest::{FOREIGN_ORIGIN, manifest, manifest_hash};
    use super::super::util::VERSION;
    use super::*;

    fn no_imports() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    fn mapped_imports() -> BTreeMap<String, String> {
        [(
            FOREIGN_ORIGIN.to_string(),
            "example.catalog.generated".to_string(),
        )]
        .into_iter()
        .collect()
    }

    fn emit_with(imports: &BTreeMap<String, String>) -> Vec<(&'static str, String)> {
        let m = manifest();
        let hash = manifest_hash(&m);
        emit(
            &m,
            &hash,
            &[
                "../../../rust/target/debug".to_string(),
                "../../../rust/target/release".to_string(),
            ],
            imports,
        )
    }

    fn emitted(file: &str) -> String {
        emit_with(&no_imports())
            .into_iter()
            .find(|(name, _)| *name == file)
            .expect("file exists")
            .1
    }

    fn golden(body: &str) -> String {
        let hash = manifest_hash(&manifest());
        body.replace("@VERSION@", VERSION).replace("@HASH@", &hash)
    }

    fn assert_text_eq(actual: &str, expected: &str, file: &str) {
        if actual != expected {
            let diff = similar::TextDiff::from_lines(expected, actual);
            panic!(
                "{file} does not match its golden:\n{}",
                diff.unified_diff()
                    .context_radius(3)
                    .header("expected", "actual")
            );
        }
    }

    #[test]
    fn models_py_matches_golden() {
        let expected = golden(
            r#"# Code generated by rspyts v@VERSION@. DO NOT EDIT.
# rspyts:manifest-hash sha256:@HASH@
"""
Data models bridged from `demo-crate`.
"""

from enum import StrEnum
from typing import Annotated, Any, Literal

from pydantic import Field
from rspyts import Contract


class AnalysisParams(Contract):
    """
    Parameters controlling the analysis pass.
    """

    min_duration_s: float
    threshold: float | None = None
    metadata: Any


class CatalogInfo(Contract):
    """
    Catalog description reported by the device.
    """

    vendor: str
    channel_count: int = Field(ge=0, le=65535)


class Severity(StrEnum):
    LOW = "low"
    MEDIUM = "medium"
    HIGH = "high"


class ThresholdEventCrossed(Contract):
    kind: Literal["crossed"] = "crossed"
    at_sample: int = Field(ge=0, le=4294967295)
    value: float


class ThresholdEventCleared(Contract):
    kind: Literal["cleared"] = "cleared"
    at_sample: int = Field(ge=0, le=4294967295)


ThresholdEvent = Annotated[
    ThresholdEventCrossed | ThresholdEventCleared,
    Field(discriminator="kind"),
]
"""
Signal threshold transitions.
"""
"#,
        );
        assert_text_eq(&emitted("models.py"), &expected, "models.py");
    }

    #[test]
    fn constants_py_matches_golden() {
        let expected = golden(
            r#"# Code generated by rspyts v@VERSION@. DO NOT EDIT.
# rspyts:manifest-hash sha256:@HASH@
"""
Constants bridged from `demo-crate`.
"""

from typing import Final

from .models import AnalysisParams

# Baseline analysis parameters.
DEFAULT_PARAMS: Final[AnalysisParams] = AnalysisParams.model_validate(
    {"minDurationS": 0.5, "threshold": None, "metadata": {"rev": 2}}
)

DEFAULT_THRESHOLD: Final[float] = 0.75

# Name reported by the analysis engine.
ENGINE_NAME: Final[str] = "neuro-engine"

SUPPORTED_UNITS: Final[list[str]] = ["uV", "mV"]
"#,
        );
        assert_text_eq(&emitted("constants.py"), &expected, "constants.py");
    }

    #[test]
    fn errors_py_matches_golden() {
        let expected = golden(
            r#"# Code generated by rspyts v@VERSION@. DO NOT EDIT.
# rspyts:manifest-hash sha256:@HASH@
"""
Exception classes bridged from `demo-crate`.
"""

import rspyts


class AnalysisError(rspyts.BridgeError): ...


class AnalysisErrorInvalidSampleRate(AnalysisError):  # noqa: N818
    """
    The sample rate must be positive.
    """


class AnalysisErrorWindowTooLarge(AnalysisError): ...  # noqa: N818  # .data: {"max": int}


rspyts.register_error("invalidSampleRate", AnalysisErrorInvalidSampleRate)
rspyts.register_error("windowTooLarge", AnalysisErrorWindowTooLarge)
"#,
        );
        assert_text_eq(&emitted("errors.py"), &expected, "errors.py");
    }

    #[test]
    fn library_py_matches_golden() {
        let expected = golden(
            r#"# Code generated by rspyts v@VERSION@. DO NOT EDIT.
# rspyts:manifest-hash sha256:@HASH@
"""
Loader for the compiled `demo-crate` bridge library.
"""

from pathlib import Path

from rspyts import Library

LIB = Library(
    name="demo_crate",
    search=[
        "../../../rust/target/debug",
        "../../../rust/target/release",
    ],
    anchor=Path(__file__).parent,
)
"#,
        );
        assert_text_eq(&emitted("library.py"), &expected, "library.py");
    }

    #[test]
    fn functions_py_matches_golden() {
        // `render_report` is TypeScript-only: it must not appear here.
        let expected = golden(
            r#"# Code generated by rspyts v@VERSION@. DO NOT EDIT.
# rspyts:manifest-hash sha256:@HASH@
"""
Typed wrappers for bridged free functions.
"""

import numpy as np

from . import errors, library  # noqa: F401
from .models import AnalysisParams


def analyze_signal(samples: np.ndarray, sample_rate: int, params: AnalysisParams) -> np.ndarray:
    """
    Analyze a signal buffer.
    """
    raw = library.LIB.call(
        "rspyts_fn__analyze_signal",
        {"sampleRate": sample_rate, "params": params.model_dump(by_alias=True, mode="json")},
        slices=((samples, "f64"),),
    )
    return raw
"#,
        );
        assert_text_eq(&emitted("functions.py"), &expected, "functions.py");
    }

    #[test]
    fn classes_py_matches_golden() {
        let expected = golden(
            r#"# Code generated by rspyts v@VERSION@. DO NOT EDIT.
# rspyts:manifest-hash sha256:@HASH@
"""
Handle classes for bridged Rust objects.
"""

from typing import Self

import numpy as np

from . import errors, library  # noqa: F401
from .models import AnalysisParams, CatalogInfo


class Recording:
    """
    A recorded session backed by a native handle.
    """

    handle: int

    def __init__(self) -> None:
        raise TypeError("Recording cannot be constructed directly; use Recording.open(...)")

    @classmethod
    def open(cls, path: str) -> Self:
        """
        Open a recording from disk.
        """
        raw = library.LIB.call(
            "rspyts_cls__Recording__open",
            {"path": path},
        )
        obj = cls.__new__(cls)
        obj.handle = raw
        return obj

    @staticmethod
    def default_extension() -> str:
        raw = library.LIB.call(
            "rspyts_cls__Recording__default_extension",
            {},
        )
        return raw

    def duration_s(self) -> float:
        """
        Total duration in seconds.
        """
        raw = library.LIB.call(
            "rspyts_cls__Recording__duration_s",
            {},
            handle=self.handle,
        )
        return raw

    def info(self) -> CatalogInfo:
        raw = library.LIB.call(
            "rspyts_cls__Recording__info",
            {},
            handle=self.handle,
        )
        return CatalogInfo.model_validate(raw)

    def preload(self) -> None:
        """
        Eagerly load samples into memory.
        """
        library.LIB.call(
            "rspyts_cls__Recording__preload",
            {},
            handle=self.handle,
        )

    def close(self) -> None:
        """
        Release the underlying Rust object. Safe to call more than once.
        """
        library.LIB.call_drop("rspyts_cls__Recording__drop", self.handle)

    def __enter__(self) -> Self:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    def __del__(self) -> None:
        try:
            self.close()
        except Exception:
            pass


class RunningStats:
    """
    Streaming statistics over a sliding window.
    """

    handle: int

    def __init__(self, window: int) -> None:
        raw = library.LIB.call(
            "rspyts_cls__RunningStats__new",
            {"window": window},
        )
        self.handle = raw

    @classmethod
    def resumed(cls, state: AnalysisParams) -> Self:
        """
        Rebuild from a snapshot.
        """
        raw = library.LIB.call(
            "rspyts_cls__RunningStats__resumed",
            {"state": state.model_dump(by_alias=True, mode="json")},
        )
        obj = cls.__new__(cls)
        obj.handle = raw
        return obj

    def push(self, chunk: np.ndarray) -> None:
        library.LIB.call(
            "rspyts_cls__RunningStats__push",
            {},
            slices=((chunk, "f64"),),
            handle=self.handle,
        )

    def snapshot(self) -> AnalysisParams:
        """
        Snapshot current state.
        """
        raw = library.LIB.call(
            "rspyts_cls__RunningStats__snapshot",
            {},
            handle=self.handle,
        )
        return AnalysisParams.model_validate(raw)

    def close(self) -> None:
        """
        Release the underlying Rust object. Safe to call more than once.
        """
        library.LIB.call_drop("rspyts_cls__RunningStats__drop", self.handle)

    def __enter__(self) -> Self:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    def __del__(self) -> None:
        try:
            self.close()
        except Exception:
            pass
"#,
        );
        assert_text_eq(&emitted("classes.py"), &expected, "classes.py");
    }

    #[test]
    fn init_py_matches_golden() {
        let expected = golden(
            r#"# Code generated by rspyts v@VERSION@. DO NOT EDIT.
# rspyts:manifest-hash sha256:@HASH@
"""
Generated bridge surface for `demo-crate`.
"""

from .classes import Recording, RunningStats
from .constants import DEFAULT_PARAMS, DEFAULT_THRESHOLD, ENGINE_NAME, SUPPORTED_UNITS
from .errors import AnalysisError, AnalysisErrorInvalidSampleRate, AnalysisErrorWindowTooLarge
from .functions import analyze_signal
from .models import (
    AnalysisParams,
    CatalogInfo,
    Severity,
    ThresholdEvent,
    ThresholdEventCleared,
    ThresholdEventCrossed,
)

__all__ = [
    "DEFAULT_PARAMS",
    "DEFAULT_THRESHOLD",
    "ENGINE_NAME",
    "SUPPORTED_UNITS",
    "AnalysisError",
    "AnalysisErrorInvalidSampleRate",
    "AnalysisErrorWindowTooLarge",
    "AnalysisParams",
    "CatalogInfo",
    "Recording",
    "RunningStats",
    "Severity",
    "ThresholdEvent",
    "ThresholdEventCleared",
    "ThresholdEventCrossed",
    "analyze_signal",
]
"#,
        );
        assert_text_eq(&emitted("__init__.py"), &expected, "__init__.py");
    }

    #[test]
    fn mapped_foreign_types_are_imported_not_emitted() {
        let files = emit_with(&mapped_imports());
        let models = &files.iter().find(|(n, _)| *n == "models.py").unwrap().1;
        // The class body is gone; the mapped import (kept for __init__
        // re-export, hence the noqa) replaces it.
        assert!(
            models.contains(
                "from example.catalog.generated import CatalogInfo  # noqa: F401\n"
            ),
            "{models}"
        );
        assert!(!models.contains("class CatalogInfo"), "{models}");

        // __init__ re-exports it exactly like a local model.
        let init = &files.iter().find(|(n, _)| *n == "__init__.py").unwrap().1;
        assert!(init.contains("CatalogInfo"), "{init}");

        // classes.py keeps importing it via .models, which re-exports.
        let classes = &files.iter().find(|(n, _)| *n == "classes.py").unwrap().1;
        assert!(
            classes.contains("from .models import AnalysisParams, CatalogInfo"),
            "{classes}"
        );
    }

    #[test]
    fn unmapped_foreign_types_are_emitted_locally() {
        // No [python.imports] entry: origin is ignored, output stays
        // self-contained.
        let models = emitted("models.py");
        assert!(models.contains("class CatalogInfo(Contract):"), "{models}");
        assert!(!models.contains("example"), "{models}");
    }

    #[test]
    fn mapped_foreign_error_enums_are_imported_not_emitted() {
        let mut m = manifest();
        if let TypeDecl::ErrorEnum { origin, .. } = &mut m.types[0] {
            *origin = FOREIGN_ORIGIN.to_string();
        } else {
            panic!("types[0] should be the error enum");
        }
        let hash = manifest_hash(&m);
        let files = emit(&m, &hash, &[], &mapped_imports());
        let errors = &files.iter().find(|(n, _)| *n == "errors.py").unwrap().1;
        assert!(
            errors.contains(
                "from example.catalog.generated import AnalysisError, \
                 AnalysisErrorInvalidSampleRate, AnalysisErrorWindowTooLarge  # noqa: F401\n"
            ),
            "{errors}"
        );
        // The foreign package registers its own error codes on import.
        assert!(!errors.contains("class AnalysisError"), "{errors}");
        assert!(!errors.contains("register_error"), "{errors}");
        assert!(!errors.contains("import rspyts"), "{errors}");
    }

    #[test]
    fn foreign_types_referenced_by_local_models_need_no_noqa() {
        let mut m = manifest();
        if let TypeDecl::Struct { fields, .. } = &mut m.types[1] {
            fields.push(FieldDecl {
                name: "catalog".to_string(),
                wire_name: "catalog".to_string(),
                docs: String::new(),
                ty: Ty::Ref {
                    name: "CatalogInfo".to_string(),
                },
                optional: false,
            });
        } else {
            panic!("types[1] should be the struct");
        }
        let hash = manifest_hash(&m);
        let files = emit(&m, &hash, &[], &mapped_imports());
        let models = &files.iter().find(|(n, _)| *n == "models.py").unwrap().1;
        assert!(
            models.contains("from example.catalog.generated import CatalogInfo\n"),
            "{models}"
        );
        assert!(!models.contains("CatalogInfo  # noqa"), "{models}");
    }

    #[test]
    fn py_json_renders_python_literals() {
        use serde_json::json;
        assert_eq!(py_json(&json!(null)), "None");
        assert_eq!(py_json(&json!(true)), "True");
        assert_eq!(py_json(&json!(false)), "False");
        assert_eq!(py_json(&json!(-3)), "-3");
        assert_eq!(py_json(&json!(0.75)), "0.75");
        assert_eq!(py_json(&json!("uV \"quoted\"")), r#""uV \"quoted\"""#);
        assert_eq!(
            py_json(&json!({"a": [1, null], "b": {"c": false}})),
            r#"{"a": [1, None], "b": {"c": False}}"#
        );
    }

    #[test]
    fn const_exprs_construct_models_and_enums() {
        use serde_json::json;
        let m = manifest();
        let mut imports = BTreeSet::new();
        // String-enum constants go through the enum constructor.
        assert_eq!(
            py_const_expr(
                &m,
                &Ty::Ref {
                    name: "Severity".into()
                },
                &json!("high"),
                &mut imports
            ),
            "Severity(\"high\")"
        );
        // Data-enum constants validate through the tagged variant class.
        assert_eq!(
            py_const_expr(
                &m,
                &Ty::Ref {
                    name: "ThresholdEvent".into()
                },
                &json!({"kind": "cleared", "atSample": 7}),
                &mut imports
            ),
            "ThresholdEventCleared.model_validate({\"kind\": \"cleared\", \"atSample\": 7})"
        );
        assert!(imports.contains("ThresholdEventCleared"));
        // Lists of refs construct element-wise.
        assert_eq!(
            py_const_expr(
                &m,
                &Ty::List {
                    inner: Box::new(Ty::Ref {
                        name: "Severity".into()
                    })
                },
                &json!(["low", "high"]),
                &mut imports
            ),
            "[Severity(\"low\"), Severity(\"high\")]"
        );
        // Optional constants collapse to None.
        assert_eq!(
            py_const_expr(
                &m,
                &Ty::Option {
                    inner: Box::new(Ty::F64)
                },
                &json!(null),
                &mut imports
            ),
            "None"
        );
    }

    #[test]
    fn split_call_finds_the_wrap_point() {
        assert_eq!(
            split_call("AnalysisParams.model_validate({\"a\": 1})"),
            Some(("AnalysisParams.model_validate", "{\"a\": 1}"))
        );
        assert_eq!(
            split_call("Severity(\"low\")"),
            Some(("Severity", "\"low\""))
        );
        assert_eq!(split_call("[1, 2]"), None);
        assert_eq!(split_call("\"text\""), None);
    }

    #[test]
    fn optional_struct_param_is_null_guarded() {
        let m = manifest();
        let ty = Ty::Option {
            inner: Box::new(Ty::Ref {
                name: "AnalysisParams".into(),
            }),
        };
        assert_eq!(
            py_conv("params", &ty, &m, Dir::Dump, 0).unwrap(),
            "None if params is None else params.model_dump(by_alias=True, mode=\"json\")"
        );
        assert_eq!(
            py_conv("raw", &ty, &m, Dir::Validate, 0).unwrap(),
            "None if raw is None else AnalysisParams.model_validate(raw)"
        );
    }

    #[test]
    fn list_of_structs_round_trips_via_comprehension() {
        let m = manifest();
        let ty = Ty::List {
            inner: Box::new(Ty::Ref {
                name: "AnalysisParams".into(),
            }),
        };
        assert_eq!(
            py_conv("items", &ty, &m, Dir::Dump, 0).unwrap(),
            "[item.model_dump(by_alias=True, mode=\"json\") for item in items]"
        );
        assert_eq!(
            py_conv("raw", &ty, &m, Dir::Validate, 0).unwrap(),
            "[AnalysisParams.model_validate(item) for item in raw]"
        );
    }

    #[test]
    fn module_shadowing_param_binds_with_underscore_and_exact_wire_key() {
        use rspyts_core::ir::{FnDecl, Target};
        let m = manifest();
        let f = FnDecl {
            name: "load".to_string(),
            docs: String::new(),
            params: vec![ParamDecl {
                name: "library".to_string(),
                wire_name: "library".to_string(),
                ty: Ty::String,
            }],
            ret: Ty::Unit,
            err: None,
            targets: Target::all(),
        };
        let mut model_imports = BTreeSet::new();
        let mut uses_np = false;
        let def = function_def(&m, &f, &mut model_imports, &mut uses_np);
        // The binding is escaped so `library.LIB.call` still resolves to
        // the module; the args-dict wire key stays exact.
        assert_eq!(
            def,
            "def load(library_: str) -> None:\n    \
             library.LIB.call(\n        \
             \"rspyts_fn__load\",\n        \
             {\"library\": library_},\n    \
             )\n"
        );
        assert_eq!(py_param_name("errors"), "errors_");
        assert_eq!(py_param_name("models"), "models_");
        assert_eq!(py_param_name("np"), "np_");
        // Keyword escaping still applies, and ordinary names pass through.
        assert_eq!(py_param_name("class"), "class_");
        assert_eq!(py_param_name("values"), "values");
    }

    #[test]
    fn scalars_buffers_and_json_pass_through() {
        let m = manifest();
        assert!(py_conv("x", &Ty::U32, &m, Dir::Dump, 0).is_none());
        assert!(py_conv("x", &Ty::Buf { dt: Dtype::F64 }, &m, Dir::Validate, 0).is_none());
        assert!(py_conv("x", &Ty::Json, &m, Dir::Dump, 0).is_none());
        assert!(py_conv("x", &Ty::Json, &m, Dir::Validate, 0).is_none());
        assert!(
            py_conv(
                "x",
                &Ty::Ref {
                    name: "Severity".into()
                },
                &m,
                Dir::Dump,
                0
            )
            .is_none()
        );
    }

    /// External verification: the rendered fixture package must pass the
    /// same ruff gate the generated-code style contract promises. Skipped
    /// when `uvx` is not installed.
    #[test]
    fn generated_python_passes_ruff() {
        let root = std::env::temp_dir().join(format!("rspyts-ruff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // The package directory itself needs a valid module name (N999).
        let dir = root.join("generated");
        std::fs::create_dir_all(&dir).expect("temp dir");
        for (name, content) in emit_with(&mapped_imports()) {
            // The mapped fixture exercises the foreign-import lines too;
            // ruff only lints the files, so the unresolvable module is fine.
            std::fs::write(dir.join(name), content).expect("write file");
        }
        let result = std::process::Command::new("uvx")
            .args([
                "ruff",
                "check",
                "--isolated",
                "--select",
                "E,F,I,N,UP,RUF",
                "--line-length",
                "120",
            ])
            .arg(&dir)
            .output();
        let output = match result {
            Ok(output) => output,
            Err(_) => {
                eprintln!("skipping: `uvx` is not installed");
                return;
            }
        };
        assert!(
            output.status.success(),
            "ruff rejected the generated package:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
