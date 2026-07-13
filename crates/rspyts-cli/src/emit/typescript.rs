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
    ClassDecl, ConstDecl, FnDecl, Manifest, ParamDecl, StaticDecl, Target, Ty, TypeDecl,
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
                "import type {{ {} }} from \"{module}\";\n",
                names.join(", ")
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
                let literals: Vec<String> = variants
                    .iter()
                    .map(|v| format!("\"{}\"", v.wire_name))
                    .collect();
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
                    let mut members = vec![format!("{tag}: \"{}\"", v.wire_name)];
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
    let name = if is_ts_ident(wire_name) {
        wire_name.to_string()
    } else {
        format!("\"{wire_name}\"")
    };
    match ty {
        Ty::Option { inner } => format!("{name}?: {} | null", ts_type(inner)),
        _ if optional => format!("{name}?: {}", ts_type(ty)),
        _ => format!("{name}: {}", ts_type(ty)),
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
        out.push_str(&const_line(c));
    }
    out
}

/// One `export const NAME = <literal> as const;` line. `as const` is
/// skipped for `null`, which cannot take a const assertion.
fn const_line(c: &ConstDecl) -> String {
    let literal = ts_json(&c.value);
    if c.value.is_null() {
        format!("export const {} = null;\n", c.name)
    } else {
        format!("export const {} = {literal} as const;\n", c.name)
    }
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
        out.push_str("import { RspytsError, registerError } from \"rspyts\";\n");
    }
    // Value imports: instantiating the foreign module also runs its own
    // registerError calls, so imported errors need no local registration.
    if !foreign.is_empty() {
        let mut all_names: Vec<String> = Vec::new();
        for (module, mut names) in foreign {
            names.sort();
            out.push_str(&format!(
                "import {{ {} }} from \"{module}\";\n",
                names.join(", ")
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
                    .map(|f| format!("{}: {}", f.wire_name, ts_type(&f.ty)))
                    .collect();
                lines.push(format!("`.data`: {{ {} }}", entries.join("; ")));
            }
            out.push('\n');
            out.push_str(&ts_doc_from_lines(&lines, ""));
            // The registry contract (BridgeErrorConstructor) bakes the code
            // into the subclass: throw sites pass only (message, data).
            out.push_str(&format!(
                "export class {name}{} extends {name} {{\n  constructor(message: string, data?: unknown) {{\n    super(message, \"{}\", data);\n  }}\n}}\n",
                v.name, v.wire_code
            ));
        }
    }
    if !local.is_empty() {
        out.push('\n');
    }
    for (name, _, variants) in &local {
        for v in *variants {
            out.push_str(&format!(
                "registerError(\"{}\", {name}{});\n",
                v.wire_code, v.name
            ));
        }
    }
    out
}

// ---------------------------------------------------------------- client.ts

fn client_ts(m: &Manifest, hash: &str) -> String {
    let client_name = format!("{}Client", pascal(&m.crate_name));
    let has_errors = m
        .types
        .iter()
        .any(|t| matches!(t, TypeDecl::ErrorEnum { .. }));
    let fns: Vec<&FnDecl> = m.functions.iter().filter(|f| on_ts(&f.targets)).collect();
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
        out.push_str("import \"./errors\";\n");
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
        out.push_str(&fn_property(f));
    }
    for class in &m.classes {
        out.push_str(&class_property(class));
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

/// The binding name for a parameter: its wire name, with a trailing
/// underscore when the wire name is a reserved word. Wire keys in the
/// args object always stay exact — only the binding is escaped.
fn ts_binding(wire_name: &str) -> String {
    if TS_RESERVED.contains(&wire_name) {
        format!("{wire_name}_")
    } else {
        wire_name.to_string()
    }
}

/// `name: Type, …` for a signature, slices typed as typed arrays.
fn ts_params(params: &[ParamDecl]) -> String {
    params
        .iter()
        .map(|p| format!("{}: {}", ts_binding(&p.wire_name), ts_type(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The `callFn(...)` argument list after the symbol: args object, then
/// slices (when present or when a handle must follow), then the handle.
fn call_args(params: &[ParamDecl], handle: Option<&str>) -> String {
    let plain: Vec<String> = params
        .iter()
        .filter(|p| !matches!(p.ty, Ty::Slice { .. }))
        .map(|p| {
            let binding = ts_binding(&p.wire_name);
            if binding == p.wire_name {
                binding
            } else {
                // No shorthand: the wire key must stay exact.
                format!("\"{}\": {binding}", p.wire_name)
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
                ts_binding(&p.wire_name),
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
    out
}

/// One `name: (…) => …` property of the returned client object.
fn fn_property(f: &FnDecl) -> String {
    let name = f.name.to_lower_camel_case();
    let symbol = format!("rspyts_fn__{}", f.name);
    let params = ts_params(&f.params);
    let call = format!("callFn(mod, \"{symbol}\", {})", call_args(&f.params, None));
    if matches!(f.ret, Ty::Unit) {
        format!("    {name}: ({params}): void => {{\n      {call};\n    }},\n")
    } else {
        let ret = ts_type(&f.ret);
        format!("    {name}: ({params}): {ret} =>\n      {call} as {ret},\n")
    }
}

/// One `Name: class { … }` property of the returned client object.
fn class_property(class: &ClassDecl) -> String {
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
                call_args(&ctor.params, None)
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
                    call_args(&ctor.params, None)
                ));
            } else {
                let bindings: Vec<String> = ctor
                    .params
                    .iter()
                    .map(|p| ts_binding(&p.wire_name))
                    .collect();
                let types: Vec<String> = ctor.params.iter().map(|p| ts_type(&p.ty)).collect();
                out.push_str(&format!(
                    "          const [{}] = args as [{}];\n",
                    bindings.join(", "),
                    types.join(", ")
                ));
                out.push_str(&format!(
                    "          raw = callFn(mod, \"rspyts_cls__{name}__new\", {}) as number;\n",
                    call_args(&ctor.params, None)
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
                call_args(&s.params, None)
            ));
            out.push_str("        return new this(INTERNAL, raw);\n");
            out.push_str("      }\n");
        } else if matches!(s.ret, Ty::Unit) {
            out.push_str(&format!(
                "      static {s_name}({}): void {{\n        callFn(mod, \"{symbol}\", {});\n      }}\n",
                ts_params(&s.params),
                call_args(&s.params, None)
            ));
        } else {
            let ret = ts_type(&s.ret);
            out.push_str(&format!(
                "      static {s_name}({}): {ret} {{\n        return callFn(mod, \"{symbol}\", {}) as {ret};\n      }}\n",
                ts_params(&s.params),
                call_args(&s.params, None)
            ));
        }
    }

    for method in class.methods.iter().filter(|method| on_ts(&method.targets)) {
        let m_name = method.name.to_lower_camel_case();
        let symbol = format!("rspyts_cls__{name}__{}", method.name);
        let call = format!(
            "callFn(mod, \"{symbol}\", {})",
            call_args(&method.params, Some("this.#handle"))
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
            out.push_str(&format!(
                "      {m_name}({}): {ret} {{\n        return {call} as {ret};\n      }}\n",
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
    use super::super::test_manifest::{FOREIGN_ORIGIN, manifest, manifest_hash};
    use super::super::util::VERSION;
    use super::*;

    fn no_imports() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    fn mapped_imports() -> BTreeMap<String, String> {
        [(
            FOREIGN_ORIGIN.to_string(),
            "@neurovirtual/hardware/generated".to_string(),
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

/** Parameters controlling the analysis pass. */
export interface AnalysisParams {
  /** Minimum duration, in seconds. */
  minDurationS: number;
  threshold?: number | null;
  metadata: unknown;
}

/** Hardware description reported by the device. */
export interface HardwareInfo {
  vendor: string;
  channelCount: number;
}

export type Severity = "low" | "medium" | "high";

/** Signal threshold transitions. */
export type ThresholdEvent =
  | { kind: "crossed"; atSample: number; value: number }
  | { kind: "cleared"; atSample: number };
"#,
        );
        assert_text_eq(&emitted("types.ts"), &expected, "types.ts");
    }

    #[test]
    fn constants_ts_matches_golden() {
        let expected = golden(
            r#"// Code generated by rspyts v@VERSION@. DO NOT EDIT.
// rspyts:manifest-hash sha256:@HASH@

/** Baseline analysis parameters. */
export const DEFAULT_PARAMS = { minDurationS: 0.5, threshold: null, metadata: { rev: 2 } } as const;

export const DEFAULT_THRESHOLD = 0.75 as const;

/** Name reported by the analysis engine. */
export const ENGINE_NAME = "neuro-engine" as const;

export const SUPPORTED_UNITS = ["uV", "mV"] as const;
"#,
        );
        assert_text_eq(&emitted("constants.ts"), &expected, "constants.ts");
    }

    #[test]
    fn errors_ts_matches_golden() {
        let expected = golden(
            r#"// Code generated by rspyts v@VERSION@. DO NOT EDIT.
// rspyts:manifest-hash sha256:@HASH@

import { RspytsError, registerError } from "rspyts";

export class AnalysisError extends RspytsError {}

/** The sample rate must be positive. */
export class AnalysisErrorInvalidSampleRate extends AnalysisError {
  constructor(message: string, data?: unknown) {
    super(message, "invalidSampleRate", data);
  }
}

/** `.data`: { max: number } */
export class AnalysisErrorWindowTooLarge extends AnalysisError {
  constructor(message: string, data?: unknown) {
    super(message, "windowTooLarge", data);
  }
}

registerError("invalidSampleRate", AnalysisErrorInvalidSampleRate);
registerError("windowTooLarge", AnalysisErrorWindowTooLarge);
"#,
        );
        assert_text_eq(&emitted("errors.ts"), &expected, "errors.ts");
    }

    #[test]
    fn client_ts_matches_golden() {
        // `preload` is Python-only and must not appear; `renderReport`
        // is TypeScript-only and must.
        let expected = golden(
            r#"// Code generated by rspyts v@VERSION@. DO NOT EDIT.
// rspyts:manifest-hash sha256:@HASH@

import { type BridgeModule, callDrop, callFn } from "rspyts";
import type { AnalysisParams, HardwareInfo } from "./types";
import "./errors";

/** A recorded session backed by a native handle. */
export interface Recording {
  /** Total duration in seconds. */
  durationS(): number;
  info(): HardwareInfo;
  /** Release the underlying Rust object. Safe to call more than once. */
  free(): void;
  [Symbol.dispose](): void;
}

/** Streaming statistics over a sliding window. */
export interface RunningStats {
  push(chunk: Float64Array): void;
  /** Snapshot current state. */
  snapshot(): AnalysisParams;
  /** Release the underlying Rust object. Safe to call more than once. */
  free(): void;
  [Symbol.dispose](): void;
}

/** The typed surface of the `demo-crate` bridge module. */
export interface DemoCrateClient {
  /** Analyze a signal buffer. */
  analyzeSignal(samples: Float64Array, sampleRate: number, params: AnalysisParams): Float64Array;
  /** Render an HTML report. */
  renderReport(): string;
  Recording: {
    /** Open a recording from disk. */
    open(path: string): Recording;
    defaultExtension(): string;
  };
  RunningStats: {
    new (window: number): RunningStats;
    /** Rebuild from a snapshot. */
    resumed(state: AnalysisParams): RunningStats;
  };
}

// Best-effort backstop: drops handles that were never free()d.
const finalizer = new FinalizationRegistry<() => void>((drop) => drop());

// Gates the internal constructor path that wraps a fresh handle.
const INTERNAL = Symbol("rspyts.internal");

/** Bind the generated API to an instantiated bridge module. */
export function createClient(mod: BridgeModule): DemoCrateClient {
  return {
    analyzeSignal: (samples: Float64Array, sampleRate: number, params: AnalysisParams): Float64Array =>
      callFn(mod, "rspyts_fn__analyze_signal", { sampleRate, params }, [{ data: samples, dt: "f64" }]) as Float64Array,
    renderReport: (): string =>
      callFn(mod, "rspyts_fn__render_report", {}) as string,
    Recording: class {
      #handle: bigint;

      constructor(internal: typeof INTERNAL, raw: number);
      constructor(...args: unknown[]) {
        if (args[0] !== INTERNAL) {
          throw new Error("Recording cannot be constructed directly; use Recording.open(...)");
        }
        this.#handle = BigInt(args[1] as number);
        const handle = this.#handle;
        finalizer.register(this, () => callDrop(mod, "rspyts_cls__Recording__drop", handle), this);
      }

      /** Open a recording from disk. */
      static open(path: string): Recording {
        const raw = callFn(mod, "rspyts_cls__Recording__open", { path }) as number;
        return new this(INTERNAL, raw);
      }

      static defaultExtension(): string {
        return callFn(mod, "rspyts_cls__Recording__default_extension", {}) as string;
      }

      /** Total duration in seconds. */
      durationS(): number {
        return callFn(mod, "rspyts_cls__Recording__duration_s", {}, [], this.#handle) as number;
      }

      info(): HardwareInfo {
        return callFn(mod, "rspyts_cls__Recording__info", {}, [], this.#handle) as HardwareInfo;
      }

      /** Release the underlying Rust object. Safe to call more than once. */
      free(): void {
        finalizer.unregister(this);
        callDrop(mod, "rspyts_cls__Recording__drop", this.#handle);
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
      static resumed(state: AnalysisParams): RunningStats {
        const raw = callFn(mod, "rspyts_cls__RunningStats__resumed", { state }) as number;
        return new this(INTERNAL, raw);
      }

      push(chunk: Float64Array): void {
        callFn(mod, "rspyts_cls__RunningStats__push", {}, [{ data: chunk, dt: "f64" }], this.#handle);
      }

      /** Snapshot current state. */
      snapshot(): AnalysisParams {
        return callFn(mod, "rspyts_cls__RunningStats__snapshot", {}, [], this.#handle) as AnalysisParams;
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
            types.contains(
                "import type { HardwareInfo } from \"@neurovirtual/hardware/generated\";\n"
            ),
            "{types}"
        );
        // Re-exported so `export * from "./types"` in index.ts surfaces it.
        assert!(types.contains("export type { HardwareInfo };\n"), "{types}");
        assert!(!types.contains("export interface HardwareInfo"), "{types}");
        // client.ts keeps importing it from "./types", which re-exports.
        let client = &files.iter().find(|(n, _)| *n == "client.ts").unwrap().1;
        assert!(
            client.contains("import type { AnalysisParams, HardwareInfo } from \"./types\";"),
            "{client}"
        );
    }

    #[test]
    fn unmapped_foreign_types_are_emitted_locally() {
        let types = emitted("types.ts");
        assert!(types.contains("export interface HardwareInfo {"), "{types}");
        assert!(!types.contains("neurovirtual"), "{types}");
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
        // A value import: loading the foreign module runs its own
        // registerError calls, so no local registration is emitted.
        assert!(
            errors.contains(
                "import { AnalysisError, AnalysisErrorInvalidSampleRate, \
                 AnalysisErrorWindowTooLarge } from \"@neurovirtual/hardware/generated\";\n"
            ),
            "{errors}"
        );
        assert!(
            errors.contains(
                "export { AnalysisError, AnalysisErrorInvalidSampleRate, \
                 AnalysisErrorWindowTooLarge };\n"
            ),
            "{errors}"
        );
        assert!(!errors.contains("export class AnalysisError"), "{errors}");
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
        assert_eq!(const_line(&c), "export const MAYBE = null;\n");
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
        assert_eq!(call_args(&f.params, None), "{ \"default\": default_ }");
        assert_eq!(
            fn_property(&f),
            "    withDefault: (default_: number): number =>\n      \
             callFn(mod, \"rspyts_fn__with_default\", { \"default\": default_ }) as number,\n"
        );
        // Non-reserved names keep the shorthand form.
        let plain = vec![ParamDecl {
            name: "count".to_string(),
            wire_name: "count".to_string(),
            ty: Ty::U32,
        }];
        assert_eq!(call_args(&plain, None), "{ count }");
    }

    #[test]
    fn quoted_property_names_when_not_identifiers() {
        assert_eq!(
            ts_field("with-dash", &Ty::Bool, false),
            "\"with-dash\": boolean"
        );
        assert_eq!(ts_field("plain", &Ty::Bool, false), "plain: boolean");
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
