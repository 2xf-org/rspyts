//! The TypeScript emitter (codegen.md §5, §8).
//!
//! Produces `client.ts`, private `codecs.ts`, `constants.ts`, `errors.ts`,
//! `index.ts`, and `types.ts`. Output targets ES2022, 2-space indent, semicolons,
//! double quotes, `strict` tsc. The surface is camelCase throughout —
//! identical to the wire names, so values need no key mapping at
//! runtime. Types whose origin crate has an entry in
//! `[typescript.imports]` are imported from that module instead of
//! re-emitted (codegen.md §9).

use super::util::{
    collect_refs, int_bounds, pascal, ts_doc, ts_doc_from_lines, ts_error_registry_name, ts_header,
    ts_type, ts_typed_array,
};
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
        ("codecs.ts", codecs_ts(m, hash)),
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
    // `index.ts`'s `export * from "./types.js"` surfaces them.
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
                ts_module_specifier(module)
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
                        ts_field(&f.wire_name, &f.ty, f.required)
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
                            .map(|f| ts_field(&f.wire_name, &f.ty, f.required)),
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
    wrap_ts_source(&out, 120)
}

/// `name: T`, or `name?: Inner | null` for optional fields.
fn ts_field(wire_name: &str, ty: &Ty, required: bool) -> String {
    let name = ts_prop(wire_name);
    if required {
        format!("{name}: {}", ts_type(ty))
    } else {
        format!("{name}?: {}", ts_type(ty))
    }
}

fn ts_string(value: &str) -> String {
    serde_json::to_string(value).expect("serializing a string cannot fail")
}

/// Render an import-map target as an ESM module specifier. Relative targets
/// name generated TypeScript modules, so their runtime-facing specifier must
/// point at the emitted JavaScript file. Bare/package specifiers stay exact.
fn ts_module_specifier(value: &str) -> String {
    let value = if value.starts_with("./") || value.starts_with("../") {
        if let Some(stem) = value.strip_suffix(".ts") {
            format!("{stem}.js")
        } else if value.ends_with(".js")
            || value
                .rsplit('/')
                .next()
                .is_some_and(|part| part.contains('.'))
        {
            value.to_string()
        } else {
            format!("{value}.js")
        }
    } else {
        value.to_string()
    };
    ts_string(&value)
}

fn ts_prop(value: &str) -> String {
    if value == "__proto__" {
        return format!("[{}]", ts_string(value));
    }
    if is_ts_ident(value) {
        value.to_string()
    } else {
        ts_string(value)
    }
}

fn ts_quoted_prop(value: &str) -> String {
    if value == "__proto__" {
        format!("[{}]", ts_string(value))
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
        return wrap_ts_source(&out, 120);
    }
    for c in &m.constants {
        out.push('\n');
        out.push_str(&ts_doc(&c.docs, ""));
        out.push_str(&const_line(m, c));
    }
    wrap_ts_source(&out, 120)
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
        Ty::Json => ts_json(value),
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
                        let key = ts_prop(key);
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
            let key_expr = ts_prop(key);
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
                    let key = ts_prop(k);
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
        return wrap_ts_source(&out, 120);
    }

    out.push('\n');
    if !local.is_empty() {
        out.push_str(
            "import { type BridgeErrorRegistry, RspytsError, wireResponse } from \"rspyts/internal/abi3\";\n",
        );
        if local
            .iter()
            .any(|(_, _, variants)| variants.iter().any(|variant| !variant.fields.is_empty()))
        {
            out.push_str("import * as codecs from \"./codecs.js\";\n");
        }
    }
    // Value imports include the foreign error classes and their scoped map.
    if !foreign.is_empty() {
        let mut all_names: Vec<String> = Vec::new();
        for (module, mut names) in foreign {
            names.sort();
            out.push_str(&format!(
                "import {{ {} }} from {};\n",
                names.join(", "),
                ts_module_specifier(module)
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
            let data = ts_error_data_expr(name, &v.name, &v.fields);
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
                ts_quoted_prop(&v.wire_code),
                v.name
            ));
        }
        out.push_str("} as const satisfies BridgeErrorRegistry;\n");
    }
    wrap_ts_source(&out, 120)
}

fn ts_error_data_expr(error: &str, variant: &str, fields: &[FieldDecl]) -> String {
    if fields.is_empty() {
        return "data".to_string();
    }
    format!(
        "data === undefined ? undefined : codecs.decode{error}{variant}Data(wireResponse(data))"
    )
}

// ---------------------------------------------------------------- codecs.ts

/// Generator/runtime plumbing. This module is imported by `client.ts` and
/// `errors.ts`, but deliberately never re-exported by `index.ts`.
fn codecs_ts(m: &Manifest, hash: &str) -> String {
    let (encode_names, decode_names) = codec_type_requirements(m);
    let mut body = String::new();

    for name in &encode_names {
        let declaration = super::util::find_type(m, name).expect("validated type reference");
        body.push_str(&named_encoder(m, declaration));
    }
    for name in &decode_names {
        let declaration = super::util::find_type(m, name).expect("validated type reference");
        body.push_str(&named_decoder(m, declaration));
    }
    for declaration in &m.types {
        let TypeDecl::ErrorEnum { name, variants, .. } = declaration else {
            continue;
        };
        for variant in variants.iter().filter(|variant| !variant.fields.is_empty()) {
            body.push_str(&error_data_decoder(m, name, &variant.name, &variant.fields));
        }
    }
    for function in m
        .functions
        .iter()
        .filter(|function| on_ts(&function.targets))
    {
        body.push_str(&args_encoder(m, &fn_args_codec(function), &function.params));
        body.push_str(&result_decoder(
            m,
            &fn_result_codec(function),
            &function.ret,
        ));
    }
    for class in &m.classes {
        if let Some(constructor) = &class.constructor {
            body.push_str(&args_encoder(
                m,
                &ctor_args_codec(class),
                &constructor.params,
            ));
        }
        for method in class.methods.iter().filter(|method| on_ts(&method.targets)) {
            body.push_str(&args_encoder(
                m,
                &method_args_codec(class, &method.name),
                &method.params,
            ));
            body.push_str(&result_decoder(
                m,
                &method_result_codec(class, &method.name),
                &method.ret,
            ));
        }
        for method in class.statics.iter().filter(|method| on_ts(&method.targets)) {
            body.push_str(&args_encoder(
                m,
                &static_args_codec(class, &method.name),
                &method.params,
            ));
            if !method.returns_self {
                body.push_str(&result_decoder(
                    m,
                    &static_result_codec(class, &method.name),
                    &method.ret,
                ));
            }
        }
    }

    let mut out = ts_header(hash);
    if body.is_empty() {
        out.push_str("\nexport {};\n");
        return wrap_ts_source(&out, 120);
    }

    let runtime_helpers = [
        "boolFromWire",
        "boundedIntFromWire",
        "bufferFromWire",
        "bytesFromWire",
        "enumFromWire",
        "f32FromWire",
        "floatFromWire",
        "i64FromWire",
        "i64ToWire",
        "jsonFromWire",
        "jsonToWire",
        "listFromWire",
        "mapFromWire",
        "nullFromWire",
        "objectFromWire",
        "stringEnumFromWire",
        "stringFromWire",
        "tupleFromWire",
        "type WireResponse",
        "u64FromWire",
        "u64ToWire",
        "wireBuffer",
    ];
    let used = runtime_helpers
        .into_iter()
        .filter(|helper| body.contains(helper.trim_start_matches("type ")))
        .collect::<Vec<_>>();
    out.push('\n');
    if !used.is_empty() {
        out.push_str(&format!(
            "import {{ {} }} from \"rspyts/internal/abi3\";\n",
            used.join(", ")
        ));
    }
    let mut types = encode_names;
    types.extend(decode_names);
    if !types.is_empty() {
        out.push_str(&format!(
            "import type {{ {} }} from \"./types.js\";\n",
            types.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    out.push_str(&body);
    wrap_ts_source(&out, 120)
}

fn codec_type_requirements(m: &Manifest) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut encode = BTreeSet::new();
    let mut decode = BTreeSet::new();
    for function in m
        .functions
        .iter()
        .filter(|function| on_ts(&function.targets))
    {
        for parameter in &function.params {
            collect_codec_refs(m, &parameter.ty, &mut encode);
        }
        collect_codec_refs(m, &function.ret, &mut decode);
    }
    for class in &m.classes {
        if let Some(constructor) = &class.constructor {
            for parameter in &constructor.params {
                collect_codec_refs(m, &parameter.ty, &mut encode);
            }
        }
        for method in class.methods.iter().filter(|method| on_ts(&method.targets)) {
            for parameter in &method.params {
                collect_codec_refs(m, &parameter.ty, &mut encode);
            }
            collect_codec_refs(m, &method.ret, &mut decode);
        }
        for method in class.statics.iter().filter(|method| on_ts(&method.targets)) {
            for parameter in &method.params {
                collect_codec_refs(m, &parameter.ty, &mut encode);
            }
            if !method.returns_self {
                collect_codec_refs(m, &method.ret, &mut decode);
            }
        }
    }
    for declaration in &m.types {
        if let TypeDecl::ErrorEnum { variants, .. } = declaration {
            for variant in variants {
                for field in &variant.fields {
                    collect_codec_refs(m, &field.ty, &mut decode);
                }
            }
        }
    }
    (encode, decode)
}

fn collect_codec_refs(m: &Manifest, ty: &Ty, names: &mut BTreeSet<String>) {
    match ty {
        Ty::Ref { name } => {
            if !names.insert(name.clone()) {
                return;
            }
            let Some(declaration) = super::util::find_type(m, name) else {
                return;
            };
            match declaration {
                TypeDecl::Newtype { inner, .. } => collect_codec_refs(m, inner, names),
                TypeDecl::Struct { fields, .. } => {
                    for field in fields {
                        collect_codec_refs(m, &field.ty, names);
                    }
                }
                TypeDecl::Enum { variants, .. } => {
                    for variant in variants {
                        for field in &variant.fields {
                            collect_codec_refs(m, &field.ty, names);
                        }
                    }
                }
                TypeDecl::StringEnum { .. } | TypeDecl::ErrorEnum { .. } => {}
            }
        }
        Ty::Option { inner } | Ty::List { inner } => collect_codec_refs(m, inner, names),
        Ty::Map { value } => collect_codec_refs(m, value, names),
        Ty::Tuple { items } => {
            for item in items {
                collect_codec_refs(m, item, names);
            }
        }
        _ => {}
    }
}

fn named_encoder(m: &Manifest, declaration: &TypeDecl) -> String {
    let name = declaration.name();
    let expression = match declaration {
        TypeDecl::Newtype { inner, .. } => {
            ts_wire_expr("value", inner, m, 0).unwrap_or_else(|| "value".to_string())
        }
        TypeDecl::Struct { fields, .. } => {
            let fields = ts_converted_fields("value", fields, m, 0);
            if fields.is_empty() {
                "value".to_string()
            } else {
                format!("{{ ...value, {} }}", fields.join(", "))
            }
        }
        TypeDecl::Enum { tag, variants, .. } => {
            let access = ts_access("value", tag);
            let arms = variants
                .iter()
                .filter_map(|variant| {
                    let fields = ts_converted_fields("value", &variant.fields, m, 0);
                    (!fields.is_empty()).then(|| {
                        format!(
                            "{access} === {} ? {{ ...value, {} }}",
                            ts_string(&variant.wire_name),
                            fields.join(", ")
                        )
                    })
                })
                .collect::<Vec<_>>();
            if arms.is_empty() {
                "value".to_string()
            } else {
                format!("{} : value", arms.join(" : "))
            }
        }
        TypeDecl::StringEnum { .. } => "value".to_string(),
        TypeDecl::ErrorEnum { .. } => return String::new(),
    }
    .replace("codecs.", "");
    format!(
        "\nexport function encode{name}(value: {name}): unknown {{\n  return {expression};\n}}\n"
    )
}

fn named_decoder(m: &Manifest, declaration: &TypeDecl) -> String {
    let name = declaration.name();
    let ty = Ty::Ref {
        name: name.to_string(),
    };
    let expression = match declaration {
        TypeDecl::Newtype { inner, .. } => ts_response_expr("response", inner, m, 0),
        TypeDecl::Struct { fields, .. } => ts_response_object("response", &ty, fields, m, 0),
        TypeDecl::StringEnum { variants, .. } => {
            let variants = variants
                .iter()
                .map(|variant| ts_string(&variant.wire_name))
                .collect::<Vec<_>>()
                .join(", ");
            format!("stringEnumFromWire(response, [{variants}])")
        }
        TypeDecl::Enum { tag, variants, .. } => {
            ts_response_enum("response", &ty, tag, variants, m, 0)
        }
        TypeDecl::ErrorEnum { .. } => return String::new(),
    }
    .replace("codecs.", "");
    format!(
        "\nexport function decode{name}(response: WireResponse): {name} {{\n  return {expression};\n}}\n"
    )
}

fn error_data_decoder(m: &Manifest, error: &str, variant: &str, fields: &[FieldDecl]) -> String {
    let required = fields
        .iter()
        .filter(|field| field.required)
        .map(|field| ts_string(&field.wire_name))
        .collect::<Vec<_>>()
        .join(", ");
    let optional = fields
        .iter()
        .filter(|field| !field.required)
        .map(|field| ts_string(&field.wire_name))
        .collect::<Vec<_>>()
        .join(", ");
    let converted = ts_response_object_fields("value", fields, m, 1).replace("codecs.", "");
    let host = fields
        .iter()
        .map(|field| ts_field(&field.wire_name, &field.ty, field.required))
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "\nexport function decode{error}{variant}Data(response: WireResponse): {{ {host} }} {{\n  const value = objectFromWire(response, [{required}], [{optional}]);\n  return {{ {converted} }};\n}}\n"
    )
}

fn args_encoder(m: &Manifest, name: &str, params: &[ParamDecl]) -> String {
    let plain = params
        .iter()
        .filter(|parameter| !matches!(parameter.ty, Ty::Slice { .. }))
        .collect::<Vec<_>>();
    let signature = plain
        .iter()
        .map(|parameter| {
            format!(
                "{}: {}",
                ts_binding(&parameter.name),
                ts_type(&parameter.ty)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let fields = plain
        .iter()
        .map(|parameter| {
            let binding = ts_binding(&parameter.name);
            let value = ts_wire_expr(&binding, &parameter.ty, m, 0)
                .unwrap_or_else(|| binding.clone())
                .replace("codecs.", "");
            format!("{}: {value}", ts_prop(&parameter.wire_name))
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "\nexport function {name}({signature}): Record<string, unknown> {{\n  return {{ {fields} }};\n}}\n"
    )
}

fn result_decoder(m: &Manifest, name: &str, ty: &Ty) -> String {
    let converted = ts_response_expr("response", ty, m, 0).replace("codecs.", "");
    if matches!(ty, Ty::Unit) {
        format!("\nexport function {name}(response: WireResponse): void {{\n  {converted};\n}}\n")
    } else {
        format!(
            "\nexport function {name}(response: WireResponse): {} {{\n  return {converted};\n}}\n",
            ts_type(ty)
        )
    }
}

fn fn_args_codec(function: &FnDecl) -> String {
    format!("encodeFn{}Args", pascal(&function.name))
}

fn fn_result_codec(function: &FnDecl) -> String {
    format!("decodeFn{}Result", pascal(&function.name))
}

fn ctor_args_codec(class: &ClassDecl) -> String {
    format!("encodeClass{}ConstructorArgs", class.name)
}

fn method_args_codec(class: &ClassDecl, method: &str) -> String {
    format!("encodeClass{}Method{}Args", class.name, pascal(method))
}

fn method_result_codec(class: &ClassDecl, method: &str) -> String {
    format!("decodeClass{}Method{}Result", class.name, pascal(method))
}

fn static_args_codec(class: &ClassDecl, method: &str) -> String {
    format!("encodeClass{}Static{}Args", class.name, pascal(method))
}

fn static_result_codec(class: &ClassDecl, method: &str) -> String {
    format!("decodeClass{}Static{}Result", class.name, pascal(method))
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
    let mut rspyts_names = vec![
        "type BridgeModule".to_string(),
        "verifyModuleContract".to_string(),
    ];
    if has_classes {
        rspyts_names.push("callDrop".to_string());
        rspyts_names.push("boundedIntFromWire".to_string());
    }
    if has_calls {
        rspyts_names.push("callFn".to_string());
    }
    out.push_str(&format!(
        "import {{ {} }} from \"rspyts/internal/abi3\";\n",
        rspyts_names.join(", ")
    ));
    out.push_str("import * as codecs from \"./codecs.js\";\n");
    if !type_imports.is_empty() {
        let names: Vec<String> = type_imports.into_iter().collect();
        out.push_str(&format!(
            "import type {{ {} }} from \"./types.js\";\n",
            names.join(", ")
        ));
    }
    if has_errors {
        out.push_str("import * as errors from \"./errors.js\";\n");
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
        out.push_str(
            "const rspytsFinalizationRegistryConstructor = (\n  globalThis as unknown as {\n    FinalizationRegistry?: new (callback: (drop: () => void) => void) => {\n      register(target: object, drop: () => void, token?: object): void;\n      unregister(token: object): boolean;\n    };\n  }\n).FinalizationRegistry;\nconst finalizer =\n  rspytsFinalizationRegistryConstructor === undefined\n    ? {\n        register(_target: object, _drop: () => void, _token?: object): void {},\n        unregister(_token: object): boolean {\n          return false;\n        },\n      }\n    : new rspytsFinalizationRegistryConstructor((drop) => drop());\n",
        );
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
    out.push_str(&format!(
        "  verifyModuleContract(mod, {});\n",
        ts_string(hash)
    ));
    if fns.is_empty() && m.classes.is_empty() {
        out.push_str("  return {};\n}\n");
        return wrap_ts_source(&out, 120);
    }
    out.push_str("  return {\n");
    for f in &fns {
        out.push_str(&fn_property(m, f));
    }
    for class in &m.classes {
        out.push_str(&class_property(m, class));
    }
    out.push_str("  };\n}\n");
    wrap_ts_source(&out, 120)
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

fn call_args_codec(
    params: &[ParamDecl],
    codec: &str,
    handle: Option<&str>,
    err: Option<&str>,
) -> String {
    let bindings = params
        .iter()
        .filter(|parameter| !matches!(parameter.ty, Ty::Slice { .. }))
        .map(|parameter| ts_binding(&parameter.name))
        .collect::<Vec<_>>();
    let mut out = format!("codecs.{codec}({})", bindings.join(", "));
    let slices = params
        .iter()
        .filter_map(|parameter| match parameter.ty {
            Ty::Slice { dt } => Some(format!(
                "{{ data: {}, dt: \"{}\" }}",
                ts_binding(&parameter.name),
                dt.wire_name()
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !slices.is_empty() {
        out.push_str(&format!(", [{}]", slices.join(", ")));
    } else if handle.is_some() || err.is_some() {
        out.push_str(", []");
    }
    if let Some(handle) = handle {
        out.push_str(&format!(", {handle}"));
    } else if err.is_some() {
        out.push_str(", undefined");
    }
    if let Some(error) = err {
        out.push_str(&format!(", errors.{}", ts_error_registry_name(error)));
    }
    out
}

fn ts_wire_expr(expr: &str, ty: &Ty, _m: &Manifest, depth: usize) -> Option<String> {
    match ty {
        Ty::Json => Some(format!("jsonToWire({expr})")),
        Ty::I64 => Some(format!("i64ToWire({expr})")),
        Ty::U64 => Some(format!("u64ToWire({expr})")),
        Ty::Buf { dt } => Some(format!("wireBuffer({expr}, \"{}\")", dt.wire_name())),
        Ty::Option { inner } => ts_wire_expr(expr, inner, _m, depth)
            .map(|converted| format!("{expr} === null ? null : {converted}")),
        Ty::List { inner } => {
            let item = format!("item{depth}");
            ts_wire_expr(&item, inner, _m, depth + 1)
                .map(|converted| format!("{expr}.map(({item}) => ({converted}))"))
        }
        Ty::Map { value } => {
            let key = format!("key{depth}");
            let item = format!("value{depth}");
            ts_wire_expr(&item, value, _m, depth + 1).map(|converted| {
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
                    match ts_wire_expr(&access, item, _m, depth + 1) {
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
        Ty::Ref { name } => Some(format!("codecs.encode{name}({expr})")),
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
            let converted = if !field.required {
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

/// Recursively validate one successful wire value before projecting it to
/// its public host type. Every cast emitted around an inline object wire type
/// is downstream of the corresponding exact shape/tag check.
fn ts_response_expr(expr: &str, ty: &Ty, _m: &Manifest, depth: usize) -> String {
    match ty {
        Ty::Null | Ty::Unit => format!("nullFromWire({expr})"),
        Ty::Bool => format!("boolFromWire({expr})"),
        Ty::U8 | Ty::U16 | Ty::U32 | Ty::I8 | Ty::I16 | Ty::I32 => {
            let (minimum, maximum) = int_bounds(ty).expect("guarded above");
            format!("boundedIntFromWire({expr}, {minimum}, {maximum})")
        }
        Ty::F32 => format!("f32FromWire({expr})"),
        Ty::F64 => format!("floatFromWire({expr})"),
        Ty::String => format!("stringFromWire({expr})"),
        Ty::Bytes => format!("bytesFromWire({expr})"),
        Ty::Buf { dt } => format!(
            "(bufferFromWire({expr}, {}) as {})",
            ts_string(dt.wire_name()),
            ts_typed_array(*dt)
        ),
        // Json stays wrapped through generic envelope decoding; schema-aware
        // conversion is what distinguishes it from an ordinary string map.
        Ty::Json => format!("jsonFromWire({expr})"),
        Ty::I64 => format!("i64FromWire({expr})"),
        Ty::U64 => format!("u64FromWire({expr})"),
        Ty::Option { inner } => format!(
            "{expr}.value === null ? null : {}",
            ts_response_expr(expr, inner, _m, depth)
        ),
        Ty::List { inner } => {
            let item = format!("item{depth}");
            let converted = ts_response_expr(&item, inner, _m, depth + 1);
            format!("listFromWire({expr}).map(({item}) => ({converted}))")
        }
        Ty::Map { value } => {
            let key = format!("key{depth}");
            let item = format!("value{depth}");
            let converted = ts_response_expr(&item, value, _m, depth + 1);
            format!(
                "Object.fromEntries(Object.entries(mapFromWire({expr})).map(([{key}, {item}]) => [{key}, {converted}]))"
            )
        }
        Ty::Tuple { items } => {
            let value = format!("value{depth}");
            let converted = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    ts_response_expr(&format!("{value}[{index}]"), item, _m, depth + 1)
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "(({value}: WireResponse[]): {} => [{converted}])(tupleFromWire({expr}, {}))",
                ts_type(ty),
                items.len()
            )
        }
        Ty::Ref { name } => format!("codecs.decode{name}({expr})"),
        // Slices are parameters only and validation rejects them in returns.
        Ty::Slice { .. } => expr.to_string(),
    }
}

fn ts_response_object(
    expr: &str,
    ty: &Ty,
    fields: &[FieldDecl],
    m: &Manifest,
    depth: usize,
) -> String {
    let host = ts_type(ty);
    let value = format!("value{depth}");
    let required = fields
        .iter()
        .filter(|field| field.required)
        .map(|field| ts_string(&field.wire_name))
        .collect::<Vec<_>>()
        .join(", ");
    let optional = fields
        .iter()
        .filter(|field| !field.required)
        .map(|field| ts_string(&field.wire_name))
        .collect::<Vec<_>>()
        .join(", ");
    let converted = ts_response_object_fields(&value, fields, m, depth + 1);
    format!(
        "(({value}: Record<string, WireResponse>): {host} => ({{ {converted} }}))(objectFromWire({expr}, [{required}], [{optional}]))"
    )
}

fn ts_response_object_fields(
    expr: &str,
    fields: &[FieldDecl],
    m: &Manifest,
    depth: usize,
) -> String {
    fields
        .iter()
        .map(|field| {
            let access = ts_access(expr, &field.wire_name);
            let key = ts_prop(&field.wire_name);
            let converted = ts_response_expr(&access, &field.ty, m, depth);
            if !field.required {
                format!(
                    "...(Object.prototype.hasOwnProperty.call({expr}, {}) ? {{ {key}: {converted} }} : {{}})",
                    ts_string(&field.wire_name)
                )
            } else {
                format!("{key}: {converted}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn ts_response_enum(
    expr: &str,
    ty: &Ty,
    tag: &str,
    variants: &[rspyts_core::ir::VariantDecl],
    m: &Manifest,
    depth: usize,
) -> String {
    let host = ts_type(ty);
    let value = format!("value{depth}");
    let shapes = variants
        .iter()
        .map(|variant| {
            let required = variant
                .fields
                .iter()
                .filter(|field| field.required)
                .map(|field| ts_string(&field.wire_name))
                .collect::<Vec<_>>()
                .join(", ");
            let optional = variant
                .fields
                .iter()
                .filter(|field| !field.required)
                .map(|field| ts_string(&field.wire_name))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{}: {{ required: [{required}], optional: [{optional}] }}",
                ts_quoted_prop(&variant.wire_name)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let arms = variants
        .iter()
        .enumerate()
        .map(|(index, variant)| {
            let converted = ts_response_object_fields(&value, &variant.fields, m, depth + 1);
            let tag_field = format!("{}: {}", ts_prop(tag), ts_string(&variant.wire_name));
            let fields = if converted.is_empty() {
                tag_field
            } else {
                format!("{tag_field}, {converted}")
            };
            let object = format!("{{ {fields} }}");
            if index + 1 == variants.len() {
                object
            } else {
                format!(
                    "stringFromWire({}) === {} ? {object} : ",
                    ts_access(&value, tag),
                    ts_string(&variant.wire_name)
                )
            }
        })
        .collect::<Vec<_>>()
        .join("");
    format!(
        "(({value}: Record<string, WireResponse>): {host} => ({arms}))(enumFromWire({expr}, {}, {{ {shapes} }}))",
        ts_string(tag)
    )
}

/// One `name: (…) => …` property of the returned client object.
fn fn_property(_m: &Manifest, f: &FnDecl) -> String {
    let name = f.name.to_lower_camel_case();
    let symbol = format!("rspyts_fn__{}", f.name);
    let params = ts_params(&f.params);
    let call = format!(
        "callFn(mod, \"{symbol}\", {})",
        call_args_codec(&f.params, &fn_args_codec(f), None, f.err.as_deref())
    );
    if matches!(f.ret, Ty::Unit) {
        format!(
            "    {name}: ({params}): void => {{\n      codecs.{}({call});\n    }},\n",
            fn_result_codec(f)
        )
    } else {
        let ret = ts_type(&f.ret);
        format!(
            "    {name}: ({params}): {ret} =>\n      codecs.{}({call}),\n",
            fn_result_codec(f)
        )
    }
}

/// One `Name: class { … }` property of the returned client object.
fn class_property(_m: &Manifest, class: &ClassDecl) -> String {
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
                "        const raw = boundedIntFromWire(callFn(mod, \"rspyts_cls__{name}__new\", {}), 1, Number.MAX_SAFE_INTEGER);\n",
                call_args_codec(&ctor.params, &ctor_args_codec(class), None, ctor.err.as_deref())
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
                    "          raw = boundedIntFromWire(callFn(mod, \"rspyts_cls__{name}__new\", {}), 1, Number.MAX_SAFE_INTEGER);\n",
                    call_args_codec(&ctor.params, &ctor_args_codec(class), None, ctor.err.as_deref())
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
                    "          raw = boundedIntFromWire(callFn(mod, \"rspyts_cls__{name}__new\", {}), 1, Number.MAX_SAFE_INTEGER);\n",
                    call_args_codec(&ctor.params, &ctor_args_codec(class), None, ctor.err.as_deref())
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
                "        const raw = boundedIntFromWire(callFn(mod, \"{symbol}\", {}), 1, Number.MAX_SAFE_INTEGER);\n",
                call_args_codec(&s.params, &static_args_codec(class, &s.name), None, s.err.as_deref())
            ));
            out.push_str("        return new this(INTERNAL, raw);\n");
            out.push_str("      }\n");
        } else if matches!(s.ret, Ty::Unit) {
            out.push_str(&format!(
                "      static {s_name}({}): void {{\n        codecs.{}(callFn(mod, \"{symbol}\", {}));\n      }}\n",
                ts_params(&s.params),
                static_result_codec(class, &s.name),
                call_args_codec(&s.params, &static_args_codec(class, &s.name), None, s.err.as_deref())
            ));
        } else {
            let ret = ts_type(&s.ret);
            let call = format!(
                "callFn(mod, \"{symbol}\", {})",
                call_args_codec(
                    &s.params,
                    &static_args_codec(class, &s.name),
                    None,
                    s.err.as_deref()
                )
            );
            out.push_str(&format!(
                "      static {s_name}({}): {ret} {{\n        return codecs.{}({call});\n      }}\n",
                ts_params(&s.params),
                static_result_codec(class, &s.name),
            ));
        }
    }

    for method in class.methods.iter().filter(|method| on_ts(&method.targets)) {
        let m_name = method.name.to_lower_camel_case();
        let symbol = format!("rspyts_cls__{name}__{}", method.name);
        let call = format!(
            "callFn(mod, \"{symbol}\", {})",
            call_args_codec(
                &method.params,
                &method_args_codec(class, &method.name),
                Some("this.#handle"),
                method.err.as_deref(),
            )
        );
        out.push('\n');
        out.push_str(&ts_doc(&method.docs, "      "));
        if matches!(method.ret, Ty::Unit) {
            out.push_str(&format!(
                "      {m_name}({}): void {{\n        codecs.{}({call});\n      }}\n",
                ts_params(&method.params),
                method_result_codec(class, &method.name),
            ));
        } else {
            let ret = ts_type(&method.ret);
            out.push_str(&format!(
                "      {m_name}({}): {ret} {{\n        return codecs.{}({call});\n      }}\n",
                ts_params(&method.params),
                method_result_codec(class, &method.name),
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

/// Deterministically wrap generated implementation lines. The emitter owns
/// formatting so checked-in output stays readable without a downstream
/// formatter. Breaks occur only at TypeScript whitespace-safe delimiters.
fn wrap_ts_source(source: &str, width: usize) -> String {
    let mut out = String::with_capacity(source.len() + source.len() / 20);
    for line in source.lines() {
        wrap_ts_line(&mut out, line, width);
    }
    out
}

fn wrap_ts_line(out: &mut String, line: &str, width: usize) {
    if line.len() <= width || line.trim_start().starts_with("//") {
        out.push_str(line);
        out.push('\n');
        return;
    }

    let leading = &line[..line.len() - line.trim_start().len()];
    let continuation = format!("{leading}    ");
    let mut remaining = line.trim_start();
    let mut prefix = leading;
    while prefix.len() + remaining.len() > width {
        let available = width.saturating_sub(prefix.len());
        let points = ts_wrap_points(remaining);
        let split = points
            .iter()
            .copied()
            .rfind(|point| *point <= available && *point >= 16)
            .or_else(|| points.iter().copied().find(|point| *point > available));
        let Some(split) = split else {
            break;
        };
        out.push_str(prefix);
        out.push_str(remaining[..split].trim_end());
        out.push('\n');
        remaining = remaining[split..].trim_start();
        prefix = &continuation;
    }
    out.push_str(prefix);
    out.push_str(remaining);
    out.push('\n');
}

fn ts_wrap_points(line: &str) -> Vec<usize> {
    let bytes = line.as_bytes();
    let mut points = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active {
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(byte, b'\"' | b'\'' | b'`') {
            quote = Some(byte);
            index += 1;
            continue;
        }
        match byte {
            b',' | b'(' | b'[' | b'{' => points.push(index + 1),
            b'?' | b':' if surrounded_by_spaces(bytes, index) => points.push(index),
            b'=' if bytes.get(index + 1) == Some(&b'>') => points.push(index + 2),
            b'&' if bytes.get(index + 1) == Some(&b'&') => points.push(index),
            b'&' if surrounded_by_spaces(bytes, index) => points.push(index),
            b'|' if bytes.get(index + 1) == Some(&b'|') => points.push(index),
            b'|' if surrounded_by_spaces(bytes, index) => points.push(index),
            _ => {}
        }
        index += 1;
    }
    points.sort_unstable();
    points.dedup();
    points
}

fn surrounded_by_spaces(bytes: &[u8], index: usize) -> bool {
    index > 0
        && bytes[index - 1].is_ascii_whitespace()
        && bytes.get(index + 1).is_some_and(u8::is_ascii_whitespace)
}

// ----------------------------------------------------------------- index.ts

fn index_ts(hash: &str) -> String {
    let mut out = ts_header(hash);
    out.push('\n');
    out.push_str("export * from \"./client.js\";\n");
    out.push_str("export * from \"./constants.js\";\n");
    out.push_str("export * from \"./errors.js\";\n");
    out.push_str("export * from \"./types.js\";\n");
    out
}

#[cfg(test)]
mod tests {
    use super::super::test_manifest::{
        FOREIGN_ORIGIN, binary_manifest, exact_manifest, manifest, manifest_hash,
    };
    use super::*;

    fn rendered(manifest: &Manifest) -> Vec<(&'static str, String)> {
        emit(manifest, &manifest_hash(manifest), &BTreeMap::new())
    }

    fn file<'a>(files: &'a [(&str, String)], name: &str) -> &'a str {
        files
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .expect("generated file exists")
            .1
            .as_str()
    }

    #[test]
    fn emits_private_codecs_and_only_the_abi3_runtime_path() {
        let files = rendered(&manifest());
        assert_eq!(
            files.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            [
                "client.ts",
                "codecs.ts",
                "constants.ts",
                "errors.ts",
                "index.ts",
                "types.ts",
            ]
        );
        let client = file(&files, "client.ts");
        assert!(client.contains("from \"rspyts/internal/abi3\""), "{client}");
        assert!(client.contains("verifyModuleContract"), "{client}");
        assert!(client.contains("callFn"), "{client}");
        assert!(!client.contains("callFnRaw"), "{client}");
        assert!(!client.contains("from \"rspyts\""), "{client}");
        assert!(!file(&files, "index.ts").contains("codecs"));
    }

    #[test]
    fn named_references_use_one_named_codec_call() {
        let files = rendered(&exact_manifest());
        let client = file(&files, "client.ts");
        let codecs = file(&files, "codecs.ts");
        assert!(
            client.contains("codecs.encodeFnRoundTripExactArgs("),
            "{client}"
        );
        assert!(
            client.contains("codecs.decodeFnRoundTripExactResult"),
            "{client}"
        );
        assert!(
            codecs.contains("export function encodeExactRecord"),
            "{codecs}"
        );
        assert!(
            codecs.contains("sequence: encodeSequenceId(value.sequence)"),
            "{codecs}"
        );
        assert!(
            codecs.contains("return { value: encodeExactRecord(value) }"),
            "{codecs}"
        );
        assert!(!client.contains("objectFromWire"), "{client}");
        assert!(!client.contains("u64FromWire"), "{client}");
    }

    #[test]
    fn response_codecs_keep_explicit_tail_context_for_nested_buffers() {
        let files = rendered(&binary_manifest());
        let codecs = file(&files, "codecs.ts");
        assert!(codecs.contains("response: WireResponse"), "{codecs}");
        assert!(
            codecs.contains("bufferFromWire(value0.samples, \"f64\")")
                || codecs.contains("bufferFromWire(value0[\"samples\"], \"f64\")"),
            "{codecs}"
        );
        assert!(codecs.contains("decodeBinaryPacket(response)"), "{codecs}");
    }

    #[test]
    fn json_is_transparent_and_exact_integers_are_bigints() {
        let mut manifest = exact_manifest();
        manifest.constants.push(ConstDecl {
            name: "MARKER_SHAPED_JSON".to_string(),
            docs: String::new(),
            origin: manifest.crate_name.clone(),
            ty: Ty::Json,
            value: serde_json::json!({"__rspyts_json__": {"kept": true}}),
        });
        let files = rendered(&manifest);
        let types = file(&files, "types.ts");
        let constants = file(&files, "constants.ts");
        let codecs = file(&files, "codecs.ts");
        assert!(
            types.contains("export type SequenceId = bigint;"),
            "{types}"
        );
        assert!(constants.contains("18446744073709551615n"), "{constants}");
        assert!(
            constants.contains("__rspyts_json__") && constants.contains("kept: true"),
            "{constants}"
        );
        assert!(codecs.contains("jsonToWire"), "{codecs}");
        assert!(codecs.contains("jsonFromWire"), "{codecs}");
        assert!(!codecs.contains("__rspyts_json__"), "{codecs}");
    }

    #[test]
    fn required_controls_presence_and_option_controls_nullability() {
        let files = rendered(&manifest());
        let types = file(&files, "types.ts");
        assert!(types.contains("minimumValue: number;"), "{types}");
        assert!(types.contains("tolerance?: number | null;"), "{types}");
    }

    #[test]
    fn error_data_uses_an_empty_tail_response_codec() {
        let files = rendered(&manifest());
        let errors = file(&files, "errors.ts");
        let codecs = file(&files, "codecs.ts");
        assert!(errors.contains("wireResponse(data)"), "{errors}");
        assert!(errors.contains("rspyts/internal/abi3"), "{errors}");
        assert!(
            codecs.contains("decodeQueryErrorBatchTooLargeData"),
            "{codecs}"
        );
    }

    #[test]
    fn retained_imported_types_still_have_local_wire_codecs() {
        let manifest = manifest();
        let imports = BTreeMap::from([(
            FOREIGN_ORIGIN.to_string(),
            "shared-types-example".to_string(),
        )]);
        let files = emit(&manifest, &manifest_hash(&manifest), &imports);
        assert!(
            file(&files, "types.ts").contains("from \"shared-types-example\""),
            "{}",
            file(&files, "types.ts")
        );
        assert!(
            file(&files, "codecs.ts").contains("decodeSourceInfo"),
            "{}",
            file(&files, "codecs.ts")
        );
    }

    #[test]
    fn every_generated_typescript_file_has_bounded_lines() {
        let mut manifest = exact_manifest();
        manifest.constants.push(ConstDecl {
            name: "LARGE_CATALOG".to_string(),
            docs: String::new(),
            origin: manifest.crate_name.clone(),
            ty: Ty::List {
                inner: Box::new(Ty::String),
            },
            value: serde_json::Value::Array(
                (0..40)
                    .map(|index| serde_json::Value::String(format!("catalog-entry-{index}")))
                    .collect(),
            ),
        });
        manifest.types.push(TypeDecl::StringEnum {
            name: "LargeCatalogKind".to_string(),
            docs: String::new(),
            origin: manifest.crate_name.clone(),
            variants: (0..40)
                .map(|index| rspyts_core::ir::StringVariantDecl {
                    name: format!("CatalogEntry{index}"),
                    wire_name: format!("catalog-entry-{index}"),
                    docs: String::new(),
                })
                .collect(),
        });
        let files = rendered(&manifest);
        for name in [
            "client.ts",
            "codecs.ts",
            "constants.ts",
            "errors.ts",
            "index.ts",
            "types.ts",
        ] {
            let longest = file(&files, name)
                .lines()
                .map(str::len)
                .max()
                .unwrap_or_default();
            assert!(longest <= 160, "{name} has a {longest}-character line");
        }
    }

    #[test]
    fn module_specifiers_and_prototype_keys_are_safe() {
        assert_eq!(
            ts_module_specifier("../shared/generated"),
            "\"../shared/generated.js\""
        );
        assert_eq!(ts_prop("__proto__"), "[\"__proto__\"]");
    }
}
