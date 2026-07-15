//! Compact Python source generation for the ABI 3 runtime.
//!
//! Public modules contain only host declarations and typed wrappers. All
//! schema-directed wire work lives in the private `_codecs.py` module. Named
//! references always cross one named codec boundary, which keeps wrapper size
//! proportional to the number of functions instead of the depth of their
//! models.

use super::util::{
    collect_refs, doc_lines, find_type, int_bounds, py_alias_roundtrips, py_docstring,
    py_error_registry_name, py_header, py_name, py_type,
};
use rspyts_core::ir::{
    ClassDecl, Dtype, FieldDecl, FnDecl, Manifest, ParamDecl, StaticDecl, Target, Ty, TypeDecl,
    VariantDecl,
};
use std::collections::{BTreeMap, BTreeSet};

const LINE_LIMIT: usize = 100;
#[cfg(test)]
const MAX_LINE_LENGTH: usize = 120;

/// Emit one wholly-owned generated Python package.
pub fn emit(
    manifest: &Manifest,
    fingerprint: &str,
    library_search: &[String],
    imports: &BTreeMap<String, String>,
) -> Vec<(&'static str, String)> {
    let codecs = CodecPlan::new(manifest);
    vec![
        ("__init__.py", init_py(manifest, fingerprint)),
        ("_codecs.py", codecs_py(manifest, fingerprint, &codecs)),
        ("classes.py", classes_py(manifest, fingerprint, &codecs)),
        ("constants.py", constants_py(manifest, fingerprint)),
        ("errors.py", errors_py(manifest, fingerprint, imports)),
        ("functions.py", functions_py(manifest, fingerprint, &codecs)),
        (
            "library.py",
            library_py(manifest, fingerprint, library_search),
        ),
        ("models.py", models_py(manifest, fingerprint, imports)),
    ]
}

fn is_imported(manifest: &Manifest, decl: &TypeDecl, imports: &BTreeMap<String, String>) -> bool {
    decl.origin() != manifest.crate_name && imports.contains_key(decl.origin())
}

fn projected_names(decl: &TypeDecl) -> Vec<String> {
    match decl {
        TypeDecl::Newtype { name, .. }
        | TypeDecl::Struct { name, .. }
        | TypeDecl::StringEnum { name, .. } => vec![name.clone()],
        TypeDecl::Enum { name, variants, .. } => std::iter::once(name.clone())
            .chain(
                variants
                    .iter()
                    .map(|variant| format!("{name}{}", variant.name)),
            )
            .collect(),
        TypeDecl::ErrorEnum { name, variants, .. } => std::iter::once(name.clone())
            .chain(
                variants
                    .iter()
                    .map(|variant| format!("{name}{}", variant.name)),
            )
            .collect(),
    }
}

// ---------------------------------------------------------------- models.py

fn models_py(manifest: &Manifest, fingerprint: &str, imports: &BTreeMap<String, String>) -> String {
    let mut body = String::new();
    let mut defined = BTreeSet::new();
    for decl in &manifest.types {
        if is_imported(manifest, decl, imports) {
            defined.insert(decl.name().to_string());
        }
    }

    for decl in data_types_in_dependency_order(manifest, imports) {
        let quote = |name: &str| !defined.contains(name) && find_type(manifest, name).is_some();
        body.push_str("\n\n");
        match decl {
            TypeDecl::Newtype {
                name, docs, inner, ..
            } => {
                for line in doc_lines(docs) {
                    if line.trim().is_empty() {
                        body.push_str("#\n");
                    } else {
                        body.push_str(&format!("# {line}\n"));
                    }
                }
                body.push_str(&format!(
                    "{name}: typing.TypeAlias = {}\n",
                    py_type(inner, true, &quote)
                ));
            }
            TypeDecl::Struct {
                name, docs, fields, ..
            } => {
                body.push_str(&format!("class {name}(rspyts.Contract):\n"));
                body.push_str(&py_docstring(docs, "    "));
                if !doc_lines(docs).is_empty() && !fields.is_empty() {
                    body.push('\n');
                }
                for field in fields {
                    body.push_str(&field_line(field, &quote));
                }
                if fields.is_empty() {
                    body.push_str("    pass\n");
                }
            }
            TypeDecl::StringEnum {
                name,
                docs,
                variants,
                ..
            } => {
                body.push_str(&format!("class {name}(enum.StrEnum):\n"));
                body.push_str(&py_docstring(docs, "    "));
                if !doc_lines(docs).is_empty() && !variants.is_empty() {
                    body.push('\n');
                }
                for variant in variants {
                    body.push_str(&format!(
                        "    {} = {}\n",
                        heck::ToShoutySnakeCase::to_shouty_snake_case(variant.name.as_str()),
                        py_string(&variant.wire_name)
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
                for variant in variants {
                    body.push_str(&variant_class(name, tag, variant, &quote));
                    body.push_str("\n\n");
                }
                body.push_str(&union_alias(name, tag, variants));
                if !doc_lines(docs).is_empty() {
                    body.push_str(&py_docstring(docs, ""));
                }
            }
            TypeDecl::ErrorEnum { .. } => unreachable!("errors are emitted in errors.py"),
        }
        defined.insert(decl.name().to_string());
    }

    let mut out = py_header(fingerprint);
    out.push_str(&format!(
        "\"\"\"\nHost data models bridged from `{}`.\n\"\"\"\n",
        manifest.crate_name
    ));
    out.push_str(&models_imports(
        &body,
        &foreign_import_lines(manifest, imports),
    ));
    if !body.is_empty() {
        let body = body.trim_start_matches('\n');
        out.push('\n');
        if !body.starts_with('#') {
            out.push('\n');
        }
        out.push_str(body);
    }
    out
}

fn data_types_in_dependency_order<'a>(
    manifest: &'a Manifest,
    imports: &BTreeMap<String, String>,
) -> Vec<&'a TypeDecl> {
    let mut pending: BTreeMap<&str, &TypeDecl> = manifest
        .types
        .iter()
        .filter(|decl| !matches!(decl, TypeDecl::ErrorEnum { .. }))
        .filter(|decl| !is_imported(manifest, decl, imports))
        .map(|decl| (decl.name(), decl))
        .collect();
    let imported: BTreeSet<&str> = manifest
        .types
        .iter()
        .filter(|decl| is_imported(manifest, decl, imports))
        .map(TypeDecl::name)
        .collect();
    let mut complete: BTreeSet<&str> = imported;
    let mut result = Vec::new();
    while !pending.is_empty() {
        let next = pending.iter().find_map(|(name, decl)| {
            let mut refs = BTreeSet::new();
            collect_decl_refs(decl, &mut refs);
            refs.iter()
                .all(|reference| complete.contains(reference.as_str()) || reference == name)
                .then_some(*name)
        });
        let name = next.unwrap_or_else(|| *pending.keys().next().expect("pending is non-empty"));
        let decl = pending.remove(name).expect("selected declaration exists");
        complete.insert(decl.name());
        result.push(decl);
    }
    result
}

fn collect_decl_refs(decl: &TypeDecl, refs: &mut BTreeSet<String>) {
    match decl {
        TypeDecl::Newtype { inner, .. } => collect_refs(inner, refs),
        TypeDecl::Struct { fields, .. } => {
            for field in fields {
                collect_refs(&field.ty, refs);
            }
        }
        TypeDecl::Enum { variants, .. } => {
            for field in variants.iter().flat_map(|variant| &variant.fields) {
                collect_refs(&field.ty, refs);
            }
        }
        TypeDecl::StringEnum { .. } | TypeDecl::ErrorEnum { .. } => {}
    }
}

fn field_line(field: &FieldDecl, quote: &dyn Fn(&str) -> bool) -> String {
    let name = py_name(&field.name);
    let inner_bounds = match &field.ty {
        Ty::Option { inner } => scalar_bounds(inner),
        other => scalar_bounds(other),
    };
    let annotation = match (&field.ty, &inner_bounds) {
        (Ty::Option { .. }, Some(_)) => "int | None".to_string(),
        (_, Some(_)) => "int".to_string(),
        (ty, None) => py_type(ty, true, quote),
    };
    let mut args = Vec::new();
    if !field.required {
        args.push("default=None".to_string());
    }
    if !py_alias_roundtrips(&name, &field.wire_name) {
        args.push(format!("alias={}", py_string(&field.wire_name)));
    }
    if let Some((minimum, maximum)) = inner_bounds {
        args.push("strict=True".to_string());
        args.push(format!("ge={minimum}"));
        args.push(format!("le={maximum}"));
    }
    match args.as_slice() {
        [] => format!("    {name}: {annotation}\n"),
        [only] if only == "default=None" => format!("    {name}: {annotation} = None\n"),
        _ => {
            let one = format!(
                "    {name}: {annotation} = pydantic.Field({})",
                args.join(", ")
            );
            if one.len() <= LINE_LIMIT {
                return one + "\n";
            }
            let mut out = format!("    {name}: {annotation} = pydantic.Field(\n");
            for arg in args {
                out.push_str(&format!("        {arg},\n"));
            }
            out.push_str("    )\n");
            out
        }
    }
}

fn scalar_bounds(ty: &Ty) -> Option<(String, String)> {
    if let Some((minimum, maximum)) = int_bounds(ty) {
        return Some((minimum.to_string(), maximum.to_string()));
    }
    match ty {
        Ty::I64 => Some((i64::MIN.to_string(), i64::MAX.to_string())),
        Ty::U64 => Some(("0".to_string(), u64::MAX.to_string())),
        _ => None,
    }
}

fn variant_class(
    enum_name: &str,
    tag: &str,
    variant: &VariantDecl,
    quote: &dyn Fn(&str) -> bool,
) -> String {
    let mut out = format!("class {enum_name}{}(rspyts.Contract):\n", variant.name);
    out.push_str(&py_docstring(&variant.docs, "    "));
    if !doc_lines(&variant.docs).is_empty() {
        out.push('\n');
    }
    let tag_attr = py_name(tag);
    let wire = py_string(&variant.wire_name);
    if py_alias_roundtrips(&tag_attr, tag) {
        out.push_str(&format!(
            "    {tag_attr}: typing.Literal[{wire}] = {wire}\n"
        ));
    } else {
        out.push_str(&format!(
            "    {tag_attr}: typing.Literal[{wire}] = pydantic.Field(default={wire}, alias={})\n",
            py_string(tag)
        ));
    }
    for field in &variant.fields {
        out.push_str(&field_line(field, quote));
    }
    out
}

fn union_alias(name: &str, tag: &str, variants: &[VariantDecl]) -> String {
    let members: Vec<String> = variants
        .iter()
        .map(|variant| format!("{name}{}", variant.name))
        .collect();
    if members.len() == 1 {
        return format!("{name} = {}\n", members[0]);
    }
    let mut out = String::new();
    out.push_str(&format!("{name} = typing.Annotated[\n"));
    let union = members.join(" | ");
    if union.len() + 5 <= LINE_LIMIT {
        out.push_str(&format!("    {union},\n"));
    } else {
        out.push_str("    (\n");
        for (index, member) in members.iter().enumerate() {
            let operator = if index == 0 { "" } else { "| " };
            out.push_str(&format!("        {operator}{member}\n"));
        }
        out.push_str("    ),\n");
    }
    out.push_str(&format!(
        "    pydantic.Field(discriminator={}),\n]\n",
        py_string(&py_name(tag))
    ));
    out
}

fn foreign_import_lines(
    manifest: &Manifest,
    imports: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let mut by_module: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for decl in &manifest.types {
        if matches!(decl, TypeDecl::ErrorEnum { .. }) || !is_imported(manifest, decl, imports) {
            continue;
        }
        by_module
            .entry(imports[decl.origin()].as_str())
            .or_default()
            .extend(projected_names(decl));
    }
    by_module
        .into_iter()
        .map(|(module, mut names)| {
            names.sort();
            names.dedup();
            (module.to_string(), wrap_reexport_import(module, &names))
        })
        .collect()
}

fn models_imports(body: &str, foreign: &[(String, String)]) -> String {
    let mut standard = Vec::new();
    if body.contains("enum.StrEnum") {
        standard.push("import enum");
    }
    if body.contains("typing.") {
        standard.push("import typing");
    }
    let mut third: Vec<String> = Vec::new();
    if body.contains("np.ndarray") {
        third.push("import numpy as np".to_string());
    }
    if body.contains("pydantic.Field(") {
        third.push("import pydantic".to_string());
    }
    if body.contains("rspyts.Contract") {
        third.push("import rspyts".to_string());
    }
    third.extend(foreign.iter().map(|(_, line)| line.clone()));
    let mut out = String::new();
    if !standard.is_empty() {
        out.push('\n');
        out.push_str(&standard.join("\n"));
        out.push('\n');
    }
    if !third.is_empty() {
        out.push('\n');
        out.push_str(&third.join("\n"));
        out.push('\n');
    }
    out
}

// -------------------------------------------------------------- codec plan

#[derive(Clone, Copy)]
enum Direction {
    Encode,
    Decode,
}

struct CodecPlan {
    encode_refs: BTreeSet<String>,
    decode_refs: BTreeSet<String>,
    encode_boundaries: BTreeMap<String, (String, Ty)>,
    decode_boundaries: BTreeMap<String, (String, Ty)>,
}

impl CodecPlan {
    fn new(manifest: &Manifest) -> Self {
        let mut plan = Self {
            encode_refs: BTreeSet::new(),
            decode_refs: BTreeSet::new(),
            encode_boundaries: BTreeMap::new(),
            decode_boundaries: BTreeMap::new(),
        };
        for function in manifest
            .functions
            .iter()
            .filter(|function| function.targets.contains(&Target::Python))
        {
            for param in &function.params {
                if !matches!(param.ty, Ty::Slice { .. }) {
                    plan.add_root(manifest, &param.ty, Direction::Encode);
                }
            }
            plan.add_root(manifest, &function.ret, Direction::Decode);
        }
        for class in &manifest.classes {
            if let Some(constructor) = &class.constructor {
                for param in &constructor.params {
                    if !matches!(param.ty, Ty::Slice { .. }) {
                        plan.add_root(manifest, &param.ty, Direction::Encode);
                    }
                }
            }
            for static_decl in class
                .statics
                .iter()
                .filter(|item| item.targets.contains(&Target::Python))
            {
                for param in &static_decl.params {
                    if !matches!(param.ty, Ty::Slice { .. }) {
                        plan.add_root(manifest, &param.ty, Direction::Encode);
                    }
                }
                if !static_decl.returns_self {
                    plan.add_root(manifest, &static_decl.ret, Direction::Decode);
                }
            }
            for method in class
                .methods
                .iter()
                .filter(|item| item.targets.contains(&Target::Python))
            {
                for param in &method.params {
                    if !matches!(param.ty, Ty::Slice { .. }) {
                        plan.add_root(manifest, &param.ty, Direction::Encode);
                    }
                }
                plan.add_root(manifest, &method.ret, Direction::Decode);
            }
        }
        for decl in &manifest.types {
            if let TypeDecl::ErrorEnum { variants, .. } = decl {
                for field in variants.iter().flat_map(|variant| &variant.fields) {
                    plan.add_type(manifest, &field.ty, Direction::Decode);
                }
            }
        }
        plan
    }

    fn add_root(&mut self, manifest: &Manifest, ty: &Ty, direction: Direction) {
        self.add_type(manifest, ty, direction);
    }

    fn add_type(&mut self, manifest: &Manifest, ty: &Ty, direction: Direction) {
        if let Ty::Ref { name } = ty {
            self.mark_ref(manifest, name, direction);
            return;
        }
        if matches!(ty, Ty::Slice { .. }) {
            return;
        }
        let key = ty_key(ty);
        let target = match direction {
            Direction::Encode => &mut self.encode_boundaries,
            Direction::Decode => &mut self.decode_boundaries,
        };
        target.entry(key).or_insert_with(|| {
            let prefix = match direction {
                Direction::Encode => "_encode_",
                Direction::Decode => "_decode_",
            };
            (format!("{prefix}{}", ty_slug(ty)), ty.clone())
        });
        match ty {
            Ty::Option { inner } | Ty::List { inner } => {
                self.add_type(manifest, inner, direction);
            }
            Ty::Map { value } => self.add_type(manifest, value, direction),
            Ty::Tuple { items } => {
                for item in items {
                    self.add_type(manifest, item, direction);
                }
            }
            _ => {}
        }
    }

    fn mark_ref(&mut self, manifest: &Manifest, name: &str, direction: Direction) {
        let inserted = match direction {
            Direction::Encode => self.encode_refs.insert(name.to_string()),
            Direction::Decode => self.decode_refs.insert(name.to_string()),
        };
        if !inserted {
            return;
        }
        let Some(decl) = find_type(manifest, name) else {
            return;
        };
        match decl {
            TypeDecl::Newtype { inner, .. } => self.add_type(manifest, inner, direction),
            TypeDecl::Struct { fields, .. } => {
                for field in fields {
                    self.add_type(manifest, &field.ty, direction);
                }
            }
            TypeDecl::Enum { variants, .. } => {
                for field in variants.iter().flat_map(|variant| &variant.fields) {
                    self.add_type(manifest, &field.ty, direction);
                }
            }
            TypeDecl::StringEnum { .. } | TypeDecl::ErrorEnum { .. } => {}
        }
    }

    fn encoder(&self, ty: &Ty) -> String {
        match ty {
            Ty::Ref { name } => type_codec_name(Direction::Encode, name),
            _ => self.encode_boundaries[&ty_key(ty)].0.clone(),
        }
    }

    fn decoder(&self, ty: &Ty) -> String {
        match ty {
            Ty::Ref { name } => type_codec_name(Direction::Decode, name),
            _ => self.decode_boundaries[&ty_key(ty)].0.clone(),
        }
    }
}

fn ty_key(ty: &Ty) -> String {
    serde_json::to_string(ty).expect("Ty is serializable")
}

fn ty_slug(ty: &Ty) -> String {
    let raw = match ty {
        Ty::Bool => "bool".to_string(),
        Ty::U8 => "u8".to_string(),
        Ty::U16 => "u16".to_string(),
        Ty::U32 => "u32".to_string(),
        Ty::I8 => "i8".to_string(),
        Ty::I16 => "i16".to_string(),
        Ty::I32 => "i32".to_string(),
        Ty::I64 => "i64".to_string(),
        Ty::U64 => "u64".to_string(),
        Ty::F32 => "f32".to_string(),
        Ty::F64 => "f64".to_string(),
        Ty::String => "string".to_string(),
        Ty::Bytes => "bytes".to_string(),
        Ty::Unit => "unit".to_string(),
        Ty::Null => "null".to_string(),
        Ty::Json => "json".to_string(),
        Ty::Option { inner } => format!("optional_{}", ty_slug(inner)),
        Ty::List { inner } => format!("list_{}", ty_slug(inner)),
        Ty::Map { value } => format!("map_{}", ty_slug(value)),
        Ty::Tuple { items } => format!(
            "tuple_{}",
            items.iter().map(ty_slug).collect::<Vec<_>>().join("_")
        ),
        Ty::Ref { name } => format!("type_{}", py_name(name)),
        Ty::Buf { dt } => format!("buffer_{}", dt.wire_name()),
        Ty::Slice { dt } => format!("slice_{}", dt.wire_name()),
    };
    if raw.len() <= 72 {
        raw
    } else {
        format!("boundary_{}", stable_name_hash(&ty_key(ty)))
    }
}

fn stable_name_hash(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn type_codec_name(direction: Direction, name: &str) -> String {
    let action = match direction {
        Direction::Encode => "encode",
        Direction::Decode => "decode",
    };
    format!("_{action}_type_{}", py_name(name))
}

// --------------------------------------------------------------- _codecs.py

fn codec_def_line(name: &str, param: &str, ret: &str) -> String {
    let one = format!("def {name}({param}) -> {ret}:");
    if one.len() <= LINE_LIMIT {
        return one + "\n";
    }
    format!("def {name}(\n    {param},\n) -> {ret}:\n")
}

fn assignment_line(indent: &str, target: &str, value: &str) -> String {
    let one = format!("{indent}{target} = {value}");
    if one.len() <= LINE_LIMIT {
        return one + "\n";
    }
    format!("{indent}{target} = (\n{indent}    {value}\n{indent})\n")
}

fn return_line(indent: &str, value: &str) -> String {
    let one = format!("{indent}return {value}");
    if one.len() <= LINE_LIMIT {
        return one + "\n";
    }
    format!("{indent}return (\n{indent}    {value}\n{indent})\n")
}

fn mapping_entry_line(indent: &str, key: &str, value: &str) -> String {
    let one = format!("{indent}{key}: {value},");
    if one.len() <= LINE_LIMIT {
        return one + "\n";
    }
    format!("{indent}{key}: (\n{indent}    {value}\n{indent}),\n")
}

fn if_line(indent: &str, condition: &str) -> String {
    let one = format!("{indent}if {condition}:");
    if one.len() <= LINE_LIMIT {
        return one + "\n";
    }
    format!("{indent}if (\n{indent}    {condition}\n{indent}):\n")
}

fn set_operation_line(
    indent: &str,
    target: &str,
    before: &str,
    values: &[String],
    after: &str,
) -> String {
    let literal = set_literal(values);
    let one = format!("{indent}{target} = {before}{literal}{after}");
    if one.len() <= LINE_LIMIT || values.len() <= 1 {
        return one + "\n";
    }
    let mut out = format!("{indent}{target} = {before}{{\n");
    for value in values {
        out.push_str(&format!("{indent}    {value},\n"));
    }
    out.push_str(&format!("{indent}}}{after}\n"));
    out
}

fn raise_line(indent: &str, error: &str, argument: &str) -> String {
    let one = format!("{indent}raise {error}({argument})");
    if one.len() <= LINE_LIMIT {
        return one + "\n";
    }
    format!("{indent}raise {error}(\n{indent}    {argument}\n{indent})\n")
}

fn codecs_py(manifest: &Manifest, fingerprint: &str, plan: &CodecPlan) -> String {
    let mut body = String::new();
    for (name, ty) in plan.encode_boundaries.values() {
        body.push_str("\n\n");
        body.push_str(&codec_function(name, ty, Direction::Encode));
    }
    for (name, ty) in plan.decode_boundaries.values() {
        body.push_str("\n\n");
        body.push_str(&codec_function(name, ty, Direction::Decode));
    }
    for name in &plan.encode_refs {
        if let Some(decl) = find_type(manifest, name) {
            body.push_str("\n\n");
            body.push_str(&named_codec(decl, Direction::Encode));
        }
    }
    for name in &plan.decode_refs {
        if let Some(decl) = find_type(manifest, name) {
            body.push_str("\n\n");
            body.push_str(&named_codec(decl, Direction::Decode));
        }
    }
    for decl in &manifest.types {
        if let TypeDecl::ErrorEnum { name, variants, .. } = decl {
            for variant in variants.iter().filter(|variant| !variant.fields.is_empty()) {
                body.push_str("\n\n");
                body.push_str(&error_data_codec(name, variant));
            }
        }
    }

    let mut out = py_header(fingerprint);
    out.push_str("\"\"\"Private schema-directed ABI 3 codecs.\"\"\"\n");
    out.push_str("\nimport typing\n\nimport rspyts._internal as _rspyts\n");
    if body.contains("models.") {
        out.push_str("\nfrom . import models\n");
    }
    out.push_str("\n_rspyts.require_emitter_api(3)\n");
    out.push_str(
        "\n\ndef _host_list(value: object) -> list[typing.Any]:\n    if type(value) is not list:\n        raise TypeError(f\"rspyts: expected a list, got {type(value).__name__}\")\n    return value\n\n\ndef _host_map(value: object) -> dict[str, typing.Any]:\n    if type(value) is not dict or any(type(key) is not str for key in value):\n        raise TypeError(\"rspyts: expected a string-keyed dict\")\n    return typing.cast(dict[str, typing.Any], value)\n\n\ndef _host_tuple(value: object, length: int) -> tuple[typing.Any, ...]:\n    if type(value) is not tuple or len(value) != length:\n        raise TypeError(f\"rspyts: expected a tuple of length {length}\")\n    return value\n",
    );
    out.push_str(&body);
    out
}

fn codec_function(name: &str, ty: &Ty, direction: Direction) -> String {
    let mut out = match direction {
        Direction::Encode => codec_def_line(name, "value: object", "object"),
        Direction::Decode => codec_def_line(name, "response: _rspyts.Response", "typing.Any"),
    };
    match (direction, ty) {
        (Direction::Encode, Ty::Option { inner }) => {
            out.push_str("    if value is None:\n        return None\n");
            out.push_str(&return_line("    ", &encode_expr("value", inner)));
        }
        (Direction::Decode, Ty::Option { inner }) => {
            out.push_str("    if response.value is None:\n        return None\n");
            out.push_str(&return_line("    ", &decode_expr("response", inner)));
        }
        (Direction::Encode, Ty::List { inner }) => {
            out.push_str(&format!(
                "    return [\n        {}\n        for item in _host_list(value)\n    ]\n",
                encode_expr("item", inner)
            ));
        }
        (Direction::Decode, Ty::List { inner }) => {
            out.push_str(&format!(
                "    return [\n        {}\n        for item in _rspyts.list_from_wire(response)\n    ]\n",
                decode_expr("item", inner)
            ));
        }
        (Direction::Encode, Ty::Map { value }) => {
            out.push_str(&format!(
                "    return {{\n        key: {}\n        for key, item in _host_map(value).items()\n    }}\n",
                encode_expr("item", value)
            ));
        }
        (Direction::Decode, Ty::Map { value }) => {
            out.push_str(&format!(
                "    return {{\n        key: {}\n        for key, item in _rspyts.map_from_wire(response).items()\n    }}\n",
                decode_expr("item", value)
            ));
        }
        (Direction::Encode, Ty::Tuple { items }) => {
            out.push_str(&format!(
                "    items = _host_tuple(value, {})\n",
                items.len()
            ));
            out.push_str("    return (\n");
            for (index, item) in items.iter().enumerate() {
                out.push_str(&format!(
                    "        {},\n",
                    encode_expr(&format!("items[{index}]"), item)
                ));
            }
            out.push_str("    )\n");
        }
        (Direction::Decode, Ty::Tuple { items }) => {
            out.push_str(&format!(
                "    items = _rspyts.tuple_from_wire(response, length={})\n",
                items.len()
            ));
            out.push_str("    return (\n");
            for (index, item) in items.iter().enumerate() {
                out.push_str(&format!(
                    "        {},\n",
                    decode_expr(&format!("items[{index}]"), item)
                ));
            }
            out.push_str("    )\n");
        }
        (Direction::Encode, _) => {
            out.push_str(&return_line("    ", &encode_leaf_expr("value", ty)));
        }
        (Direction::Decode, _) => {
            out.push_str(&return_line("    ", &decode_leaf_expr("response", ty)));
        }
    }
    out
}

fn named_codec(decl: &TypeDecl, direction: Direction) -> String {
    let function = type_codec_name(direction, decl.name());
    match (direction, decl) {
        (Direction::Encode, TypeDecl::Newtype { inner, .. }) => {
            let mut out = codec_def_line(&function, "value: object", "object");
            out.push_str(&return_line("    ", &encode_expr("value", inner)));
            out
        }
        (Direction::Decode, TypeDecl::Newtype { inner, .. }) => {
            let mut out = codec_def_line(&function, "response: _rspyts.Response", "typing.Any");
            out.push_str(&return_line("    ", &decode_expr("response", inner)));
            out
        }
        (Direction::Encode, TypeDecl::Struct { name, fields, .. }) => {
            encode_model(function, name, fields, None)
        }
        (Direction::Decode, TypeDecl::Struct { name, fields, .. }) => {
            decode_model(function, name, fields, None)
        }
        (Direction::Encode, TypeDecl::StringEnum { name, .. }) => {
            let mut out = codec_def_line(&function, "value: object", "str");
            out.push_str(&format!("    if type(value) is not models.{name}:\n"));
            out.push_str(&raise_line(
                "        ",
                "TypeError",
                &format!("f\"rspyts: expected {name}, got {{type(value).__name__}}\""),
            ));
            out.push_str("    return value.value\n");
            out
        }
        (Direction::Decode, TypeDecl::StringEnum { name, .. }) => {
            let mut out = codec_def_line(
                &function,
                "response: _rspyts.Response",
                &format!("models.{name}"),
            );
            out.push_str(&return_line(
                "    ",
                &format!("models.{name}(_rspyts.string_from_wire(response))"),
            ));
            out
        }
        (
            Direction::Encode,
            TypeDecl::Enum {
                name,
                tag,
                variants,
                ..
            },
        ) => {
            let mut out = codec_def_line(&function, "value: object", "object");
            for variant in variants {
                let model = format!("{name}{}", variant.name);
                out.push_str(&format!("    if type(value) is models.{model}:\n"));
                out.push_str(&format!(
                    "        return {{\n            {}: {},\n",
                    py_string(tag),
                    py_string(&variant.wire_name)
                ));
                for field in &variant.fields {
                    let attr = py_name(&field.name);
                    let converted = encode_expr(&format!("value.{attr}"), &field.ty);
                    out.push_str(&mapping_entry_line(
                        "            ",
                        &py_string(&field.wire_name),
                        &converted,
                    ));
                }
                out.push_str("        }\n");
            }
            out.push_str(&raise_line(
                "    ",
                "TypeError",
                &format!("f\"rspyts: expected {name}, got {{type(value).__name__}}\""),
            ));
            out
        }
        (
            Direction::Decode,
            TypeDecl::Enum {
                name,
                tag,
                variants,
                ..
            },
        ) => {
            let mut out = codec_def_line(
                &function,
                "response: _rspyts.Response",
                &format!("models.{name}"),
            );
            out.push_str(&format!(
                "    return _rspyts.enum_from_wire(\n        response,\n        tag={},\n        variants={{\n",
                py_string(tag)
            ));
            for variant in variants {
                out.push_str(&mapping_entry_line(
                    "            ",
                    &py_string(&variant.wire_name),
                    &enum_variant_decoder_name(name, &variant.name),
                ));
            }
            out.push_str("        },\n    )\n");
            for variant in variants {
                out.push_str("\n\n");
                out.push_str(&decode_model(
                    enum_variant_decoder_name(name, &variant.name),
                    &format!("{name}{}", variant.name),
                    &variant.fields,
                    Some((tag, &variant.wire_name)),
                ));
            }
            out
        }
        (_, TypeDecl::ErrorEnum { .. }) => String::new(),
    }
}

fn encode_model(
    function: String,
    name: &str,
    fields: &[FieldDecl],
    tag: Option<(&str, &str)>,
) -> String {
    let mut out = codec_def_line(&function, "value: object", "object");
    out.push_str(&format!("    if type(value) is not models.{name}:\n"));
    out.push_str(&raise_line(
        "        ",
        "TypeError",
        &format!("f\"rspyts: expected {name}, got {{type(value).__name__}}\""),
    ));
    out.push_str("    encoded: dict[str, object] = {}\n");
    if let Some((tag_name, tag_value)) = tag {
        out.push_str(&assignment_line(
            "    ",
            &format!("encoded[{}]", py_string(tag_name)),
            &py_string(tag_value),
        ));
    }
    for field in fields {
        let attr = py_name(&field.name);
        let value = format!("value.{attr}");
        let encoded = encode_expr(&value, &field.ty);
        if field.required {
            out.push_str(&assignment_line(
                "    ",
                &format!("encoded[{}]", py_string(&field.wire_name)),
                &encoded,
            ));
        } else {
            out.push_str(&format!("    if {value} is not None:\n"));
            out.push_str(&assignment_line(
                "        ",
                &format!("encoded[{}]", py_string(&field.wire_name)),
                &encoded,
            ));
        }
    }
    out.push_str("    return encoded\n");
    out
}

fn decode_model(
    function: String,
    name: &str,
    fields: &[FieldDecl],
    tag: Option<(&str, &str)>,
) -> String {
    let mut allowed: Vec<String> = fields
        .iter()
        .map(|field| py_string(&field.wire_name))
        .collect();
    if let Some((tag_name, _)) = tag {
        allowed.push(py_string(tag_name));
    }
    let mut required: Vec<String> = fields
        .iter()
        .filter(|field| field.required)
        .map(|field| py_string(&field.wire_name))
        .collect();
    if let Some((tag_name, _)) = tag {
        required.push(py_string(tag_name));
    }
    let mut out = codec_def_line(
        &function,
        "response: _rspyts.Response",
        &format!("models.{name}"),
    );
    out.push_str("    obj = _rspyts.map_from_wire(response)\n");
    out.push_str(&set_operation_line(
        "    ",
        "unknown",
        "set(obj) - ",
        &allowed,
        "",
    ));
    out.push_str("    if unknown:\n");
    out.push_str(&raise_line(
        "        ",
        "TypeError",
        &format!("f\"rspyts: unexpected {name} fields: {{sorted(unknown)!r}}\""),
    ));
    out.push_str(&set_operation_line(
        "    ",
        "missing",
        "",
        &required,
        " - set(obj)",
    ));
    out.push_str("    if missing:\n");
    out.push_str(&raise_line(
        "        ",
        "TypeError",
        &format!("f\"rspyts: missing {name} fields: {{sorted(missing)!r}}\""),
    ));
    if let Some((tag_name, tag_value)) = tag {
        out.push_str(&if_line(
            "    ",
            &format!(
                "_rspyts.string_from_wire(obj[{}]) != {}",
                py_string(tag_name),
                py_string(tag_value)
            ),
        ));
        out.push_str(&raise_line(
            "        ",
            "ValueError",
            &format!("\"rspyts: invalid {name} discriminator\""),
        ));
    }
    out.push_str("    values: dict[str, object] = {}\n");
    for field in fields {
        let key = py_string(&field.wire_name);
        let converted = decode_expr(&format!("obj[{key}]"), &field.ty);
        if field.required {
            out.push_str(&assignment_line(
                "    ",
                &format!("values[{}]", py_string(&py_name(&field.name))),
                &converted,
            ));
        } else {
            out.push_str(&format!("    if {key} in obj:\n"));
            out.push_str(&assignment_line(
                "        ",
                &format!("values[{}]", py_string(&py_name(&field.name))),
                &converted,
            ));
        }
    }
    if let Some((tag_name, tag_value)) = tag {
        out.push_str(&assignment_line(
            "    ",
            &format!("values[{}]", py_string(&py_name(tag_name))),
            &py_string(tag_value),
        ));
    }
    out.push_str(&return_line(
        "    ",
        &format!("models.{name}.model_validate(values, strict=True)"),
    ));
    out
}

fn set_literal(values: &[String]) -> String {
    match values {
        [] => "set()".to_string(),
        [only] => format!("{{{only}}}"),
        _ => format!("{{{}}}", values.join(", ")),
    }
}

fn enum_variant_decoder_name(enum_name: &str, variant_name: &str) -> String {
    format!(
        "_decode_variant_{}_{}",
        py_name(enum_name),
        py_name(variant_name)
    )
}

fn encode_expr(expr: &str, ty: &Ty) -> String {
    let function = match ty {
        Ty::Ref { name } => type_codec_name(Direction::Encode, name),
        _ => format!("_encode_{}", ty_slug(ty)),
    };
    format!("{function}({expr})")
}

fn decode_expr(expr: &str, ty: &Ty) -> String {
    let function = match ty {
        Ty::Ref { name } => type_codec_name(Direction::Decode, name),
        _ => format!("_decode_{}", ty_slug(ty)),
    };
    format!("{function}({expr})")
}

fn encode_leaf_expr(expr: &str, ty: &Ty) -> String {
    match ty {
        Ty::Bool => format!("_rspyts.bool_from_wire(_rspyts.Response({expr}))"),
        Ty::U8 | Ty::U16 | Ty::U32 | Ty::I8 | Ty::I16 | Ty::I32 => {
            let (minimum, maximum) = int_bounds(ty).expect("integer kind");
            format!(
                "_rspyts.bounded_int_from_wire(_rspyts.Response({expr}), minimum={minimum}, maximum={maximum})"
            )
        }
        Ty::I64 => format!("_rspyts.i64_to_wire({expr})"),
        Ty::U64 => format!("_rspyts.u64_to_wire({expr})"),
        Ty::F32 => format!("_rspyts.float_from_wire(_rspyts.Response({expr}), f32=True)"),
        Ty::F64 => format!("_rspyts.float_from_wire(_rspyts.Response({expr}))"),
        Ty::String => format!("_rspyts.string_from_wire(_rspyts.Response({expr}))"),
        Ty::Bytes | Ty::Buf { .. } | Ty::Slice { .. } => expr.to_string(),
        Ty::Unit | Ty::Null => format!("_rspyts.null_from_wire(_rspyts.Response({expr}))"),
        Ty::Json => format!("_rspyts.json_to_wire({expr})"),
        Ty::Ref { .. }
        | Ty::Option { .. }
        | Ty::List { .. }
        | Ty::Map { .. }
        | Ty::Tuple { .. } => unreachable!("composite values use reusable codecs"),
    }
}

fn decode_leaf_expr(expr: &str, ty: &Ty) -> String {
    match ty {
        Ty::Bool => format!("_rspyts.bool_from_wire({expr})"),
        Ty::U8 | Ty::U16 | Ty::U32 | Ty::I8 | Ty::I16 | Ty::I32 => {
            let (minimum, maximum) = int_bounds(ty).expect("integer kind");
            format!("_rspyts.bounded_int_from_wire({expr}, minimum={minimum}, maximum={maximum})")
        }
        Ty::I64 => format!("_rspyts.i64_from_wire({expr})"),
        Ty::U64 => format!("_rspyts.u64_from_wire({expr})"),
        Ty::F32 => format!("_rspyts.float_from_wire({expr}, f32=True)"),
        Ty::F64 => format!("_rspyts.float_from_wire({expr})"),
        Ty::String => format!("_rspyts.string_from_wire({expr})"),
        Ty::Bytes => format!("_rspyts.bytes_from_wire({expr})"),
        Ty::Buf { dt } => format!(
            "_rspyts.buffer_from_wire({expr}, dtype={})",
            py_string(dt.wire_name())
        ),
        Ty::Unit | Ty::Null => format!("_rspyts.null_from_wire({expr})"),
        Ty::Json => format!("_rspyts.json_from_wire({expr})"),
        Ty::Ref { .. }
        | Ty::Option { .. }
        | Ty::List { .. }
        | Ty::Map { .. }
        | Ty::Tuple { .. } => unreachable!("composite values use reusable codecs"),
        Ty::Slice { .. } => unreachable!("slice cannot be a return type"),
    }
}

fn tuple_expr(values: &[String]) -> String {
    let trailing = if values.len() == 1 { "," } else { "" };
    format!("({}{trailing})", values.join(", "))
}

fn error_data_codec(error_name: &str, variant: &rspyts_core::ir::ErrorVariantDecl) -> String {
    let function = error_codec_name(error_name, &variant.name);
    let allowed: Vec<String> = variant
        .fields
        .iter()
        .map(|field| py_string(&field.wire_name))
        .collect();
    let required: Vec<String> = variant
        .fields
        .iter()
        .filter(|field| field.required)
        .map(|field| py_string(&field.wire_name))
        .collect();
    let mut out = codec_def_line(&function, "response: _rspyts.Response", "dict[str, object]");
    out.push_str("    obj = _rspyts.map_from_wire(response)\n");
    out.push_str(&set_operation_line(
        "    ",
        "unknown",
        "set(obj) - ",
        &allowed,
        "",
    ));
    out.push_str("    if unknown:\n");
    out.push_str(&raise_line(
        "        ",
        "TypeError",
        "f\"rspyts: unexpected error data fields: {sorted(unknown)!r}\"",
    ));
    out.push_str(&set_operation_line(
        "    ",
        "missing",
        "",
        &required,
        " - set(obj)",
    ));
    out.push_str("    if missing:\n");
    out.push_str(&raise_line(
        "        ",
        "TypeError",
        "f\"rspyts: missing error data fields: {sorted(missing)!r}\"",
    ));
    out.push_str("    data: dict[str, object] = {}\n");
    for field in &variant.fields {
        let key = py_string(&field.wire_name);
        let decoded = decode_expr(&format!("obj[{key}]"), &field.ty);
        if field.required {
            out.push_str(&assignment_line("    ", &format!("data[{key}]"), &decoded));
        } else {
            out.push_str(&format!("    if {key} in obj:\n"));
            out.push_str(&assignment_line(
                "        ",
                &format!("data[{key}]"),
                &decoded,
            ));
        }
    }
    out.push_str("    return data\n");
    out
}

fn error_codec_name(error_name: &str, variant_name: &str) -> String {
    format!(
        "_decode_error_{}_{}",
        py_name(error_name),
        py_name(variant_name)
    )
}

// ---------------------------------------------------------------- errors.py

fn errors_py(manifest: &Manifest, fingerprint: &str, imports: &BTreeMap<String, String>) -> String {
    let mut local = Vec::new();
    let mut foreign: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for decl in &manifest.types {
        let TypeDecl::ErrorEnum {
            name,
            docs,
            variants,
            ..
        } = decl
        else {
            continue;
        };
        if is_imported(manifest, decl, imports) {
            let mut names = projected_names(decl);
            names.push(py_error_registry_name(name));
            foreign
                .entry(imports[decl.origin()].as_str())
                .or_default()
                .extend(names);
        } else {
            local.push((name, docs, variants));
        }
    }

    let mut out = py_header(fingerprint);
    out.push_str(&format!(
        "\"\"\"\nException classes bridged from `{}`.\n\"\"\"\n",
        manifest.crate_name
    ));
    if local.is_empty() && foreign.is_empty() {
        return out;
    }
    if !local.is_empty() {
        let has_data = local
            .iter()
            .any(|(_, _, variants)| variants.iter().any(|variant| !variant.fields.is_empty()));
        if has_data {
            out.push_str("\nimport typing\n");
        }
        out.push_str("\nimport rspyts\nimport rspyts._internal as _rspyts\n");
        if has_data {
            out.push_str("\nfrom . import _codecs\n");
        }
    }
    for (module, names) in foreign {
        out.push('\n');
        out.push_str(&wrap_reexport_import(
            module,
            &names.into_iter().collect::<Vec<_>>(),
        ));
        out.push('\n');
    }

    for (name, docs, variants) in &local {
        out.push_str("\n\n");
        out.push_str(&error_class(name, "rspyts.BridgeError", docs, None));
        for variant in *variants {
            out.push_str("\n\n");
            out.push_str(&error_class(
                &format!("{name}{}", variant.name),
                name,
                &variant.docs,
                (!variant.fields.is_empty()).then_some(error_codec_name(name, &variant.name)),
            ));
        }
    }
    for (name, _, variants) in &local {
        out.push_str("\n\n");
        out.push_str(&format!(
            "{}: _rspyts.BridgeErrorRegistry = {{\n",
            py_error_registry_name(name)
        ));
        for variant in *variants {
            out.push_str(&format!(
                "    {}: {name}{},\n",
                py_string(&variant.wire_code),
                variant.name
            ));
        }
        out.push_str("}\n");
    }
    out
}

fn error_class(name: &str, base: &str, docs: &str, codec: Option<String>) -> String {
    let noqa = if name.ends_with("Error") {
        ""
    } else {
        "  # noqa: N818"
    };
    let doc = py_docstring(docs, "    ");
    let Some(codec) = codec else {
        if doc.is_empty() {
            return format!("class {name}({base}): ...{noqa}\n");
        }
        return format!("class {name}({base}):{noqa}\n{doc}");
    };
    let mut out = format!("class {name}({base}):{noqa}\n");
    out.push_str(&doc);
    if !doc.is_empty() {
        out.push('\n');
    }
    out.push_str(
        "    def __init__(self, message: str, *, code: str, data: typing.Any | None = None) -> None:\n",
    );
    out.push_str(&format!(
        "        if data is not None:\n            data = _codecs.{codec}(_rspyts.Response(data))\n"
    ));
    out.push_str("        super().__init__(message, code=code, data=data)\n");
    out
}

// ---------------------------------------------------------------- library.py

fn library_py(manifest: &Manifest, fingerprint: &str, library_search: &[String]) -> String {
    let mut out = py_header(fingerprint);
    out.push_str(&format!(
        "\"\"\"\nLoader for the compiled `{}` bridge library.\n\"\"\"\n",
        manifest.crate_name
    ));
    out.push_str("\nimport pathlib\n\nimport rspyts\nimport rspyts._internal as _rspyts\n\n");
    out.push_str("_rspyts.require_emitter_api(3)\n\n");
    out.push_str("LIB = rspyts.Library(\n");
    out.push_str(&format!(
        "    name={},\n",
        py_string(&manifest.crate_name.replace('-', "_"))
    ));
    out.push_str("    search=[\n");
    for directory in library_search {
        out.push_str(&format!("        {},\n", py_string(directory)));
    }
    out.push_str("    ],\n");
    out.push_str("    anchor=pathlib.Path(__file__).parent,\n");
    out.push_str(&format!(
        "    expected_contract_fingerprint={},\n",
        py_string(fingerprint)
    ));
    out.push_str(")\n");
    out
}

// -------------------------------------------------------------- constants.py

fn constants_py(manifest: &Manifest, fingerprint: &str) -> String {
    let mut body = String::new();
    let mut model_imports = BTreeSet::new();
    for constant in &manifest.constants {
        body.push('\n');
        for line in doc_lines(&constant.docs) {
            if line.trim().is_empty() {
                body.push_str("#\n");
            } else {
                body.push_str(&format!("# {line}\n"));
            }
        }
        record_model_uses(manifest, &constant.ty, &mut model_imports);
        body.push_str(&constant_decl(manifest, constant));
    }
    let mut out = py_header(fingerprint);
    out.push_str("\"\"\"\nConstants bridged from Rust.\n\"\"\"\n");
    if !manifest.constants.is_empty() {
        out.push_str("\nimport typing\n");
        if !model_imports.is_empty() {
            out.push('\n');
            out.push_str(&wrap_from_import(
                ".models",
                &model_imports.into_iter().collect::<Vec<_>>(),
            ));
        }
    }
    out.push_str(&body);
    out
}

fn constant_decl(manifest: &Manifest, constant: &rspyts_core::ir::ConstDecl) -> String {
    let annotation = py_type(&constant.ty, false, &|_| false);
    let prefix = format!("{}: typing.Final[{annotation}] = ", constant.name);
    let compact = py_const_expr_compact(manifest, &constant.ty, &constant.value);
    if prefix.len() + compact.len() <= LINE_LIMIT {
        return format!("{prefix}{compact}\n");
    }
    let rendered = py_const_expr_pretty(manifest, &constant.ty, &constant.value, 0, true);
    if prefix.len() <= LINE_LIMIT {
        format!("{prefix}{rendered}\n")
    } else {
        format!(
            "{}: typing.Final[\n    {annotation}\n] = {rendered}\n",
            constant.name
        )
    }
}

fn py_bool(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

fn py_const_expr_compact(manifest: &Manifest, ty: &Ty, value: &serde_json::Value) -> String {
    match ty {
        Ty::Bool => py_bool(value.as_bool().expect("validated bool")).to_string(),
        Ty::U8 | Ty::U16 | Ty::U32 | Ty::I8 | Ty::I16 | Ty::I32 => value.to_string(),
        Ty::I64 | Ty::U64 => value
            .as_str()
            .map(ToString::to_string)
            .unwrap_or_else(|| value.to_string()),
        Ty::F32 | Ty::F64 => {
            let number = value.as_f64().expect("validated float");
            if number == 0.0 {
                "0.0".to_string()
            } else {
                format!("{number:?}")
            }
        }
        Ty::String => py_string(value.as_str().expect("validated string")),
        Ty::Unit | Ty::Null => "None".to_string(),
        Ty::Json => py_json(value),
        Ty::Option { inner } => {
            if value.is_null() {
                "None".to_string()
            } else {
                py_const_expr_compact(manifest, inner, value)
            }
        }
        Ty::List { inner } => format!(
            "[{}]",
            value
                .as_array()
                .expect("validated list")
                .iter()
                .map(|item| py_const_expr_compact(manifest, inner, item))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Ty::Map { value: inner } => format!(
            "{{{}}}",
            value
                .as_object()
                .expect("validated map")
                .iter()
                .map(|(key, item)| format!(
                    "{}: {}",
                    py_string(key),
                    py_const_expr_compact(manifest, inner, item)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Ty::Tuple { items } => {
            let values = value.as_array().expect("validated tuple");
            tuple_expr(
                &items
                    .iter()
                    .zip(values)
                    .map(|(item_ty, item)| py_const_expr_compact(manifest, item_ty, item))
                    .collect::<Vec<_>>(),
            )
        }
        Ty::Ref { name } => match find_type(manifest, name).expect("validated reference") {
            TypeDecl::Newtype { inner, .. } => py_const_expr_compact(manifest, inner, value),
            TypeDecl::StringEnum { .. } => format!(
                "{name}({})",
                py_string(value.as_str().expect("validated enum string"))
            ),
            TypeDecl::Struct { fields, .. } => {
                model_const_compact(name, fields, value, manifest, None)
            }
            TypeDecl::Enum { tag, variants, .. } => {
                let object = value.as_object().expect("validated enum object");
                let wire = object[tag].as_str().expect("validated discriminator");
                let variant = variants
                    .iter()
                    .find(|variant| variant.wire_name == wire)
                    .expect("validated variant");
                model_const_compact(
                    &format!("{name}{}", variant.name),
                    &variant.fields,
                    value,
                    manifest,
                    Some((tag, wire)),
                )
            }
            TypeDecl::ErrorEnum { .. } => unreachable!("error const type"),
        },
        Ty::Bytes | Ty::Buf { .. } | Ty::Slice { .. } => {
            unreachable!("binary and slice constants are rejected")
        }
    }
}

fn model_const_compact(
    name: &str,
    fields: &[FieldDecl],
    value: &serde_json::Value,
    manifest: &Manifest,
    tag: Option<(&str, &str)>,
) -> String {
    let object = value.as_object().expect("validated object");
    let mut args = Vec::new();
    if let Some((tag_name, tag_value)) = tag {
        args.push(format!("{}={}", py_name(tag_name), py_string(tag_value)));
    }
    for field in fields {
        if let Some(field_value) = object.get(&field.wire_name) {
            args.push(format!(
                "{}={}",
                py_name(&field.name),
                py_const_expr_compact(manifest, &field.ty, field_value)
            ));
        }
    }
    format!("{name}({})", args.join(", "))
}

fn py_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "None".to_string(),
        serde_json::Value::Bool(value) => py_bool(*value).to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::String(value) => py_string(value),
        serde_json::Value::Array(values) => format!(
            "[{}]",
            values.iter().map(py_json).collect::<Vec<_>>().join(", ")
        ),
        serde_json::Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!("{}: {}", py_string(key), py_json(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn py_const_expr_pretty(
    manifest: &Manifest,
    ty: &Ty,
    value: &serde_json::Value,
    indent: usize,
    force: bool,
) -> String {
    let compact = py_const_expr_compact(manifest, ty, value);
    if !force && indent + compact.len() <= LINE_LIMIT {
        return compact;
    }
    match ty {
        Ty::Option { inner } if !value.is_null() => {
            py_const_expr_pretty(manifest, inner, value, indent, force)
        }
        Ty::List { inner } => {
            let items = value
                .as_array()
                .expect("validated list")
                .iter()
                .map(|item| py_const_expr_pretty(manifest, inner, item, indent + 4, false))
                .collect();
            multiline_items("[", "]", items, indent)
        }
        Ty::Map { value: inner } => {
            let items = value
                .as_object()
                .expect("validated map")
                .iter()
                .map(|(key, item)| {
                    format!(
                        "{}: {}",
                        py_string(key),
                        py_const_expr_pretty(manifest, inner, item, indent + 4, false)
                    )
                })
                .collect();
            multiline_items("{", "}", items, indent)
        }
        Ty::Tuple { items } => {
            let values = value.as_array().expect("validated tuple");
            let items = items
                .iter()
                .zip(values)
                .map(|(item_ty, item)| {
                    py_const_expr_pretty(manifest, item_ty, item, indent + 4, false)
                })
                .collect();
            multiline_items("(", ")", items, indent)
        }
        Ty::Json => py_json_pretty(value, indent),
        Ty::Ref { name } => match find_type(manifest, name).expect("validated reference") {
            TypeDecl::Newtype { inner, .. } => {
                py_const_expr_pretty(manifest, inner, value, indent, force)
            }
            TypeDecl::Struct { fields, .. } => {
                model_const_pretty(name, fields, value, manifest, None, indent)
            }
            TypeDecl::Enum { tag, variants, .. } => {
                let object = value.as_object().expect("validated enum object");
                let wire = object[tag].as_str().expect("validated discriminator");
                let variant = variants
                    .iter()
                    .find(|variant| variant.wire_name == wire)
                    .expect("validated variant");
                model_const_pretty(
                    &format!("{name}{}", variant.name),
                    &variant.fields,
                    value,
                    manifest,
                    Some((tag, wire)),
                    indent,
                )
            }
            TypeDecl::StringEnum { .. } | TypeDecl::ErrorEnum { .. } => compact,
        },
        _ => compact,
    }
}

fn model_const_pretty(
    name: &str,
    fields: &[FieldDecl],
    value: &serde_json::Value,
    manifest: &Manifest,
    tag: Option<(&str, &str)>,
    indent: usize,
) -> String {
    let object = value.as_object().expect("validated object");
    let mut args = Vec::new();
    if let Some((tag_name, tag_value)) = tag {
        args.push(format!("{}={}", py_name(tag_name), py_string(tag_value)));
    }
    for field in fields {
        if let Some(field_value) = object.get(&field.wire_name) {
            args.push(format!(
                "{}={}",
                py_name(&field.name),
                py_const_expr_pretty(manifest, &field.ty, field_value, indent + 4, false)
            ));
        }
    }
    multiline_items(&format!("{name}("), ")", args, indent)
}

fn py_json_pretty(value: &serde_json::Value, indent: usize) -> String {
    let compact = py_json(value);
    if indent + compact.len() <= LINE_LIMIT {
        return compact;
    }
    match value {
        serde_json::Value::Array(values) => multiline_items(
            "[",
            "]",
            values
                .iter()
                .map(|item| py_json_pretty(item, indent + 4))
                .collect(),
            indent,
        ),
        serde_json::Value::Object(values) => multiline_items(
            "{",
            "}",
            values
                .iter()
                .map(|(key, item)| {
                    format!("{}: {}", py_string(key), py_json_pretty(item, indent + 4))
                })
                .collect(),
            indent,
        ),
        _ => compact,
    }
}

fn multiline_items(open: &str, close: &str, items: Vec<String>, indent: usize) -> String {
    if items.is_empty() {
        return format!("{open}{close}");
    }
    let item_indent = " ".repeat(indent + 4);
    let closing_indent = " ".repeat(indent);
    let mut out = format!("{open}\n");
    for item in items {
        out.push_str(&item_indent);
        out.push_str(&item);
        out.push_str(",\n");
    }
    out.push_str(&closing_indent);
    out.push_str(close);
    out
}

// -------------------------------------------------------------- functions.py

fn functions_py(manifest: &Manifest, fingerprint: &str, codecs: &CodecPlan) -> String {
    let functions: Vec<&FnDecl> = manifest
        .functions
        .iter()
        .filter(|function| function.targets.contains(&Target::Python))
        .collect();
    let mut body = String::new();
    let mut model_imports = BTreeSet::new();
    let mut uses_numpy = false;
    for function in &functions {
        body.push_str("\n\n");
        body.push_str(&function_def(
            manifest,
            function,
            codecs,
            &mut model_imports,
            &mut uses_numpy,
        ));
    }
    let mut out = py_header(fingerprint);
    out.push_str("\"\"\"\nTyped wrappers for bridged free functions.\n\"\"\"\n");
    if !functions.is_empty() {
        if body.contains("typing.") {
            out.push_str("\nimport typing\n");
        }
        if uses_numpy {
            out.push_str("\nimport numpy as np\n");
        }
        if body.contains("errors.") {
            out.push_str("\nfrom . import _codecs, errors, library\n");
        } else {
            out.push_str("\nfrom . import _codecs, library\n");
        }
        if !model_imports.is_empty() {
            out.push_str(&wrap_from_import(
                ".models",
                &model_imports.into_iter().collect::<Vec<_>>(),
            ));
        }
    }
    out.push_str(&body);
    out
}

fn function_def(
    manifest: &Manifest,
    function: &FnDecl,
    codecs: &CodecPlan,
    model_imports: &mut BTreeSet<String>,
    uses_numpy: &mut bool,
) -> String {
    let params = py_params(manifest, &function.params, model_imports, uses_numpy);
    let ret = py_return_type(manifest, &function.ret, model_imports, uses_numpy);
    let mut out = def_line("    ", &function.name, &params, &ret, 0);
    out.push_str(&py_docstring(&function.docs, "    "));
    out.push_str(&call_block(
        "    ",
        &format!("rspyts_fn__{}", function.name),
        &function.params,
        codecs,
        false,
        function.err.as_deref(),
    ));
    out.push_str(&format!(
        "    return _codecs.{}(response)\n",
        codecs.decoder(&function.ret)
    ));
    out
}

// ---------------------------------------------------------------- classes.py

fn classes_py(manifest: &Manifest, fingerprint: &str, codecs: &CodecPlan) -> String {
    let mut body = String::new();
    let mut model_imports = BTreeSet::new();
    let mut uses_numpy = false;
    for class in &manifest.classes {
        body.push_str("\n\n");
        body.push_str(&class_def(
            manifest,
            class,
            codecs,
            &mut model_imports,
            &mut uses_numpy,
        ));
    }
    let mut out = py_header(fingerprint);
    out.push_str("\"\"\"\nHandle classes for bridged Rust objects.\n\"\"\"\n");
    if !manifest.classes.is_empty() {
        out.push_str("\nimport typing\n\n");
        if uses_numpy {
            out.push_str("import numpy as np\n");
        }
        out.push_str("import rspyts._internal as _rspyts\n");
        if body.contains("errors.") {
            out.push_str("\nfrom . import _codecs, errors, library\n");
        } else {
            out.push_str("\nfrom . import _codecs, library\n");
        }
        if !model_imports.is_empty() {
            out.push_str(&wrap_from_import(
                ".models",
                &model_imports.into_iter().collect::<Vec<_>>(),
            ));
        }
    }
    out.push_str(&body);
    out
}

fn class_def(
    manifest: &Manifest,
    class: &ClassDecl,
    codecs: &CodecPlan,
    model_imports: &mut BTreeSet<String>,
    uses_numpy: &mut bool,
) -> String {
    let name = &class.name;
    let statics: Vec<&StaticDecl> = class
        .statics
        .iter()
        .filter(|item| item.targets.contains(&Target::Python))
        .collect();
    let factories: Vec<&&StaticDecl> = statics.iter().filter(|item| item.returns_self).collect();
    let mut out = format!("class {name}:\n");
    out.push_str(&py_docstring(&class.docs, "    "));
    if !out.ends_with(":\n") {
        out.push('\n');
    }
    out.push_str("    handle: int\n\n");

    if let Some(constructor) = &class.constructor {
        let mut params = vec![("self".to_string(), String::new())];
        params.extend(py_params(
            manifest,
            &constructor.params,
            model_imports,
            uses_numpy,
        ));
        out.push_str(&def_line("        ", "__init__", &params, "None", 4));
        out.push_str(&py_docstring(&constructor.docs, "        "));
        out.push_str(&call_block(
            "        ",
            &format!("rspyts_cls__{name}__new"),
            &constructor.params,
            codecs,
            false,
            constructor.err.as_deref(),
        ));
        out.push_str("        self.handle = _rspyts.bounded_int_from_wire(\n            response, minimum=1, maximum=9007199254740991\n        )\n");
    } else {
        let hint = if factories.is_empty() {
            String::new()
        } else {
            format!(
                "; use {}",
                factories
                    .iter()
                    .map(|item| format!("{name}.{}(...)", item.name))
                    .collect::<Vec<_>>()
                    .join(" or ")
            )
        };
        out.push_str(&format!(
            "    def __init__(self) -> None:\n        raise TypeError({})\n",
            py_string(&format!("{name} cannot be constructed directly{hint}"))
        ));
    }

    for static_decl in &statics {
        out.push('\n');
        if static_decl.returns_self {
            out.push_str("    @classmethod\n");
            let mut params = vec![("cls".to_string(), String::new())];
            params.extend(py_params(
                manifest,
                &static_decl.params,
                model_imports,
                uses_numpy,
            ));
            out.push_str(&def_line(
                "        ",
                &static_decl.name,
                &params,
                "typing.Self",
                4,
            ));
            out.push_str(&py_docstring(&static_decl.docs, "        "));
            out.push_str(&call_block(
                "        ",
                &format!("rspyts_cls__{name}__{}", static_decl.name),
                &static_decl.params,
                codecs,
                false,
                static_decl.err.as_deref(),
            ));
            out.push_str("        obj = cls.__new__(cls)\n        obj.handle = _rspyts.bounded_int_from_wire(\n            response, minimum=1, maximum=9007199254740991\n        )\n        return obj\n");
        } else {
            out.push_str("    @staticmethod\n");
            let params = py_params(manifest, &static_decl.params, model_imports, uses_numpy);
            let ret = py_return_type(manifest, &static_decl.ret, model_imports, uses_numpy);
            out.push_str(&def_line("        ", &static_decl.name, &params, &ret, 4));
            out.push_str(&py_docstring(&static_decl.docs, "        "));
            out.push_str(&call_block(
                "        ",
                &format!("rspyts_cls__{name}__{}", static_decl.name),
                &static_decl.params,
                codecs,
                false,
                static_decl.err.as_deref(),
            ));
            out.push_str(&format!(
                "        return _codecs.{}(response)\n",
                codecs.decoder(&static_decl.ret)
            ));
        }
    }

    for method in class
        .methods
        .iter()
        .filter(|item| item.targets.contains(&Target::Python))
    {
        out.push('\n');
        let mut params = vec![("self".to_string(), String::new())];
        params.extend(py_params(
            manifest,
            &method.params,
            model_imports,
            uses_numpy,
        ));
        let ret = py_return_type(manifest, &method.ret, model_imports, uses_numpy);
        out.push_str(&def_line("        ", &method.name, &params, &ret, 4));
        out.push_str(&py_docstring(&method.docs, "        "));
        out.push_str(&call_block(
            "        ",
            &format!("rspyts_cls__{name}__{}", method.name),
            &method.params,
            codecs,
            true,
            method.err.as_deref(),
        ));
        out.push_str(&format!(
            "        return _codecs.{}(response)\n",
            codecs.decoder(&method.ret)
        ));
    }

    out.push_str(&format!(
        "\n    def __copy__(self) -> typing.NoReturn:\n        raise TypeError(\"{name} owns a Rust handle and cannot be copied\")\n\n    def __deepcopy__(self, memo: dict[int, object]) -> typing.NoReturn:\n        raise TypeError(\"{name} owns a Rust handle and cannot be copied\")\n\n    def __reduce__(self) -> typing.NoReturn:\n        raise TypeError(\"{name} owns a Rust handle and cannot be pickled\")\n\n    def __reduce_ex__(self, protocol: typing.SupportsIndex) -> typing.NoReturn:\n        raise TypeError(\"{name} owns a Rust handle and cannot be pickled\")\n\n    def close(self) -> None:\n        \"\"\"Release the Rust object. Safe to call more than once.\"\"\"\n        handle = getattr(self, \"handle\", None)\n        if handle is not None:\n            library.LIB.call_drop(\"rspyts_cls__{name}__drop\", handle)\n\n    def __enter__(self) -> typing.Self:\n        return self\n\n    def __exit__(self, *exc: object) -> None:\n        self.close()\n\n    def __del__(self) -> None:\n        try:\n            self.close()\n        except Exception:\n            pass\n"
    ));
    out
}

// ------------------------------------------------------------------ wrappers

fn py_params(
    manifest: &Manifest,
    params: &[ParamDecl],
    model_imports: &mut BTreeSet<String>,
    uses_numpy: &mut bool,
) -> Vec<(String, String)> {
    params
        .iter()
        .map(|param| {
            record_model_uses(manifest, &param.ty, model_imports);
            *uses_numpy |= uses_ndarray(&param.ty);
            (
                py_param_name(&param.name),
                py_type(&param.ty, false, &|_| false),
            )
        })
        .collect()
}

fn py_return_type(
    manifest: &Manifest,
    ty: &Ty,
    model_imports: &mut BTreeSet<String>,
    uses_numpy: &mut bool,
) -> String {
    record_model_uses(manifest, ty, model_imports);
    *uses_numpy |= uses_ndarray(ty);
    py_type(ty, false, &|_| false)
}

fn record_model_uses(manifest: &Manifest, ty: &Ty, model_imports: &mut BTreeSet<String>) {
    let mut refs = BTreeSet::new();
    collect_refs(ty, &mut refs);
    for name in refs {
        let Some(decl) = find_type(manifest, &name) else {
            continue;
        };
        match decl {
            TypeDecl::ErrorEnum { .. } => {}
            _ => {
                model_imports.insert(name);
            }
        }
    }
}

fn uses_ndarray(ty: &Ty) -> bool {
    match ty {
        Ty::Buf { .. } | Ty::Slice { .. } => true,
        Ty::Option { inner } | Ty::List { inner } => uses_ndarray(inner),
        Ty::Map { value } => uses_ndarray(value),
        Ty::Tuple { items } => items.iter().any(uses_ndarray),
        _ => false,
    }
}

fn call_block(
    indent: &str,
    symbol: &str,
    params: &[ParamDecl],
    codecs: &CodecPlan,
    handle: bool,
    error: Option<&str>,
) -> String {
    let args: Vec<String> = params
        .iter()
        .filter(|param| !matches!(param.ty, Ty::Slice { .. }))
        .map(|param| {
            format!(
                "{}: _codecs.{}({})",
                py_string(&param.wire_name),
                codecs.encoder(&param.ty),
                py_param_name(&param.name)
            )
        })
        .collect();
    let slices: Vec<(String, Dtype)> = params
        .iter()
        .filter_map(|param| match param.ty {
            Ty::Slice { dt } => Some((py_param_name(&param.name), dt)),
            _ => None,
        })
        .collect();
    let mut out = format!("{indent}response = library.LIB.call(\n{indent}    {symbol:?},\n");
    out.push_str(&args_dict(&args, &format!("{indent}    ")));
    if !slices.is_empty() {
        let values = slices
            .iter()
            .map(|(name, dtype)| format!("({name}, {:?})", dtype.wire_name()))
            .collect::<Vec<_>>();
        let trailing = if values.len() == 1 { "," } else { "" };
        out.push_str(&format!(
            "{indent}    slices=({}{trailing}),\n",
            values.join(", ")
        ));
    }
    if handle {
        out.push_str(&format!("{indent}    handle=self.handle,\n"));
    }
    if let Some(error) = error {
        out.push_str(&format!(
            "{indent}    error_types=errors.{},\n",
            py_error_registry_name(error)
        ));
    }
    out.push_str(&format!("{indent})\n"));
    out
}

fn args_dict(entries: &[String], indent: &str) -> String {
    if entries.is_empty() {
        return format!("{indent}{{}},\n");
    }
    let one = format!("{indent}{{{}}},", entries.join(", "));
    if one.len() <= LINE_LIMIT {
        return one + "\n";
    }
    let mut out = format!("{indent}{{\n");
    for entry in entries {
        out.push_str(&format!("{indent}    {entry},\n"));
    }
    out.push_str(&format!("{indent}}},\n"));
    out
}

fn def_line(
    body_indent: &str,
    name: &str,
    params: &[(String, String)],
    ret: &str,
    def_indent: usize,
) -> String {
    let indent = " ".repeat(def_indent);
    let rendered = params
        .iter()
        .map(|(name, annotation)| {
            if annotation.is_empty() {
                name.clone()
            } else {
                format!("{name}: {annotation}")
            }
        })
        .collect::<Vec<_>>();
    let one = format!("{indent}def {name}({}) -> {ret}:", rendered.join(", "));
    if one.len() <= LINE_LIMIT {
        return one + "\n";
    }
    let mut out = format!("{indent}def {name}(\n");
    for param in rendered {
        out.push_str(&format!("{body_indent}{param},\n"));
    }
    out.push_str(&format!("{indent}) -> {ret}:\n"));
    out
}

const PY_MODULE_NAMES: &[&str] = &["errors", "library", "models", "np"];

fn py_param_name(name: &str) -> String {
    let escaped = py_name(name);
    if PY_MODULE_NAMES.contains(&escaped.as_str()) {
        format!("{escaped}_")
    } else {
        escaped
    }
}

// ---------------------------------------------------------------- __init__.py

fn init_py(manifest: &Manifest, fingerprint: &str) -> String {
    let mut models = Vec::new();
    let mut errors = Vec::new();
    for decl in &manifest.types {
        if let TypeDecl::ErrorEnum { name, .. } = decl {
            errors.extend(projected_names(decl));
            errors.push(py_error_registry_name(name));
        } else {
            models.extend(projected_names(decl));
        }
    }
    let mut sections = vec![
        (
            ".classes",
            manifest
                .classes
                .iter()
                .map(|class| class.name.clone())
                .collect::<Vec<_>>(),
        ),
        (
            ".constants",
            manifest
                .constants
                .iter()
                .map(|constant| constant.name.clone())
                .collect::<Vec<_>>(),
        ),
        (".errors", errors),
        (
            ".functions",
            manifest
                .functions
                .iter()
                .filter(|function| function.targets.contains(&Target::Python))
                .map(|function| function.name.clone())
                .collect::<Vec<_>>(),
        ),
        (".models", models),
    ];
    let mut out = py_header(fingerprint);
    out.push_str(&format!(
        "\"\"\"\nGenerated bridge surface for `{}`.\n\"\"\"\n",
        manifest.crate_name
    ));
    let mut started_imports = false;
    for (module, names) in &mut sections {
        if names.is_empty() {
            continue;
        }
        names.sort();
        if !started_imports {
            out.push('\n');
            started_imports = true;
        }
        out.push_str(&wrap_from_import(module, names));
    }
    let mut all = sections
        .into_iter()
        .flat_map(|(_, names)| names)
        .collect::<Vec<_>>();
    let screaming = |name: &str| {
        !name.chars().any(|character| character.is_ascii_lowercase())
            && name.chars().any(|character| character.is_ascii_uppercase())
    };
    all.sort_by(|left, right| {
        (!screaming(left), left.as_str()).cmp(&(!screaming(right), right.as_str()))
    });
    out.push_str("\n__all__ = [\n");
    for name in all {
        out.push_str(&format!("    {name:?},\n"));
    }
    out.push_str("]\n");
    out
}

// ---------------------------------------------------------------- formatting

fn py_string(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization is infallible")
}

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

fn wrap_reexport_import(module: &str, names: &[String]) -> String {
    let one = format!("from {module} import {}  # noqa: F401", names.join(", "));
    if one.len() <= LINE_LIMIT {
        return one;
    }
    let mut out = format!("from {module} import (  # noqa: F401\n");
    for name in names {
        out.push_str(&format!("    {name},\n"));
    }
    out.push(')');
    out
}

#[cfg(test)]
mod tests {
    use super::super::test_manifest::{binary_manifest, exact_manifest, manifest, manifest_hash};
    use super::*;

    fn emitted(manifest: &Manifest) -> Vec<(&'static str, String)> {
        emit(
            manifest,
            &manifest_hash(manifest),
            &["lib".to_string()],
            &BTreeMap::new(),
        )
    }

    fn file<'a>(files: &'a [(&str, String)], name: &str) -> &'a str {
        &files.iter().find(|(path, _)| *path == name).unwrap().1
    }

    #[test]
    fn emits_private_compact_codecs_and_one_call_path() {
        let files = emitted(&manifest());
        let codecs = file(&files, "_codecs.py");
        let functions = file(&files, "functions.py");
        let init = file(&files, "__init__.py");
        assert!(codecs.contains("import rspyts._internal as _rspyts"));
        assert!(codecs.contains("def _encode_type_query_options"));
        assert!(codecs.contains("def _decode_type_query_options"));
        assert!(functions.contains("library.LIB.call("));
        assert!(!functions.contains("call_raw"));
        assert!(!init.contains("_codecs"));
    }

    #[test]
    fn named_refs_are_called_not_expanded_in_wrappers() {
        let files = emitted(&manifest());
        let functions = file(&files, "functions.py");
        assert!(functions.contains("_codecs._encode_type_query_options(options)"));
        assert!(!functions.contains("model_dump"));
        assert!(!functions.contains("minimumValue"));
    }

    #[test]
    fn models_are_host_only_and_required_options_stay_required() {
        let files = emitted(&manifest());
        let models = file(&files, "models.py");
        assert!(!models.contains("_from_wire"));
        assert!(!models.contains("_codecs"));
        assert!(models.contains("metadata: typing.Any"));
    }

    #[test]
    fn explicit_response_context_reaches_nested_attachments() {
        let manifest = binary_manifest();
        let files = emitted(&manifest);
        let codecs = file(&files, "_codecs.py");
        assert!(codecs.contains("_rspyts.bytes_from_wire"));
        assert!(codecs.contains("_rspyts.buffer_from_wire"));
        assert!(codecs.contains("_rspyts.list_from_wire"));
        assert!(codecs.contains("_rspyts.map_from_wire"));
    }

    #[test]
    fn exact_integers_are_plain_host_ints_with_strict_codecs() {
        let manifest = exact_manifest();
        let files = emitted(&manifest);
        let models = file(&files, "models.py");
        let codecs = file(&files, "_codecs.py");
        assert!(models.contains("delta: int"));
        assert!(!models.contains("rspyts.I64"));
        assert!(codecs.contains("_rspyts.i64_to_wire"));
        assert!(codecs.contains("_rspyts.u64_from_wire"));
    }

    #[test]
    fn loader_pins_emitter_api_and_contract_fingerprint() {
        let manifest = manifest();
        let hash = manifest_hash(&manifest);
        let files = emit(&manifest, &hash, &["lib".to_string()], &BTreeMap::new());
        let library = file(&files, "library.py");
        assert!(library.contains("_rspyts.require_emitter_api(3)"));
        assert!(library.contains(&format!("expected_contract_fingerprint={hash:?}")));
    }

    #[test]
    fn codecs_are_readable_instead_of_single_giant_lines() {
        let manifest = binary_manifest();
        let files = emitted(&manifest);
        let codecs = file(&files, "_codecs.py");
        assert!(codecs.lines().all(|line| line.len() <= MAX_LINE_LENGTH));
        assert!(codecs.lines().count() > 40);
    }

    #[test]
    fn every_generated_python_file_respects_the_line_limit() {
        for manifest in [manifest(), binary_manifest(), exact_manifest()] {
            for (path, source) in emitted(&manifest) {
                let longest = source.lines().map(str::len).max().unwrap_or_default();
                assert!(
                    longest <= MAX_LINE_LENGTH,
                    "{path} contains a {longest}-column generated line"
                );
            }
        }
    }

    #[test]
    fn constants_use_python_boolean_literals() {
        assert_eq!(py_bool(true), "True");
        assert_eq!(py_bool(false), "False");
        assert_eq!(py_json(&serde_json::json!([true, false])), "[True, False]");
    }
}
