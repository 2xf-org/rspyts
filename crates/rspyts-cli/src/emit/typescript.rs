//! The TypeScript emitter (codegen.md §5, §8).
//!
//! Produces `client.ts`, `constants.ts`, `errors.ts`, `index.ts`, and
//! `types.ts`. Output targets ES2022, 2-space indent, semicolons,
//! double quotes, `strict` tsc. The surface is camelCase throughout —
//! identical to the wire names, so values need no key mapping at
//! runtime. Types whose origin crate has an entry in
//! `[typescript.imports]` are imported from that module instead of
//! re-emitted (codegen.md §9).

use super::util::{collect_refs, pascal, ts_doc, ts_doc_from_lines, ts_header, ts_type};
use heck::ToLowerCamelCase;
use rspyts_core::ir::{
    ClassDecl, ConstDecl, FieldDecl, FnDecl, Manifest, ParamDecl, StaticDecl, Target, Ty, TypeDecl,
};
use std::collections::{BTreeMap, BTreeSet};

/// Emit all five files: `(file name, content)` pairs, name-sorted.
pub fn emit(
    m: &Manifest,
    hash: &str,
    imports: &BTreeMap<String, String>,
) -> Vec<(&'static str, String)> {
    vec![
        ("client.ts", client_ts(m, hash)),
        ("constants.ts", constants_ts(m, hash)),
        ("errors.ts", errors_ts(m, hash, imports)),
        ("index.ts", index_ts(hash)),
        ("types.ts", types_ts(m, hash, imports)),
    ]
}

/// Is `decl` imported from another crate's generated module instead of
/// emitted here? Foreign-origin types without a mapping stay local, so
/// the output is always self-contained.
fn is_imported(m: &Manifest, decl: &TypeDecl, imports: &BTreeMap<String, String>) -> bool {
    decl.origin() != m.crate_name && imports.contains_key(decl.origin())
}

fn on_ts(targets: &[Target]) -> bool {
    targets.contains(&Target::Typescript)
}

fn ts_error_registry_name(name: &str) -> String {
    let base = name.strip_suffix("Error").unwrap_or(name);
    format!("{base}ErrorTypes")
}

// ----------------------------------------------------------------- types.ts

fn types_ts(m: &Manifest, hash: &str, imports: &BTreeMap<String, String>) -> String {
    let mut out = ts_header(hash);
    let mut any = false;

    // Imported foreign data types: import for local use, re-export so
    // `index.ts`'s `export * from "./types"` surfaces them.
    let mut by_module: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for decl in &m.types {
        if matches!(decl, TypeDecl::ErrorEnum { .. }) || !is_imported(m, decl, imports) {
            continue;
        }
        by_module
            .entry(imports[decl.origin()].as_str())
            .or_default()
            .push(decl.name());
    }
    if !by_module.is_empty() {
        out.push('\n');
        let mut all_names: Vec<&str> = Vec::new();
        for (module, mut names) in by_module {
            names.sort_unstable();
            out.push_str(&format!(
                "import type {{ {} }} from {};\n",
                names.join(", "),
                ts_string(module)
            ));
            all_names.extend(names);
        }
        all_names.sort_unstable();
        out.push('\n');
        out.push_str(&format!("export type {{ {} }};\n", all_names.join(", ")));
        any = true;
    }

    for decl in &m.types {
        if is_imported(m, decl, imports) {
            continue;
        }
        match decl {
            TypeDecl::Newtype {
                name, docs, inner, ..
            } => {
                out.push('\n');
                out.push_str(&ts_doc(docs, ""));
                out.push_str(&format!("export type {name} = {};\n", ts_type(inner)));
            }
            TypeDecl::Struct {
                name, docs, fields, ..
            } => {
                out.push('\n');
                out.push_str(&ts_doc(docs, ""));
                out.push_str(&format!("export interface {name} {{\n"));
                for f in fields {
                    out.push_str(&ts_doc(&f.docs, "  "));
                    out.push_str(&format!(
                        "  {};\n",
                        ts_field(&f.wire_name, &f.ty, f.optional)
                    ));
                }
                out.push_str("}\n");
            }
            TypeDecl::StringEnum {
                name,
                docs,
                variants,
                ..
            } => {
                out.push('\n');
                out.push_str(&ts_doc(docs, ""));
                let literals: Vec<String> =
                    variants.iter().map(|v| ts_string(&v.wire_name)).collect();
                out.push_str(&format!("export type {name} = {};\n", literals.join(" | ")));
            }
            TypeDecl::Enum {
                name,
                docs,
                tag,
                variants,
                ..
            } => {
                out.push('\n');
                out.push_str(&ts_doc(docs, ""));
                out.push_str(&format!("export type {name} ="));
                for v in variants {
                    let mut members =
                        vec![format!("{}: {}", ts_prop(tag), ts_string(&v.wire_name))];
                    members.extend(
                        v.fields
                            .iter()
                            .map(|f| ts_field(&f.wire_name, &f.ty, f.optional)),
                    );
                    out.push_str(&format!("\n  | {{ {} }}", members.join("; ")));
                }
                out.push_str(";\n");
            }
            TypeDecl::ErrorEnum { .. } => continue,
        }
        any = true;
    }
    if !any {
        out.push_str("\nexport {};\n");
    }
    out
}

/// `name: T`, or `name?: Inner | null` for optional fields.
fn ts_field(wire_name: &str, ty: &Ty, optional: bool) -> String {
    let name = ts_prop(wire_name);
    match ty {
        Ty::Option { inner } => format!("{name}?: {} | null", ts_type(inner)),
        _ if optional => format!("{name}?: {}", ts_type(ty)),
        _ => format!("{name}: {}", ts_type(ty)),
    }
}

fn ts_string(value: &str) -> String {
    serde_json::to_string(value).expect("serializing a string cannot fail")
}

fn ts_prop(value: &str) -> String {
    if is_ts_ident(value) {
        value.to_string()
    } else {
        ts_string(value)
    }
}

fn is_ts_ident(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

// -------------------------------------------------------------- constants.ts

fn constants_ts(m: &Manifest, hash: &str) -> String {
    let mut out = ts_header(hash);
    if m.constants.is_empty() {
        out.push_str("\nexport {};\n");
        return out;
    }
    for c in &m.constants {
        out.push('\n');
        out.push_str(&ts_doc(&c.docs, ""));
        out.push_str(&const_line(m, c));
    }
    out
}

/// One `export const NAME = <literal> as const;` line. `as const` is
/// skipped for `null`, which cannot take a const assertion.
fn const_line(m: &Manifest, c: &ConstDecl) -> String {
    let literal = ts_const_expr(m, &c.ty, &c.value);
    if c.value.is_null() {
        format!("export const {} = null;\n", c.name)
    } else {
        format!("export const {} = {literal} as const;\n", c.name)
    }
}

fn ts_const_expr(m: &Manifest, ty: &Ty, value: &serde_json::Value) -> String {
    match ty {
        Ty::Json => value
            .get("__rspyts_json__")
            .map(ts_json)
            .unwrap_or_else(|| ts_json(value)),
        Ty::I64 | Ty::U64 => value
            .as_str()
            .map(|value| format!("{value}n"))
            .unwrap_or_else(|| ts_json(value)),
        Ty::Option { inner } => {
            if value.is_null() {
                "null".to_string()
            } else {
                ts_const_expr(m, inner, value)
            }
        }
        Ty::List { inner } => value.as_array().map_or_else(
            || ts_json(value),
            |items| {
                format!(
                    "[{}]",
                    items
                        .iter()
                        .map(|item| ts_const_expr(m, inner, item))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
        ),
        Ty::Map { value: value_ty } => value.as_object().map_or_else(
            || ts_json(value),
            |entries| {
                let rendered = entries
                    .iter()
                    .map(|(key, value)| {
                        let key = if is_ts_ident(key) {
                            key.clone()
                        } else {
                            serde_json::to_string(key).expect("key serializes")
                        };
                        format!("{key}: {}", ts_const_expr(m, value_ty, value))
                    })
                    .collect::<Vec<_>>();
                format!("{{ {} }}", rendered.join(", "))
            },
        ),
        Ty::Tuple { items } => value.as_array().map_or_else(
            || ts_json(value),
            |values| {
                format!(
                    "[{}]",
                    items
                        .iter()
                        .zip(values)
                        .map(|(ty, value)| ts_const_expr(m, ty, value))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
        ),
        Ty::Ref { name } => match super::util::find_type(m, name) {
            Some(TypeDecl::Newtype { inner, .. }) => ts_const_expr(m, inner, value),
            Some(TypeDecl::Struct { fields, .. }) => ts_const_object(m, fields, value, None),
            Some(TypeDecl::Enum { tag, variants, .. }) => {
                let fields = value
                    .get(tag)
                    .and_then(serde_json::Value::as_str)
                    .and_then(|wire| variants.iter().find(|variant| variant.wire_name == wire))
                    .map(|variant| variant.fields.as_slice())
                    .unwrap_or(&[]);
                ts_const_object(m, fields, value, Some(tag))
            }
            _ => ts_json(value),
        },
        _ => ts_json(value),
    }
}

fn ts_const_object(
    m: &Manifest,
    fields: &[FieldDecl],
    value: &serde_json::Value,
    tag: Option<&str>,
) -> String {
    let Some(entries) = value.as_object() else {
        return ts_json(value);
    };
    let rendered = entries
        .iter()
        .map(|(key, value)| {
            let key_expr = if is_ts_ident(key) {
                key.clone()
            } else {
                serde_json::to_string(key).expect("key serializes")
            };
            let rendered = if tag == Some(key.as_str()) {
                ts_json(value)
            } else if let Some(field) = fields.iter().find(|field| field.wire_name == *key) {
                ts_const_expr(m, &field.ty, value)
            } else {
                ts_json(value)
            };
            format!("{key_expr}: {rendered}")
        })
        .collect::<Vec<_>>();
    format!("{{ {} }}", rendered.join(", "))
}

/// A JSON value as a TypeScript literal expression.
fn ts_json(value: &serde_json::Value) -> String {
    use serde_json::Value;
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => serde_json::to_string(s).expect("string serializes"),
        Value::Array(items) => {
            let rendered: Vec<String> = items.iter().map(ts_json).collect();
            format!("[{}]", rendered.join(", "))
        }
        Value::Object(entries) => {
            if entries.is_empty() {
                return "{}".to_string();
            }
            let rendered: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    let key = if is_ts_ident(k) {
                        k.clone()
                    } else {
                        serde_json::to_string(k).expect("key serializes")
                    };
                    format!("{key}: {}", ts_json(v))
                })
                .collect();
            format!("{{ {} }}", rendered.join(", "))
        }
    }
}

// ---------------------------------------------------------------- errors.ts

fn errors_ts(m: &Manifest, hash: &str, imports: &BTreeMap<String, String>) -> String {
    let mut local: Vec<(&String, &String, &Vec<rspyts_core::ir::ErrorVariantDecl>)> = Vec::new();
    let mut foreign: BTreeMap<&str, Vec<String>> = BTreeMap::new();
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
            let names = foreign.entry(imports[t.origin()].as_str()).or_default();
            names.push(name.clone());
            names.extend(variants.iter().map(|v| format!("{name}{}", v.name)));
            names.push(ts_error_registry_name(name));
        } else {
            local.push((name, docs, variants));
        }
    }

    let mut out = ts_header(hash);
    if local.is_empty() && foreign.is_empty() {
        out.push_str("\nexport {};\n");
        return out;
    }

    out.push('\n');
    if !local.is_empty() {
        let mut runtime = vec!["type BridgeErrorRegistry", "RspytsError"];
        if local.iter().any(|(_, _, variants)| {
            variants.iter().any(|variant| {
                variant
                    .fields
                    .iter()
                    .any(|field| contains_kind(m, &field.ty, ConversionKind::Float))
            })
        }) {
            runtime.push("floatFromWire");
        }
        if local.iter().any(|(_, _, variants)| {
            variants.iter().any(|variant| {
                variant
                    .fields
                    .iter()
                    .any(|field| contains_kind(m, &field.ty, ConversionKind::Json))
            })
        }) {
            runtime.push("jsonFromWire");
        }
        if local.iter().any(|(_, _, variants)| {
            variants.iter().any(|variant| {
                variant
                    .fields
                    .iter()
                    .any(|field| contains_kind(m, &field.ty, ConversionKind::I64))
            })
        }) {
            runtime.push("i64FromWire");
        }
        if local.iter().any(|(_, _, variants)| {
            variants.iter().any(|variant| {
                variant
                    .fields
                    .iter()
                    .any(|field| contains_kind(m, &field.ty, ConversionKind::U64))
            })
        }) {
            runtime.push("u64FromWire");
        }
        out.push_str(&format!(
            "import {{ {} }} from \"rspyts\";\n",
            runtime.join(", ")
        ));
    }
    // Value imports include the foreign error classes and their scoped map.
    if !foreign.is_empty() {
        let mut all_names: Vec<String> = Vec::new();
        for (module, mut names) in foreign {
            names.sort();
            out.push_str(&format!(
                "import {{ {} }} from {};\n",
                names.join(", "),
                ts_string(module)
            ));
            all_names.extend(names);
        }
        all_names.sort();
        out.push('\n');
        out.push_str(&format!("export {{ {} }};\n", all_names.join(", ")));
    }

    for (name, docs, variants) in &local {
        out.push('\n');
        out.push_str(&ts_doc(docs, ""));
        out.push_str(&format!("export class {name} extends RspytsError {{}}\n"));
        for v in *variants {
            let mut lines: Vec<String> = super::util::doc_lines(&v.docs)
                .into_iter()
                .map(str::to_string)
                .collect();
            if !v.fields.is_empty() {
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                let entries: Vec<String> = v
                    .fields
                    .iter()
                    .map(|f| format!("{}: {}", ts_prop(&f.wire_name), ts_type(&f.ty)))
                    .collect();
                lines.push(format!("`.data`: {{ {} }}", entries.join("; ")));
            }
            out.push('\n');
            out.push_str(&ts_doc_from_lines(&lines, ""));
            // The registry contract (BridgeErrorConstructor) bakes the code
            // into the subclass: throw sites pass only (message, data).
            let data = ts_error_data_expr(m, &v.fields);
            out.push_str(&format!(
                "export class {name}{} extends {name} {{\n  constructor(message: string, data?: unknown) {{\n    super(message, {}, {data});\n  }}\n}}\n",
                v.name,
                ts_string(&v.wire_code),
            ));
        }
    }
    for (name, _, variants) in &local {
        out.push('\n');
        out.push_str(&format!(
            "export const {} = {{\n",
            ts_error_registry_name(name)
        ));
        for v in *variants {
            out.push_str(&format!(
                "  {}: {name}{},\n",
                ts_string(&v.wire_code),
                v.name
            ));
        }
        out.push_str("} as const satisfies BridgeErrorRegistry;\n");
    }
    out
}

fn ts_error_data_expr(m: &Manifest, fields: &[FieldDecl]) -> String {
    let converted = ts_host_converted_fields("value", fields, m, 0, HostContext::Error);
    if converted.is_empty() {
        return "data".to_string();
    }
    let wire = format!(
        "{{ {} }}",
        fields
            .iter()
            .map(|field| ts_wire_field(field, m))
            .collect::<Vec<_>>()
            .join("; ")
    );
    format!(
        "data === undefined ? undefined : ((value: {wire}) => ({{ ...value, {} }}))(data as {wire})",
        converted.join(", ")
    )
}

// ---------------------------------------------------------------- client.ts

fn client_ts(m: &Manifest, hash: &str) -> String {
    let client_name = format!("{}Client", pascal(&m.crate_name));
    let fns: Vec<&FnDecl> = m.functions.iter().filter(|f| on_ts(&f.targets)).collect();
    let has_errors = fns.iter().any(|function| function.err.is_some())
        || m.classes.iter().any(|class| {
            class
                .constructor
                .as_ref()
                .is_some_and(|constructor| constructor.err.is_some())
                || class
                    .methods
                    .iter()
                    .any(|method| on_ts(&method.targets) && method.err.is_some())
                || class.statics.iter().any(|static_method| {
                    on_ts(&static_method.targets) && static_method.err.is_some()
                })
        });
    let has_calls = !fns.is_empty() || !m.classes.is_empty();
    let has_classes = !m.classes.is_empty();
    // Any class whose instances are made from a raw handle (factories,
    // or a missing constructor) routes construction through INTERNAL.
    let needs_internal = m.classes.iter().any(|c| {
        c.constructor.is_none()
            || c.statics
                .iter()
                .any(|s| s.returns_self && on_ts(&s.targets))
    });

    // Named types referenced anywhere in the TS-visible callable surface.
    let mut type_imports: BTreeSet<String> = BTreeSet::new();
    let mut all_params: Vec<&ParamDecl> = Vec::new();
    let mut all_rets: Vec<&Ty> = Vec::new();
    for f in &fns {
        all_params.extend(f.params.iter());
        all_rets.push(&f.ret);
    }
    for c in &m.classes {
        if let Some(ctor) = &c.constructor {
            all_params.extend(ctor.params.iter());
        }
        for method in c.methods.iter().filter(|method| on_ts(&method.targets)) {
            all_params.extend(method.params.iter());
            all_rets.push(&method.ret);
        }
        for s in c.statics.iter().filter(|s| on_ts(&s.targets)) {
            all_params.extend(s.params.iter());
            if !s.returns_self {
                all_rets.push(&s.ret);
            }
        }
    }
    for p in &all_params {
        collect_refs(&p.ty, &mut type_imports);
    }
    for r in &all_rets {
        collect_refs(r, &mut type_imports);
    }

    let mut out = ts_header(hash);
    out.push('\n');
    let mut rspyts_names = vec!["type BridgeModule".to_string()];
    if has_classes {
        rspyts_names.push("callDrop".to_string());
    }
    if has_calls {
        rspyts_names.push("callFn".to_string());
    }
    if all_params
        .iter()
        .any(|param| contains_kind(m, &param.ty, ConversionKind::I64))
    {
        rspyts_names.push("i64ToWire".to_string());
    }
    if all_rets
        .iter()
        .any(|ret| contains_kind(m, ret, ConversionKind::I64))
    {
        rspyts_names.push("i64FromWire".to_string());
    }
    if all_params
        .iter()
        .any(|param| contains_kind(m, &param.ty, ConversionKind::U64))
    {
        rspyts_names.push("u64ToWire".to_string());
    }
    if all_rets
        .iter()
        .any(|ret| contains_kind(m, ret, ConversionKind::U64))
    {
        rspyts_names.push("u64FromWire".to_string());
    }
    if all_rets
        .iter()
        .any(|ret| contains_kind(m, ret, ConversionKind::Float))
    {
        rspyts_names.push("floatFromWire".to_string());
    }
    if all_params
        .iter()
        .any(|param| contains_kind(m, &param.ty, ConversionKind::Buf))
    {
        rspyts_names.push("wireBuffer".to_string());
    }
    out.push_str(&format!(
        "import {{ {} }} from \"rspyts\";\n",
        rspyts_names.join(", ")
    ));
    if !type_imports.is_empty() {
        let names: Vec<String> = type_imports.into_iter().collect();
        out.push_str(&format!(
            "import type {{ {} }} from \"./types\";\n",
            names.join(", ")
        ));
    }
    if has_errors {
        out.push_str("import * as errors from \"./errors\";\n");
    }

    // Instance interfaces for handle classes.
    for class in &m.classes {
        out.push('\n');
        out.push_str(&ts_doc(&class.docs, ""));
        out.push_str(&format!("export interface {} {{\n", class.name));
        for method in class.methods.iter().filter(|method| on_ts(&method.targets)) {
            out.push_str(&ts_doc(&method.docs, "  "));
            out.push_str(&format!(
                "  {}({}): {};\n",
                method.name.to_lower_camel_case(),
                ts_params(&method.params),
                ts_type(&method.ret)
            ));
        }
        out.push_str("  /** Release the underlying Rust object. Safe to call more than once. */\n");
        out.push_str("  free(): void;\n");
        out.push_str("  [Symbol.dispose](): void;\n");
        out.push_str("}\n");
    }

    // The client interface.
    out.push('\n');
    out.push_str(&format!(
        "/** The typed surface of the `{}` bridge module. */\n",
        m.crate_name
    ));
    out.push_str(&format!("export interface {client_name} {{\n"));
    for f in &fns {
        out.push_str(&ts_doc(&f.docs, "  "));
        out.push_str(&format!(
            "  {}({}): {};\n",
            f.name.to_lower_camel_case(),
            ts_params(&f.params),
            ts_type(&f.ret)
        ));
    }
    for class in &m.classes {
        let statics: Vec<&StaticDecl> =
            class.statics.iter().filter(|s| on_ts(&s.targets)).collect();
        match (&class.constructor, statics.as_slice()) {
            (Some(ctor), []) => {
                out.push_str(&ts_doc(&ctor.docs, "  "));
                out.push_str(&format!(
                    "  {}: new ({}) => {};\n",
                    class.name,
                    ts_params(&ctor.params),
                    class.name
                ));
            }
            (ctor, _) => {
                out.push_str(&format!("  {}: {{\n", class.name));
                if let Some(ctor) = ctor {
                    out.push_str(&ts_doc(&ctor.docs, "    "));
                    out.push_str(&format!(
                        "    new ({}): {};\n",
                        ts_params(&ctor.params),
                        class.name
                    ));
                }
                for s in &statics {
                    let ret = if s.returns_self {
                        class.name.clone()
                    } else {
                        ts_type(&s.ret)
                    };
                    out.push_str(&ts_doc(&s.docs, "    "));
                    out.push_str(&format!(
                        "    {}({}): {ret};\n",
                        s.name.to_lower_camel_case(),
                        ts_params(&s.params)
                    ));
                }
                out.push_str("  };\n");
            }
        }
    }
    out.push_str("}\n");

    if has_classes {
        out.push('\n');
        out.push_str("// Best-effort backstop: drops handles that were never free()d.\n");
        out.push_str("const finalizer = new FinalizationRegistry<() => void>((drop) => drop());\n");
    }
    if needs_internal {
        out.push('\n');
        out.push_str("// Gates the internal constructor path that wraps a fresh handle.\n");
        out.push_str("const INTERNAL = Symbol(\"rspyts.internal\");\n");
    }

    out.push('\n');
    out.push_str("/** Bind the generated API to an instantiated bridge module. */\n");
    out.push_str(&format!(
        "export function createClient(mod: BridgeModule): {client_name} {{\n"
    ));
    if fns.is_empty() && m.classes.is_empty() {
        out.push_str("  return {};\n}\n");
        return out;
    }
    out.push_str("  return {\n");
    for f in &fns {
        out.push_str(&fn_property(m, f));
    }
    for class in &m.classes {
        out.push_str(&class_property(m, class));
    }
    out.push_str("  };\n}\n");
    out
}

/// Reserved words (including strict-mode ones, which apply everywhere in
/// modules) that cannot be parameter binding names in TypeScript.
const TS_RESERVED: &[&str] = &[
    "arguments",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "eval",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "of",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// Project a Rust parameter name to a stable TypeScript binding. Wire keys
/// remain independent and exact in the request object.
fn ts_binding(rust_name: &str) -> String {
    let source = rust_name.strip_prefix("r#").unwrap_or(rust_name);
    let name = source.to_lower_camel_case();
    if TS_RESERVED.contains(&name.as_str()) {
        format!("{name}_")
    } else {
        name
    }
}

/// `name: Type, …` for a signature, slices typed as typed arrays.
fn ts_params(params: &[ParamDecl]) -> String {
    params
        .iter()
        .map(|p| format!("{}: {}", ts_binding(&p.name), ts_type(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The `callFn(...)` argument list after the symbol: args object, then
/// slices (when present or when a handle must follow), then the handle.
fn call_args(
    m: &Manifest,
    params: &[ParamDecl],
    handle: Option<&str>,
    err: Option<&str>,
) -> String {
    let plain: Vec<String> = params
        .iter()
        .filter(|p| !matches!(p.ty, Ty::Slice { .. }))
        .map(|p| {
            let binding = ts_binding(&p.name);
            let value = ts_wire_expr(&binding, &p.ty, m, 0).unwrap_or_else(|| binding.clone());
            if binding == p.wire_name && value == binding {
                binding
            } else {
                format!("{}: {value}", ts_prop(&p.wire_name))
            }
        })
        .collect();
    let args = if plain.is_empty() {
        "{}".to_string()
    } else {
        format!("{{ {} }}", plain.join(", "))
    };
    let slices: Vec<String> = params
        .iter()
        .filter_map(|p| match p.ty {
            Ty::Slice { dt } => Some(format!(
                "{{ data: {}, dt: \"{}\" }}",
                ts_binding(&p.name),
                dt.wire_name()
            )),
            _ => None,
        })
        .collect();
    let mut out = args;
    if !slices.is_empty() {
        out.push_str(&format!(", [{}]", slices.join(", ")));
    } else if handle.is_some() {
        out.push_str(", []");
    }
    if let Some(h) = handle {
        out.push_str(", ");
        out.push_str(h);
    }
    if let Some(error_name) = err {
        if slices.is_empty() && handle.is_none() {
            out.push_str(", [], undefined");
        } else if !slices.is_empty() && handle.is_none() {
            out.push_str(", undefined");
        }
        out.push_str(&format!(", errors.{}", ts_error_registry_name(error_name)));
    }
    out
}

fn ts_wire_expr(expr: &str, ty: &Ty, m: &Manifest, depth: usize) -> Option<String> {
    match ty {
        Ty::Json => Some(format!("{{ \"__rspyts_json__\": {expr} }}")),
        Ty::I64 => Some(format!("i64ToWire({expr})")),
        Ty::U64 => Some(format!("u64ToWire({expr})")),
        Ty::Buf { dt } => Some(format!("wireBuffer({expr}, \"{}\")", dt.wire_name())),
        Ty::Option { inner } => ts_wire_expr(expr, inner, m, depth)
            .map(|converted| format!("{expr} === null ? null : {converted}")),
        Ty::List { inner } => {
            let item = format!("item{depth}");
            ts_wire_expr(&item, inner, m, depth + 1)
                .map(|converted| format!("{expr}.map(({item}) => {converted})"))
        }
        Ty::Map { value } => {
            let key = format!("key{depth}");
            let item = format!("value{depth}");
            ts_wire_expr(&item, value, m, depth + 1).map(|converted| {
                format!(
                    "Object.fromEntries(Object.entries({expr}).map(([{key}, {item}]) => [{key}, {converted}]))"
                )
            })
        }
        Ty::Tuple { items } => {
            let mut changed = false;
            let converted = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let access = format!("{expr}[{index}]");
                    match ts_wire_expr(&access, item, m, depth + 1) {
                        Some(converted) => {
                            changed = true;
                            converted
                        }
                        None => access,
                    }
                })
                .collect::<Vec<_>>();
            changed.then(|| format!("[{}]", converted.join(", ")))
        }
        Ty::Ref { name } => match super::util::find_type(m, name) {
            Some(TypeDecl::Newtype { inner, .. }) => ts_wire_expr(expr, inner, m, depth),
            Some(TypeDecl::Struct { fields, .. }) => {
                let converted = ts_converted_fields(expr, fields, m, depth);
                (!converted.is_empty())
                    .then(|| format!("{{ ...{expr}, {} }}", converted.join(", ")))
            }
            Some(TypeDecl::Enum { tag, variants, .. }) => {
                let access = ts_access(expr, tag);
                let mut arms = Vec::new();
                for variant in variants {
                    let converted = ts_converted_fields(expr, &variant.fields, m, depth);
                    if !converted.is_empty() {
                        arms.push(format!(
                            "{access} === {} ? {{ ...{expr}, {} }}",
                            ts_string(&variant.wire_name),
                            converted.join(", ")
                        ));
                    }
                }
                (!arms.is_empty()).then(|| format!("{} : {expr}", arms.join(" : ")))
            }
            _ => None,
        },
        _ => None,
    }
}

fn ts_converted_fields(
    expr: &str,
    fields: &[rspyts_core::ir::FieldDecl],
    m: &Manifest,
    depth: usize,
) -> Vec<String> {
    fields
        .iter()
        .filter_map(|field| {
            let access = ts_access(expr, &field.wire_name);
            let converted = ts_wire_expr(&access, &field.ty, m, depth + 1)?;
            let converted = if field.optional {
                format!("{access} === undefined ? undefined : {converted}")
            } else {
                converted
            };
            let key = ts_prop(&field.wire_name);
            Some(format!("{key}: {converted}"))
        })
        .collect()
}

fn ts_access(expr: &str, field: &str) -> String {
    if is_ts_ident(field) {
        format!("{expr}.{field}")
    } else {
        format!(
            "{expr}[{}]",
            serde_json::to_string(field).expect("field serializes")
        )
    }
}

#[derive(Clone, Copy)]
enum ConversionKind {
    Float,
    I64,
    Json,
    U64,
    Buf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HostContext {
    Response,
    Error,
}

fn contains_kind(m: &Manifest, ty: &Ty, kind: ConversionKind) -> bool {
    fn visit(m: &Manifest, ty: &Ty, kind: ConversionKind, seen: &mut BTreeSet<String>) -> bool {
        match ty {
            Ty::F32 | Ty::F64 => matches!(kind, ConversionKind::Float),
            Ty::I64 => matches!(kind, ConversionKind::I64),
            Ty::Json => matches!(kind, ConversionKind::Json),
            Ty::U64 => matches!(kind, ConversionKind::U64),
            Ty::Buf { .. } => matches!(kind, ConversionKind::Buf),
            Ty::Option { inner } | Ty::List { inner } => visit(m, inner, kind, seen),
            Ty::Map { value } => visit(m, value, kind, seen),
            Ty::Tuple { items } => items.iter().any(|item| visit(m, item, kind, seen)),
            Ty::Ref { name } if seen.insert(name.clone()) => {
                let found = match super::util::find_type(m, name) {
                    Some(TypeDecl::Newtype { inner, .. }) => visit(m, inner, kind, seen),
                    Some(TypeDecl::Struct { fields, .. }) => {
                        fields.iter().any(|field| visit(m, &field.ty, kind, seen))
                    }
                    Some(TypeDecl::Enum { variants, .. }) => variants.iter().any(|variant| {
                        variant
                            .fields
                            .iter()
                            .any(|field| visit(m, &field.ty, kind, seen))
                    }),
                    _ => false,
                };
                seen.remove(name);
                found
            }
            _ => false,
        }
    }

    visit(m, ty, kind, &mut BTreeSet::new())
}

fn ts_wire_type(ty: &Ty, m: &Manifest) -> String {
    match ty {
        Ty::I64 | Ty::U64 => "string".to_string(),
        Ty::Option { inner } => format!("{} | null", ts_wire_type(inner, m)),
        Ty::List { inner } => {
            let inner = ts_wire_type(inner, m);
            if inner.contains(' ') {
                format!("({inner})[]")
            } else {
                format!("{inner}[]")
            }
        }
        Ty::Map { value } => format!("Record<string, {}>", ts_wire_type(value, m)),
        Ty::Tuple { items } => format!(
            "[{}]",
            items
                .iter()
                .map(|item| ts_wire_type(item, m))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Ty::Ref { name } => match super::util::find_type(m, name) {
            Some(TypeDecl::Newtype { inner, .. }) => ts_wire_type(inner, m),
            Some(TypeDecl::Struct { fields, .. }) => format!(
                "{{ {} }}",
                fields
                    .iter()
                    .map(|field| ts_wire_field(field, m))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            Some(TypeDecl::Enum { tag, variants, .. }) => variants
                .iter()
                .map(|variant| {
                    let mut fields = vec![format!(
                        "{}: {}",
                        ts_prop(tag),
                        ts_string(&variant.wire_name)
                    )];
                    fields.extend(variant.fields.iter().map(|field| ts_wire_field(field, m)));
                    format!("{{ {} }}", fields.join("; "))
                })
                .collect::<Vec<_>>()
                .join(" | "),
            _ => name.clone(),
        },
        _ => ts_type(ty),
    }
}

fn ts_wire_field(field: &FieldDecl, m: &Manifest) -> String {
    let name = ts_prop(&field.wire_name);
    match &field.ty {
        Ty::Option { inner } => format!("{name}?: {} | null", ts_wire_type(inner, m)),
        _ if field.optional => format!("{name}?: {}", ts_wire_type(&field.ty, m)),
        _ => format!("{name}: {}", ts_wire_type(&field.ty, m)),
    }
}

fn ts_host_expr(
    expr: &str,
    ty: &Ty,
    m: &Manifest,
    depth: usize,
    context: HostContext,
) -> Option<String> {
    match ty {
        Ty::Json if context == HostContext::Error => Some(format!("jsonFromWire({expr})")),
        Ty::F32 | Ty::F64 => Some(format!("floatFromWire({expr})")),
        Ty::I64 => Some(format!("i64FromWire({expr})")),
        Ty::U64 => Some(format!("u64FromWire({expr})")),
        Ty::Option { inner } => ts_host_expr(expr, inner, m, depth, context)
            .map(|converted| format!("{expr} === null ? null : {converted}")),
        Ty::List { inner } => {
            let item = format!("item{depth}");
            ts_host_expr(&item, inner, m, depth + 1, context)
                .map(|converted| format!("{expr}.map(({item}) => {converted})"))
        }
        Ty::Map { value } => {
            let key = format!("key{depth}");
            let item = format!("value{depth}");
            ts_host_expr(&item, value, m, depth + 1, context).map(|converted| {
                format!(
                    "Object.fromEntries(Object.entries({expr}).map(([{key}, {item}]) => [{key}, {converted}]))"
                )
            })
        }
        Ty::Tuple { items } => {
            let mut changed = false;
            let converted = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let access = format!("{expr}[{index}]");
                    match ts_host_expr(&access, item, m, depth + 1, context) {
                        Some(converted) => {
                            changed = true;
                            converted
                        }
                        None => access,
                    }
                })
                .collect::<Vec<_>>();
            changed.then(|| format!("[{}]", converted.join(", ")))
        }
        Ty::Ref { name } => match super::util::find_type(m, name) {
            Some(TypeDecl::Newtype { inner, .. }) => ts_host_expr(expr, inner, m, depth, context),
            Some(TypeDecl::Struct { fields, .. }) => {
                let converted = ts_host_converted_fields(expr, fields, m, depth, context);
                (!converted.is_empty())
                    .then(|| format!("{{ ...{expr}, {} }}", converted.join(", ")))
            }
            Some(TypeDecl::Enum { tag, variants, .. }) => {
                let access = ts_access(expr, tag);
                let mut arms = Vec::new();
                for variant in variants {
                    let converted =
                        ts_host_converted_fields(expr, &variant.fields, m, depth, context);
                    if !converted.is_empty() {
                        arms.push(format!(
                            "{access} === {} ? {{ ...{expr}, {} }}",
                            ts_string(&variant.wire_name),
                            converted.join(", ")
                        ));
                    }
                }
                (!arms.is_empty()).then(|| format!("{} : {expr}", arms.join(" : ")))
            }
            _ => None,
        },
        _ => None,
    }
}

fn ts_host_converted_fields(
    expr: &str,
    fields: &[FieldDecl],
    m: &Manifest,
    depth: usize,
    context: HostContext,
) -> Vec<String> {
    fields
        .iter()
        .filter_map(|field| {
            let access = ts_access(expr, &field.wire_name);
            let converted = ts_host_expr(&access, &field.ty, m, depth + 1, context)?;
            let converted = if field.optional {
                format!("{access} === undefined ? undefined : {converted}")
            } else {
                converted
            };
            let key = ts_prop(&field.wire_name);
            Some(format!("{key}: {converted}"))
        })
        .collect()
}

fn ts_decoded_expr(call: &str, ty: &Ty, m: &Manifest) -> String {
    let host = ts_type(ty);
    match ts_host_expr("value", ty, m, 0, HostContext::Response) {
        Some(converted) => {
            let wire = ts_wire_type(ty, m);
            format!("((value: {wire}): {host} => ({converted}))({call} as {wire})")
        }
        None => format!("{call} as {host}"),
    }
}

/// One `name: (…) => …` property of the returned client object.
fn fn_property(m: &Manifest, f: &FnDecl) -> String {
    let name = f.name.to_lower_camel_case();
    let symbol = format!("rspyts_fn__{}", f.name);
    let params = ts_params(&f.params);
    let call = format!(
        "callFn(mod, \"{symbol}\", {})",
        call_args(m, &f.params, None, f.err.as_deref())
    );
    if matches!(f.ret, Ty::Unit) {
        format!("    {name}: ({params}): void => {{\n      {call};\n    }},\n")
    } else {
        let ret = ts_type(&f.ret);
        let decoded = ts_decoded_expr(&call, &f.ret, m);
        format!("    {name}: ({params}): {ret} =>\n      {decoded},\n")
    }
}

/// One `Name: class { … }` property of the returned client object.
fn class_property(m: &Manifest, class: &ClassDecl) -> String {
    let name = &class.name;
    let drop_symbol = format!("rspyts_cls__{name}__drop");
    let statics: Vec<&StaticDecl> = class.statics.iter().filter(|s| on_ts(&s.targets)).collect();
    let factories: Vec<&&StaticDecl> = statics.iter().filter(|s| s.returns_self).collect();
    // Factories wrap a raw handle, so construction routes through the
    // INTERNAL sentinel; a class with neither factories nor a bridged
    // constructor still takes the guarded path (its ctor only throws).
    let internal_route = class.constructor.is_none() || !factories.is_empty();

    let mut out = format!("    {name}: class {{\n");
    out.push_str("      #handle: bigint;\n\n");

    match (&class.constructor, internal_route) {
        (Some(ctor), false) => {
            out.push_str(&format!(
                "      constructor({}) {{\n",
                ts_params(&ctor.params)
            ));
            out.push_str(&format!(
                "        const raw = callFn(mod, \"rspyts_cls__{name}__new\", {}) as number;\n",
                call_args(m, &ctor.params, None, ctor.err.as_deref())
            ));
            out.push_str("        this.#handle = BigInt(raw);\n");
        }
        (Some(ctor), true) => {
            // Overloads: the public signature, plus the internal one the
            // factories use. The implementation dispatches on INTERNAL.
            out.push_str(&format!(
                "      constructor({});\n",
                ts_params(&ctor.params)
            ));
            out.push_str("      constructor(internal: typeof INTERNAL, raw: number);\n");
            out.push_str("      constructor(...args: unknown[]) {\n");
            out.push_str("        let raw: number;\n");
            out.push_str("        if (args[0] === INTERNAL) {\n");
            out.push_str("          raw = args[1] as number;\n");
            out.push_str("        } else {\n");
            if ctor.params.is_empty() {
                out.push_str(&format!(
                    "          raw = callFn(mod, \"rspyts_cls__{name}__new\", {}) as number;\n",
                    call_args(m, &ctor.params, None, ctor.err.as_deref())
                ));
            } else {
                let bindings: Vec<String> =
                    ctor.params.iter().map(|p| ts_binding(&p.name)).collect();
                let types: Vec<String> = ctor.params.iter().map(|p| ts_type(&p.ty)).collect();
                out.push_str(&format!(
                    "          const [{}] = args as [{}];\n",
                    bindings.join(", "),
                    types.join(", ")
                ));
                out.push_str(&format!(
                    "          raw = callFn(mod, \"rspyts_cls__{name}__new\", {}) as number;\n",
                    call_args(m, &ctor.params, None, ctor.err.as_deref())
                ));
            }
            out.push_str("        }\n");
            out.push_str("        this.#handle = BigInt(raw);\n");
        }
        (None, _) => {
            let hint = match factories.as_slice() {
                [] => String::new(),
                many => format!(
                    "; use {}",
                    many.iter()
                        .map(|s| format!("{name}.{}(...)", s.name.to_lower_camel_case()))
                        .collect::<Vec<_>>()
                        .join(" or ")
                ),
            };
            out.push_str("      constructor(internal: typeof INTERNAL, raw: number);\n");
            out.push_str("      constructor(...args: unknown[]) {\n");
            out.push_str("        if (args[0] !== INTERNAL) {\n");
            out.push_str(&format!(
                "          throw new Error(\"{name} cannot be constructed directly{hint}\");\n"
            ));
            out.push_str("        }\n");
            out.push_str("        this.#handle = BigInt(args[1] as number);\n");
        }
    }
    out.push_str("        const handle = this.#handle;\n");
    out.push_str(&format!(
        "        finalizer.register(this, () => callDrop(mod, \"{drop_symbol}\", handle), this);\n"
    ));
    out.push_str("      }\n");

    for s in &statics {
        let s_name = s.name.to_lower_camel_case();
        let symbol = format!("rspyts_cls__{name}__{}", s.name);
        out.push('\n');
        out.push_str(&ts_doc(&s.docs, "      "));
        if s.returns_self {
            out.push_str(&format!(
                "      static {s_name}({}): {name} {{\n",
                ts_params(&s.params)
            ));
            out.push_str(&format!(
                "        const raw = callFn(mod, \"{symbol}\", {}) as number;\n",
                call_args(m, &s.params, None, s.err.as_deref())
            ));
            out.push_str("        return new this(INTERNAL, raw);\n");
            out.push_str("      }\n");
        } else if matches!(s.ret, Ty::Unit) {
            out.push_str(&format!(
                "      static {s_name}({}): void {{\n        callFn(mod, \"{symbol}\", {});\n      }}\n",
                ts_params(&s.params),
                call_args(m, &s.params, None, s.err.as_deref())
            ));
        } else {
            let ret = ts_type(&s.ret);
            let call = format!(
                "callFn(mod, \"{symbol}\", {})",
                call_args(m, &s.params, None, s.err.as_deref())
            );
            let decoded = ts_decoded_expr(&call, &s.ret, m);
            out.push_str(&format!(
                "      static {s_name}({}): {ret} {{\n        return {decoded};\n      }}\n",
                ts_params(&s.params),
            ));
        }
    }

    for method in class.methods.iter().filter(|method| on_ts(&method.targets)) {
        let m_name = method.name.to_lower_camel_case();
        let symbol = format!("rspyts_cls__{name}__{}", method.name);
        let call = format!(
            "callFn(mod, \"{symbol}\", {})",
            call_args(
                m,
                &method.params,
                Some("this.#handle"),
                method.err.as_deref(),
            )
        );
        out.push('\n');
        out.push_str(&ts_doc(&method.docs, "      "));
        if matches!(method.ret, Ty::Unit) {
            out.push_str(&format!(
                "      {m_name}({}): void {{\n        {call};\n      }}\n",
                ts_params(&method.params)
            ));
        } else {
            let ret = ts_type(&method.ret);
            let decoded = ts_decoded_expr(&call, &method.ret, m);
            out.push_str(&format!(
                "      {m_name}({}): {ret} {{\n        return {decoded};\n      }}\n",
                ts_params(&method.params)
            ));
        }
    }

    out.push_str(&format!(
        "
      /** Release the underlying Rust object. Safe to call more than once. */
      free(): void {{
        finalizer.unregister(this);
        callDrop(mod, \"{drop_symbol}\", this.#handle);
      }}

      [Symbol.dispose](): void {{
        this.free();
      }}
    }},\n"
    ));
    out
}

// ----------------------------------------------------------------- index.ts

fn index_ts(hash: &str) -> String {
    let mut out = ts_header(hash);
    out.push('\n');
    out.push_str("export * from \"./client\";\n");
    out.push_str("export * from \"./constants\";\n");
    out.push_str("export * from \"./errors\";\n");
    out.push_str("export * from \"./types\";\n");
    out
}

#[cfg(test)]
mod tests {
    use super::super::test_manifest::{
        FOREIGN_ORIGIN, binary_manifest, exact_manifest, manifest, manifest_hash,
    };
    use super::super::util::VERSION;
    use super::*;

    fn no_imports() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    fn mapped_imports() -> BTreeMap<String, String> {
        [(
            FOREIGN_ORIGIN.to_string(),
            "shared-types-example".to_string(),
        )]
        .into_iter()
        .collect()
    }

    fn emitted(file: &str) -> String {
        let m = manifest();
        let hash = manifest_hash(&m);
        emit(&m, &hash, &no_imports())
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
    fn types_ts_matches_golden() {
        let expected = golden(
            r#"// Code generated by rspyts v@VERSION@. DO NOT EDIT.
// rspyts:manifest-hash sha256:@HASH@

/** Options controlling value processing. */
export interface QueryOptions {
  /** Minimum value to include. */
  minimumValue: number;
  tolerance?: number | null;
  metadata: unknown;
}

/** Description of an input source. */
export interface SourceInfo {
  name: string;
  fieldCount: number;
}

export type Severity = "low" | "medium" | "high";

/** Value-processing transitions. */
export type ValueEvent =
  | { kind: "accepted"; index: number; value: number }
  | { kind: "rejected"; index: number };
"#,
        );
        assert_text_eq(&emitted("types.ts"), &expected, "types.ts");
    }

    #[test]
    fn constants_ts_matches_golden() {
        let expected = golden(
            r#"// Code generated by rspyts v@VERSION@. DO NOT EDIT.
// rspyts:manifest-hash sha256:@HASH@

/** Baseline processing options. */
export const DEFAULT_OPTIONS = { minimumValue: 0.5, tolerance: null, metadata: { rev: 2 } } as const;

export const DEFAULT_LIMIT = 0.75 as const;

/** Name reported by the value processor. */
export const PROCESSOR_NAME = "vector-processor" as const;

export const SUPPORTED_FORMATS = ["csv", "json"] as const;
"#,
        );
        assert_text_eq(&emitted("constants.ts"), &expected, "constants.ts");
    }

    #[test]
    fn errors_ts_matches_golden() {
        let expected = golden(
            r#"// Code generated by rspyts v@VERSION@. DO NOT EDIT.
// rspyts:manifest-hash sha256:@HASH@

import { type BridgeErrorRegistry, RspytsError } from "rspyts";

export class QueryError extends RspytsError {}

/** The batch size must be positive. */
export class QueryErrorInvalidBatchSize extends QueryError {
  constructor(message: string, data?: unknown) {
    super(message, "invalidBatchSize", data);
  }
}

/** `.data`: { max: number } */
export class QueryErrorBatchTooLarge extends QueryError {
  constructor(message: string, data?: unknown) {
    super(message, "batchTooLarge", data);
  }
}

export const QueryErrorTypes = {
  "invalidBatchSize": QueryErrorInvalidBatchSize,
  "batchTooLarge": QueryErrorBatchTooLarge,
} as const satisfies BridgeErrorRegistry;
"#,
        );
        assert_text_eq(&emitted("errors.ts"), &expected, "errors.ts");
    }

    #[test]
    fn client_ts_matches_golden() {
        // `warm_up` is Python-only and must not appear; `renderSummary`
        // is TypeScript-only and must.
        let expected = golden(
            r#"// Code generated by rspyts v@VERSION@. DO NOT EDIT.
// rspyts:manifest-hash sha256:@HASH@

import { type BridgeModule, callDrop, callFn, floatFromWire } from "rspyts";
import type { QueryOptions, SourceInfo } from "./types";
import * as errors from "./errors";

/** A processing session backed by a native handle. */
export interface Session {
  /** Current completion ratio. */
  progress(): number;
  info(): SourceInfo;
  /** Release the underlying Rust object. Safe to call more than once. */
  free(): void;
  [Symbol.dispose](): void;
}

/** Streaming statistics over a sliding window. */
export interface RunningStats {
  push(chunk: Float64Array): void;
  /** Snapshot current state. */
  snapshot(): QueryOptions;
  /** Release the underlying Rust object. Safe to call more than once. */
  free(): void;
  [Symbol.dispose](): void;
}

/** The typed surface of the `demo-crate` bridge module. */
export interface DemoCrateClient {
  /** Process a buffer of numeric values. */
  processValues(values: Float64Array, batchSize: number, options: QueryOptions): Float64Array;
  /** Render an HTML summary. */
  renderSummary(): string;
  Session: {
    /** Open a processing session from disk. */
    open(path: string): Session;
    defaultExtension(): string;
  };
  RunningStats: {
    new (window: number): RunningStats;
    /** Rebuild from a snapshot. */
    resumed(state: QueryOptions): RunningStats;
  };
}

// Best-effort backstop: drops handles that were never free()d.
const finalizer = new FinalizationRegistry<() => void>((drop) => drop());

// Gates the internal constructor path that wraps a fresh handle.
const INTERNAL = Symbol("rspyts.internal");

/** Bind the generated API to an instantiated bridge module. */
export function createClient(mod: BridgeModule): DemoCrateClient {
  return {
    processValues: (values: Float64Array, batchSize: number, options: QueryOptions): Float64Array =>
      callFn(mod, "rspyts_fn__process_values", { batchSize, options: { ...options, metadata: { "__rspyts_json__": options.metadata } } }, [{ data: values, dt: "f64" }], undefined, errors.QueryErrorTypes) as Float64Array,
    renderSummary: (): string =>
      callFn(mod, "rspyts_fn__render_summary", {}) as string,
    Session: class {
      #handle: bigint;

      constructor(internal: typeof INTERNAL, raw: number);
      constructor(...args: unknown[]) {
        if (args[0] !== INTERNAL) {
          throw new Error("Session cannot be constructed directly; use Session.open(...)");
        }
        this.#handle = BigInt(args[1] as number);
        const handle = this.#handle;
        finalizer.register(this, () => callDrop(mod, "rspyts_cls__Session__drop", handle), this);
      }

      /** Open a processing session from disk. */
      static open(path: string): Session {
        const raw = callFn(mod, "rspyts_cls__Session__open", { path }, [], undefined, errors.QueryErrorTypes) as number;
        return new this(INTERNAL, raw);
      }

      static defaultExtension(): string {
        return callFn(mod, "rspyts_cls__Session__default_extension", {}) as string;
      }

      /** Current completion ratio. */
      progress(): number {
        return ((value: number): number => (floatFromWire(value)))(callFn(mod, "rspyts_cls__Session__progress", {}, [], this.#handle) as number);
      }

      info(): SourceInfo {
        return callFn(mod, "rspyts_cls__Session__info", {}, [], this.#handle) as SourceInfo;
      }

      /** Release the underlying Rust object. Safe to call more than once. */
      free(): void {
        finalizer.unregister(this);
        callDrop(mod, "rspyts_cls__Session__drop", this.#handle);
      }

      [Symbol.dispose](): void {
        this.free();
      }
    },
    RunningStats: class {
      #handle: bigint;

      constructor(window: number);
      constructor(internal: typeof INTERNAL, raw: number);
      constructor(...args: unknown[]) {
        let raw: number;
        if (args[0] === INTERNAL) {
          raw = args[1] as number;
        } else {
          const [window] = args as [number];
          raw = callFn(mod, "rspyts_cls__RunningStats__new", { window }) as number;
        }
        this.#handle = BigInt(raw);
        const handle = this.#handle;
        finalizer.register(this, () => callDrop(mod, "rspyts_cls__RunningStats__drop", handle), this);
      }

      /** Rebuild from a snapshot. */
      static resumed(state: QueryOptions): RunningStats {
        const raw = callFn(mod, "rspyts_cls__RunningStats__resumed", { state: { ...state, metadata: { "__rspyts_json__": state.metadata } } }) as number;
        return new this(INTERNAL, raw);
      }

      push(chunk: Float64Array): void {
        callFn(mod, "rspyts_cls__RunningStats__push", {}, [{ data: chunk, dt: "f64" }], this.#handle);
      }

      /** Snapshot current state. */
      snapshot(): QueryOptions {
        return ((value: { minimumValue: number; tolerance?: number | null; metadata: unknown }): QueryOptions => ({ ...value, minimumValue: floatFromWire(value.minimumValue), tolerance: value.tolerance === undefined ? undefined : value.tolerance === null ? null : floatFromWire(value.tolerance) }))(callFn(mod, "rspyts_cls__RunningStats__snapshot", {}, [], this.#handle) as { minimumValue: number; tolerance?: number | null; metadata: unknown });
      }

      /** Release the underlying Rust object. Safe to call more than once. */
      free(): void {
        finalizer.unregister(this);
        callDrop(mod, "rspyts_cls__RunningStats__drop", this.#handle);
      }

      [Symbol.dispose](): void {
        this.free();
      }
    },
  };
}
"#,
        );
        assert_text_eq(&emitted("client.ts"), &expected, "client.ts");
    }

    #[test]
    fn index_ts_matches_golden() {
        let expected = golden(
            r#"// Code generated by rspyts v@VERSION@. DO NOT EDIT.
// rspyts:manifest-hash sha256:@HASH@

export * from "./client";
export * from "./constants";
export * from "./errors";
export * from "./types";
"#,
        );
        assert_text_eq(&emitted("index.ts"), &expected, "index.ts");
    }

    #[test]
    fn mapped_foreign_types_are_imported_not_emitted() {
        let m = manifest();
        let hash = manifest_hash(&m);
        let files = emit(&m, &hash, &mapped_imports());
        let types = &files.iter().find(|(n, _)| *n == "types.ts").unwrap().1;
        assert!(
            types.contains("import type { SourceInfo } from \"shared-types-example\";\n"),
            "{types}"
        );
        // Re-exported so `export * from "./types"` in index.ts surfaces it.
        assert!(types.contains("export type { SourceInfo };\n"), "{types}");
        assert!(!types.contains("export interface SourceInfo"), "{types}");
        // client.ts keeps importing it from "./types", which re-exports.
        let client = &files.iter().find(|(n, _)| *n == "client.ts").unwrap().1;
        assert!(
            client.contains("import type { QueryOptions, SourceInfo } from \"./types\";"),
            "{client}"
        );
    }

    #[test]
    fn unmapped_foreign_types_are_emitted_locally() {
        let types = emitted("types.ts");
        assert!(types.contains("export interface SourceInfo {"), "{types}");
        assert!(!types.contains("shared-types-example"), "{types}");
    }

    #[test]
    fn binary_newtype_fixture_marks_only_numeric_u8_buffers() {
        let m = binary_manifest();
        let hash = manifest_hash(&m);
        let files = emit(&m, &hash, &no_imports());
        let types = &files.iter().find(|(n, _)| *n == "types.ts").unwrap().1;
        let client = &files.iter().find(|(n, _)| *n == "client.ts").unwrap().1;

        assert!(types.contains("export type PacketId = number;"));
        assert!(types.contains("  payload: Uint8Array;"));
        assert!(types.contains("  samples: Float64Array;"));
        assert!(types.contains("  chunks: Int16Array[];"));
        assert!(types.contains("  channels: Record<string, Uint8Array>;"));
        assert!(client.contains("samples: wireBuffer(value.samples, \"f64\")"));
        assert!(client.contains("wireBuffer(item1, \"i16\")"));
        assert!(client.contains("wireBuffer(value1, \"u8\")"));
        assert!(client.contains("echoBytes: (value: Uint8Array): Uint8Array =>\n      callFn(mod, \"rspyts_fn__echo_bytes\", { value })"));
        assert!(client.contains("echoU8: (value: Uint8Array): Uint8Array =>\n      callFn(mod, \"rspyts_fn__echo_u8\", { value: wireBuffer(value, \"u8\") })"));
    }

    #[test]
    fn exact_tuple_and_mixed_fixture_projects_recursive_conversions() {
        let m = exact_manifest();
        crate::validate::validate(&m).expect("fixture validates");
        let hash = manifest_hash(&m);
        let files = emit(&m, &hash, &no_imports());
        let file = |name| files.iter().find(|(n, _)| *n == name).unwrap().1.as_str();
        let types = file("types.ts");
        let client = file("client.ts");
        let constants = file("constants.ts");
        let errors = file("errors.ts");

        assert!(types.contains("export type SequenceId = bigint;"));
        assert!(types.contains("  pair: [bigint, bigint];"));
        assert!(types.contains("  | { type: \"pending\" }"));
        assert!(types.contains("  | { type: \"ready\"; total: bigint }"));
        assert!(client.contains("i64ToWire(value[0])"));
        assert!(client.contains("u64ToWire(value[1])"));
        assert!(client.contains("i64FromWire(value[0])"));
        assert!(client.contains("u64FromWire(value[1])"));
        assert!(client.contains("value.type === \"ready\""));
        assert!(constants.contains(
            "export const EXACT_PAIR = [-9223372036854775808n, 18446744073709551615n] as const;"
        ));
        assert!(constants.contains("export const MAX_SEQUENCE = 18446744073709551615n as const;"));
        assert!(errors.contains("u64FromWire(value.exactLimit)"));
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
        let files = emit(&m, &hash, &mapped_imports());
        let errors = &files.iter().find(|(n, _)| *n == "errors.ts").unwrap().1;
        // A value import carries the foreign registry into local wrappers.
        assert!(
            errors.contains(
                "import { QueryError, QueryErrorBatchTooLarge, \
                 QueryErrorInvalidBatchSize, QueryErrorTypes } from \"shared-types-example\";\n"
            ),
            "{errors}"
        );
        assert!(
            errors.contains(
                "export { QueryError, QueryErrorBatchTooLarge, \
                 QueryErrorInvalidBatchSize, QueryErrorTypes };\n"
            ),
            "{errors}"
        );
        assert!(!errors.contains("export class QueryError"), "{errors}");
        assert!(!errors.contains("registerError("), "{errors}");
    }

    #[test]
    fn class_without_statics_keeps_the_plain_constructor_path() {
        let mut m = manifest();
        // Drop the factory-only class and RunningStats's factory: no
        // class routes through INTERNAL anymore.
        m.classes.remove(0);
        m.classes[0].statics.clear();
        let hash = manifest_hash(&m);
        let files = emit(&m, &hash, &no_imports());
        let client = &files.iter().find(|(n, _)| *n == "client.ts").unwrap().1;
        assert!(!client.contains("INTERNAL"), "{client}");
        assert!(
            client.contains("  RunningStats: new (window: number) => RunningStats;\n"),
            "{client}"
        );
        assert!(
            client.contains(
                "      constructor(window: number) {\n        \
                 const raw = callFn(mod, \"rspyts_cls__RunningStats__new\", { window }) as number;\n"
            ),
            "{client}"
        );
    }

    #[test]
    fn null_constants_skip_the_const_assertion() {
        use rspyts_core::ir::ConstDecl;
        let c = ConstDecl {
            name: "MAYBE".to_string(),
            docs: String::new(),
            origin: "demo-crate".to_string(),
            ty: Ty::Option {
                inner: Box::new(Ty::F64),
            },
            value: serde_json::Value::Null,
        };
        // `null as const` is a TS1355 error; plain `null` is emitted.
        assert_eq!(const_line(&manifest(), &c), "export const MAYBE = null;\n");
    }

    #[test]
    fn ts_json_renders_literals_and_quotes_non_identifier_keys() {
        use serde_json::json;
        assert_eq!(
            ts_json(&json!({"with-dash": 1, "plain": true})),
            "{ \"with-dash\": 1, plain: true }"
        );
        assert_eq!(ts_json(&json!([1.5, null, "x"])), "[1.5, null, \"x\"]");
        assert_eq!(ts_json(&json!({})), "{}");
    }

    #[test]
    fn reserved_word_param_binds_with_underscore_and_exact_wire_key() {
        let m = manifest();
        let f = FnDecl {
            name: "with_default".to_string(),
            docs: String::new(),
            params: vec![ParamDecl {
                name: "default".to_string(),
                wire_name: "default".to_string(),
                ty: Ty::U32,
            }],
            ret: Ty::U32,
            err: None,
            targets: Target::all(),
        };
        // Signature binds `default_`; the args object keeps the exact
        // wire key with no shorthand.
        assert_eq!(ts_params(&f.params), "default_: number");
        assert_eq!(
            call_args(&m, &f.params, None, None),
            "{ default: default_ }"
        );
        assert_eq!(
            fn_property(&m, &f),
            "    withDefault: (default_: number): number =>\n      \
             callFn(mod, \"rspyts_fn__with_default\", { default: default_ }) as number,\n"
        );
        // Non-reserved names keep the shorthand form.
        let plain = vec![ParamDecl {
            name: "count".to_string(),
            wire_name: "count".to_string(),
            ty: Ty::U32,
        }];
        assert_eq!(call_args(&m, &plain, None, None), "{ count }");
    }

    #[test]
    fn quoted_property_names_when_not_identifiers() {
        assert_eq!(
            ts_field("with-dash", &Ty::Bool, false),
            "\"with-dash\": boolean"
        );
        assert_eq!(ts_field("plain", &Ty::Bool, false), "plain: boolean");
        assert_eq!(
            ts_field("line\n\"quote-dash", &Ty::Bool, false),
            "\"line\\n\\\"quote-dash\": boolean"
        );
    }

    #[test]
    fn json_type_projects_to_unknown() {
        assert_eq!(ts_type(&Ty::Json), "unknown");
        assert_eq!(
            ts_type(&Ty::List {
                inner: Box::new(Ty::Json)
            }),
            "unknown[]"
        );
    }
}
