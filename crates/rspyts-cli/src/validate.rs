//! Manifest validation.
//!
//! The `#[bridge]` macros reject these shapes at compile time, so a
//! well-formed module never trips validation — this is the CLI's
//! defense against hand-rolled shims, macro bugs, and version skew.
//! Every check reports the offending item by name; all problems are
//! collected before failing so one run shows everything.

use crate::emit::util::{pascal, py_error_registry_name, ts_error_registry_name};
use heck::ToLowerCamelCase;
use rspyts_core::ir::{ClassDecl, FieldDecl, Manifest, ParamDecl, Target, Ty, TypeDecl};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Reject any manifest that does not use this CLI's exact ABI vocabulary.
pub(crate) fn validate_manifest_abi(value: &str) -> anyhow::Result<()> {
    let (major, minor) = value
        .split_once('.')
        .ok_or_else(|| anyhow::anyhow!("manifest ABI `{value}` is not in `major.minor` form"))?;
    anyhow::ensure!(
        !major.is_empty()
            && !minor.is_empty()
            && major.bytes().all(|byte| byte.is_ascii_digit())
            && minor.bytes().all(|byte| byte.is_ascii_digit()),
        "manifest ABI `{value}` is not in `major.minor` form"
    );
    anyhow::ensure!(
        value == rspyts_core::ABI_VERSION_STR,
        "unsupported manifest ABI `{value}`; this rspyts CLI requires `{}`",
        rspyts_core::ABI_VERSION_STR
    );
    Ok(())
}

/// What kind of declaration a name refers to.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Newtype,
    Struct,
    Enum,
    StringEnum,
    ErrorEnum,
}

impl Kind {
    fn describe(self) -> &'static str {
        match self {
            Kind::Newtype => "newtype",
            Kind::Struct => "struct",
            Kind::Enum => "enum",
            Kind::StringEnum => "string enum",
            Kind::ErrorEnum => "error enum",
        }
    }
}

fn declaration_kind(declaration: &TypeDecl) -> Kind {
    match declaration {
        TypeDecl::Newtype { .. } => Kind::Newtype,
        TypeDecl::Struct { .. } => Kind::Struct,
        TypeDecl::Enum { .. } => Kind::Enum,
        TypeDecl::StringEnum { .. } => Kind::StringEnum,
        TypeDecl::ErrorEnum { .. } => Kind::ErrorEnum,
    }
}

/// Where a [`Ty`] appears; determines which shapes are legal.
#[derive(Clone, Copy, PartialEq)]
enum Pos {
    /// Top level of a function/constructor/method parameter.
    ParamTop,
    /// Nested inside a parameter type.
    ParamNested,
    /// Top level of a return type.
    ReturnTop,
    /// Nested inside a return type.
    ReturnNested,
    /// A struct, enum-variant, or error-variant field.
    Field,
}

impl Pos {
    fn nested(self) -> Pos {
        match self {
            Pos::ParamTop | Pos::ParamNested => Pos::ParamNested,
            Pos::ReturnTop | Pos::ReturnNested => Pos::ReturnNested,
            Pos::Field => Pos::Field,
        }
    }
}

/// Validate `manifest`, returning one error carrying every finding.
pub fn validate(manifest: &Manifest) -> anyhow::Result<()> {
    validate_manifest_abi(&manifest.abi)?;
    let attachment_types = attachment_types(manifest);
    let mut v = Validator {
        kinds: BTreeMap::new(),
        attachment_types,
        errors: Vec::new(),
    };
    for ty in &manifest.types {
        let kind = declaration_kind(ty);
        v.kinds.insert(ty.name().to_string(), kind);
    }
    v.check_newtype_cycles(manifest);
    v.check_structural_cycles(manifest);
    v.check_projected_names(manifest);

    for ty in &manifest.types {
        match ty {
            TypeDecl::Newtype { name, inner, .. } => {
                v.check(inner, Pos::Field, &format!("newtype `{name}` inner type"));
            }
            TypeDecl::Struct { name, fields, .. } => {
                v.check_wire_names(&format!("struct `{name}`"), fields, None);
                v.check_fields(&format!("struct `{name}`"), fields);
            }
            TypeDecl::Enum {
                name,
                tag,
                variants,
                ..
            } => {
                v.check_unique_strings(
                    &format!("enum `{name}` variant wire names"),
                    variants.iter().map(|variant| variant.wire_name.as_str()),
                );
                for variant in variants {
                    v.check_wire_names(
                        &format!("enum `{name}` variant `{}`", variant.name),
                        &variant.fields,
                        Some(tag),
                    );
                    v.check_fields(
                        &format!("enum `{name}` variant `{}`", variant.name),
                        &variant.fields,
                    );
                }
            }
            TypeDecl::StringEnum { name, variants, .. } => {
                v.check_unique_strings(
                    &format!("string enum `{name}` wire values"),
                    variants.iter().map(|variant| variant.wire_name.as_str()),
                );
            }
            TypeDecl::ErrorEnum { name, variants, .. } => {
                v.check_unique_strings(
                    &format!("error enum `{name}` wire codes"),
                    variants.iter().map(|variant| variant.wire_code.as_str()),
                );
                for variant in variants {
                    v.check_wire_names(
                        &format!("error enum `{name}` variant `{}`", variant.name),
                        &variant.fields,
                        None,
                    );
                    v.check_fields(
                        &format!("error enum `{name}` variant `{}`", variant.name),
                        &variant.fields,
                    );
                    for field in &variant.fields {
                        if v.contains_attachment(&field.ty) {
                            v.errors.push(format!(
                                "error enum `{name}` variant `{}` field `{}` contains `Buf` or `Bytes`, but error envelopes cannot carry attachment tails",
                                variant.name, field.name
                            ));
                        }
                    }
                }
            }
        }
    }

    for c in &manifest.constants {
        v.check(&c.ty, Pos::Field, &format!("constant `{}`", c.name));
        if v.contains_attachment(&c.ty) {
            v.errors.push(format!(
                "constant `{}` contains `Buf` or `Bytes`, but package constants have no attachment tail",
                c.name
            ));
        }
        v.check_const(manifest, &c.ty, &c.value, &format!("constant `{}`", c.name));
    }

    for f in &manifest.functions {
        let ctx = format!("function `{}`", f.name);
        v.check_param_wire_names(&ctx, &f.params);
        v.check_params(&ctx, &f.params);
        v.check(&f.ret, Pos::ReturnTop, &format!("{ctx} return type"));
        v.check_err(&ctx, f.err.as_deref());
        v.check_targets(&ctx, &f.targets);
    }

    for class in &manifest.classes {
        v.check_class_member_names(class);
        if let Some(ctor) = &class.constructor {
            let ctx = format!("class `{}` constructor", class.name);
            v.check_param_wire_names(&ctx, &ctor.params);
            v.check_params(&ctx, &ctor.params);
            v.check_err(&ctx, ctor.err.as_deref());
        } else if !class.statics.iter().any(|s| s.returns_self) {
            v.errors.push(format!(
                "class `{}` has neither a constructor nor a factory (a static returning `Self`) \
                 — it can never be constructed",
                class.name
            ));
        }
        for m in &class.methods {
            let ctx = format!("class `{}` method `{}`", class.name, m.name);
            v.check_param_wire_names(&ctx, &m.params);
            v.check_params(&ctx, &m.params);
            v.check(&m.ret, Pos::ReturnTop, &format!("{ctx} return type"));
            v.check_err(&ctx, m.err.as_deref());
            v.check_targets(&ctx, &m.targets);
        }
        for s in &class.statics {
            let ctx = format!("class `{}` static `{}`", class.name, s.name);
            v.check_param_wire_names(&ctx, &s.params);
            v.check_params(&ctx, &s.params);
            // A factory's `ret` is ignored: the envelope carries a handle.
            if !s.returns_self {
                v.check(&s.ret, Pos::ReturnTop, &format!("{ctx} return type"));
            }
            v.check_err(&ctx, s.err.as_deref());
            v.check_targets(&ctx, &s.targets);
        }
    }

    if v.errors.is_empty() {
        Ok(())
    } else {
        let mut msg = format!("invalid manifest ({} problem(s)):", v.errors.len());
        for e in &v.errors {
            write!(msg, "\n  - {e}").expect("writing to String cannot fail");
        }
        Err(anyhow::anyhow!(msg))
    }
}

struct Validator {
    kinds: BTreeMap<String, Kind>,
    attachment_types: BTreeSet<String>,
    errors: Vec<String>,
}

impl Validator {
    /// Validate every public name after projecting the manifest into each
    /// host language.  The two maps intentionally stay separate: a spelling
    /// used only by Python must not make an otherwise-safe TypeScript-only
    /// declaration invalid (and vice versa).
    fn check_projected_names(&mut self, manifest: &Manifest) {
        let mut python = BTreeMap::<String, Vec<String>>::new();
        let mut typescript = BTreeMap::<String, Vec<String>>::new();

        add_projected_name(
            &mut python,
            "__all__",
            "generated Python package export list",
        );
        add_projected_name(
            &mut typescript,
            "createClient",
            "generated TypeScript client factory",
        );
        add_projected_name(
            &mut typescript,
            format!("{}Client", pascal(&manifest.crate_name)),
            format!(
                "generated TypeScript client interface for crate `{}`",
                manifest.crate_name
            ),
        );

        for declaration in &manifest.types {
            let kind = declaration_kind(declaration);
            let owner = format!(
                "{} `{}` from `{}`",
                kind.describe(),
                declaration.name(),
                declaration.origin()
            );
            add_projected_name(&mut python, declaration.name(), &owner);
            add_projected_name(&mut typescript, declaration.name(), &owner);

            match declaration {
                TypeDecl::Enum { name, variants, .. } => {
                    // Python emits one model class per data-enum variant.
                    // TypeScript represents data enums as a union only.
                    for variant in variants {
                        add_projected_name(
                            &mut python,
                            format!("{name}{}", variant.name),
                            format!(
                                "enum `{name}` variant `{}` from `{}`",
                                variant.name,
                                declaration.origin()
                            ),
                        );
                    }
                }
                TypeDecl::ErrorEnum { name, variants, .. } => {
                    // Both hosts expose one concrete error class per variant.
                    for variant in variants {
                        let projected = format!("{name}{}", variant.name);
                        let variant_owner = format!(
                            "error enum `{name}` variant `{}` from `{}`",
                            variant.name,
                            declaration.origin()
                        );
                        add_projected_name(&mut python, &projected, &variant_owner);
                        add_projected_name(&mut typescript, projected, variant_owner);
                    }
                    add_projected_name(
                        &mut python,
                        py_error_registry_name(name),
                        format!(
                            "generated Python registry for error enum `{name}` from `{}`",
                            declaration.origin()
                        ),
                    );
                    add_projected_name(
                        &mut typescript,
                        ts_error_registry_name(name),
                        format!(
                            "generated TypeScript registry for error enum `{name}` from `{}`",
                            declaration.origin()
                        ),
                    );
                }
                TypeDecl::Newtype { .. }
                | TypeDecl::Struct { .. }
                | TypeDecl::StringEnum { .. } => {}
            }
        }

        for class in &manifest.classes {
            let owner = format!("class `{}`", class.name);
            add_projected_name(&mut python, &class.name, &owner);
            add_projected_name(&mut typescript, &class.name, &owner);
        }
        for constant in &manifest.constants {
            let owner = format!("constant `{}` from `{}`", constant.name, constant.origin);
            add_projected_name(&mut python, &constant.name, &owner);
            add_projected_name(&mut typescript, &constant.name, owner);
        }
        for function in &manifest.functions {
            if function.targets.contains(&Target::Python) {
                add_projected_name(
                    &mut python,
                    &function.name,
                    format!("function `{}`", function.name),
                );
            }
        }

        report_projected_collisions(&mut self.errors, "Python", "package root export", python);
        report_projected_collisions(
            &mut self.errors,
            "TypeScript",
            "module root export",
            typescript,
        );

        // TypeScript free functions and classes are properties of the client,
        // not root values. They still share one projected member namespace.
        let mut client = BTreeMap::<String, Vec<String>>::new();
        for function in &manifest.functions {
            if function.targets.contains(&Target::Typescript) {
                add_projected_name(
                    &mut client,
                    function.name.to_lower_camel_case(),
                    format!("function `{}`", function.name),
                );
            }
        }
        for class in &manifest.classes {
            add_projected_name(
                &mut client,
                &class.name,
                format!("class `{}` constructor property", class.name),
            );
        }
        report_projected_collisions(&mut self.errors, "TypeScript", "client member", client);
    }

    fn check_class_member_names(&mut self, class: &ClassDecl) {
        let mut python = BTreeMap::<String, Vec<String>>::new();
        for reserved in [
            "handle",
            "__init__",
            "__new__",
            "__copy__",
            "__deepcopy__",
            "__reduce__",
            "__reduce_ex__",
            "close",
            "__enter__",
            "__exit__",
            "__del__",
        ] {
            add_projected_name(
                &mut python,
                reserved,
                format!("generated runtime member `{reserved}`"),
            );
        }
        for method in &class.methods {
            if method.targets.contains(&Target::Python) {
                add_projected_name(
                    &mut python,
                    &method.name,
                    format!("method `{}`", method.name),
                );
            }
        }
        for static_method in &class.statics {
            if static_method.targets.contains(&Target::Python) {
                add_projected_name(
                    &mut python,
                    &static_method.name,
                    format!("static `{}`", static_method.name),
                );
            }
        }
        report_projected_collisions(
            &mut self.errors,
            "Python",
            &format!("class `{}` member", class.name),
            python,
        );

        let mut ts_instance = BTreeMap::<String, Vec<String>>::new();
        for reserved in ["constructor", "free", "dispose"] {
            add_projected_name(
                &mut ts_instance,
                reserved,
                format!("reserved generated lifecycle member `{reserved}`"),
            );
        }
        for method in &class.methods {
            if method.targets.contains(&Target::Typescript) {
                add_projected_name(
                    &mut ts_instance,
                    method.name.to_lower_camel_case(),
                    format!("method `{}`", method.name),
                );
            }
        }
        report_projected_collisions(
            &mut self.errors,
            "TypeScript",
            &format!("class `{}` instance member", class.name),
            ts_instance,
        );

        let mut ts_static = BTreeMap::<String, Vec<String>>::new();
        add_projected_name(
            &mut ts_static,
            "prototype",
            "reserved JavaScript class static `prototype`",
        );
        for static_method in &class.statics {
            if static_method.targets.contains(&Target::Typescript) {
                add_projected_name(
                    &mut ts_static,
                    static_method.name.to_lower_camel_case(),
                    format!("static `{}`", static_method.name),
                );
            }
        }
        report_projected_collisions(
            &mut self.errors,
            "TypeScript",
            &format!("class `{}` static member", class.name),
            ts_static,
        );
    }

    fn check_newtype_cycles(&mut self, manifest: &Manifest) {
        let aliases: BTreeMap<&str, &Ty> = manifest
            .types
            .iter()
            .filter_map(|decl| match decl {
                TypeDecl::Newtype { name, inner, .. } => Some((name.as_str(), inner)),
                _ => None,
            })
            .collect();
        for name in aliases.keys().copied() {
            let mut path = Vec::new();
            if alias_reaches(name, name, &aliases, &mut path) {
                self.errors.push(format!(
                    "newtype `{name}` is recursive through transparent aliases — newtypes must resolve to a concrete non-recursive wire shape"
                ));
            }
        }
    }

    fn check_structural_cycles(&mut self, manifest: &Manifest) {
        let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut structural = BTreeSet::new();
        for declaration in &manifest.types {
            let mut refs = Vec::new();
            match declaration {
                TypeDecl::Newtype { inner, .. } => collect_refs(inner, &mut refs),
                TypeDecl::Struct { name, fields, .. } => {
                    structural.insert(name.clone());
                    for field in fields {
                        collect_refs(&field.ty, &mut refs);
                    }
                }
                TypeDecl::Enum { name, variants, .. } => {
                    structural.insert(name.clone());
                    for variant in variants {
                        for field in &variant.fields {
                            collect_refs(&field.ty, &mut refs);
                        }
                    }
                }
                TypeDecl::StringEnum { .. } | TypeDecl::ErrorEnum { .. } => {}
            }
            graph.insert(
                declaration.name().to_string(),
                refs.into_iter().map(str::to_string).collect(),
            );
        }

        let mut reported = BTreeSet::new();
        for start in structural {
            if reported.contains(&start) {
                continue;
            }
            let mut path = vec![start.clone()];
            let mut visiting = BTreeSet::from([start.clone()]);
            if reference_cycle(&start, &start, &graph, &mut visiting, &mut path) {
                reported.extend(path.iter().cloned());
                self.errors.push(format!(
                    "data type `{start}` participates in a recursive reference cycle `{}` — data contracts must have finite non-recursive wire shapes",
                    path.join(" -> ")
                ));
            }
        }
    }

    fn check_params(&mut self, ctx: &str, params: &[ParamDecl]) {
        for p in params {
            self.check(
                &p.ty,
                Pos::ParamTop,
                &format!("{ctx} parameter `{}`", p.name),
            );
        }
    }

    fn check_param_wire_names(&mut self, ctx: &str, params: &[ParamDecl]) {
        self.check_unique_strings(
            &format!("{ctx} parameter wire names"),
            params
                .iter()
                .filter(|param| !matches!(param.ty, Ty::Slice { .. }))
                .map(|param| param.wire_name.as_str()),
        );
    }

    fn check_fields(&mut self, ctx: &str, fields: &[FieldDecl]) {
        for f in fields {
            if !f.required && !matches!(f.ty, Ty::Option { .. }) {
                self.errors.push(format!(
                    "{ctx} field `{}` has inconsistent required metadata — only a direct `Option<T>` field may be omitted",
                    f.name
                ));
            }
            self.check(&f.ty, Pos::Field, &format!("{ctx} field `{}`", f.name));
        }
    }

    fn check_wire_names(&mut self, ctx: &str, fields: &[FieldDecl], tag: Option<&str>) {
        self.check_unique_strings(
            &format!("{ctx} field wire names"),
            fields.iter().map(|field| field.wire_name.as_str()),
        );
        for field in fields {
            if tag == Some(field.wire_name.as_str()) {
                self.errors.push(format!(
                    "{ctx} field `{}` uses discriminator key `{}` — tag and data fields must be distinct",
                    field.name, field.wire_name
                ));
            }
        }
    }

    fn check_unique_strings<'a>(&mut self, ctx: &str, values: impl Iterator<Item = &'a str>) {
        let mut seen = BTreeSet::new();
        for value in values {
            if !seen.insert(value) {
                self.errors
                    .push(format!("{ctx} contain duplicate value `{value}`"));
            }
        }
    }

    fn contains_attachment(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Bytes | Ty::Buf { .. } => true,
            Ty::Option { inner } | Ty::List { inner } => self.contains_attachment(inner),
            Ty::Map { value } => self.contains_attachment(value),
            Ty::Tuple { items } => items.iter().any(|item| self.contains_attachment(item)),
            Ty::Ref { name } => self.attachment_types.contains(name),
            _ => false,
        }
    }

    fn check_const(&mut self, manifest: &Manifest, ty: &Ty, value: &Value, ctx: &str) {
        match ty {
            Ty::Null => {
                if !value.is_null() {
                    self.errors
                        .push(format!("{ctx} must contain the JSON null value"));
                }
            }
            Ty::F32 | Ty::F64 => {
                if value.as_f64().is_none_or(|number| !number.is_finite()) {
                    self.errors.push(format!(
                        "{ctx} must contain a finite JSON number; NaN and infinities are only portable through binary buffers"
                    ));
                }
            }
            Ty::Option { inner } => {
                if !value.is_null() {
                    self.check_const(manifest, inner, value, ctx);
                }
            }
            Ty::List { inner } => {
                if let Some(items) = value.as_array() {
                    for (index, item) in items.iter().enumerate() {
                        self.check_const(manifest, inner, item, &format!("{ctx}[{index}]"));
                    }
                }
            }
            Ty::Map { value: inner } => {
                if let Some(items) = value.as_object() {
                    for (key, item) in items {
                        self.check_const(manifest, inner, item, &format!("{ctx}.{key}"));
                    }
                }
            }
            Ty::Tuple { items } => {
                if let Some(values) = value.as_array() {
                    for (index, (item_ty, item)) in items.iter().zip(values).enumerate() {
                        self.check_const(manifest, item_ty, item, &format!("{ctx}[{index}]"));
                    }
                }
            }
            Ty::Ref { name } => {
                let Some(decl) = manifest.types.iter().find(|decl| decl.name() == name) else {
                    return;
                };
                match decl {
                    TypeDecl::Newtype { inner, .. } => {
                        self.check_const(manifest, inner, value, ctx);
                    }
                    TypeDecl::Struct { fields, .. } => {
                        if let Some(object) = value.as_object() {
                            for field in fields {
                                if let Some(item) = object.get(&field.wire_name) {
                                    self.check_const(
                                        manifest,
                                        &field.ty,
                                        item,
                                        &format!("{ctx}.{}", field.wire_name),
                                    );
                                }
                            }
                        }
                    }
                    TypeDecl::Enum { tag, variants, .. } => {
                        if let Some(object) = value.as_object() {
                            let variant =
                                object.get(tag).and_then(Value::as_str).and_then(|wire| {
                                    variants.iter().find(|variant| variant.wire_name == wire)
                                });
                            if let Some(variant) = variant {
                                for field in &variant.fields {
                                    if let Some(item) = object.get(&field.wire_name) {
                                        self.check_const(
                                            manifest,
                                            &field.ty,
                                            item,
                                            &format!("{ctx}.{}", field.wire_name),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    TypeDecl::StringEnum { .. } | TypeDecl::ErrorEnum { .. } => {}
                }
            }
            _ => {}
        }
    }

    fn check_targets(&mut self, ctx: &str, targets: &[Target]) {
        if targets.is_empty() {
            self.errors.push(format!(
                "{ctx}: empty target list — it would appear in no projection"
            ));
        }
    }

    fn check_err(&mut self, ctx: &str, err: Option<&str>) {
        let Some(name) = err else { return };
        match self.kinds.get(name) {
            Some(Kind::ErrorEnum) => {}
            Some(_) => self
                .errors
                .push(format!("{ctx}: error type `{name}` is not an error enum")),
            None => self.errors.push(format!(
                "{ctx}: error type `{name}` is not declared in the manifest"
            )),
        }
    }

    fn check(&mut self, ty: &Ty, pos: Pos, ctx: &str) {
        match ty {
            Ty::Ref { name } => match self.kinds.get(name) {
                Some(Kind::ErrorEnum) => self.errors.push(format!(
                    "{ctx}: references error enum `{name}` as a data type — error enums only \
                     appear in `Result` error position"
                )),
                Some(_) => {}
                None => self
                    .errors
                    .push(format!("{ctx}: references undeclared type `{name}`")),
            },
            Ty::Slice { .. } => {
                if pos != Pos::ParamTop {
                    self.errors.push(format!(
                        "{ctx}: slices are only valid as top-level parameters — return an owned \
                         `Buf<T>` instead"
                    ));
                }
            }
            Ty::Unit => {
                if pos != Pos::ReturnTop {
                    self.errors
                        .push(format!("{ctx}: `()` is only valid as a return type"));
                }
            }
            Ty::Option { inner } | Ty::List { inner } => {
                self.check(inner, pos.nested(), ctx);
            }
            Ty::Map { value } => self.check(value, pos.nested(), ctx),
            Ty::Tuple { items } => {
                if !(2..=12).contains(&items.len()) {
                    self.errors.push(format!(
                        "{ctx}: tuples must contain between 2 and 12 items, found {}",
                        items.len()
                    ));
                }
                for item in items {
                    self.check(item, pos.nested(), ctx);
                }
            }
            // `Json` is schemaless passthrough: legal anywhere data is.
            Ty::Bool
            | Ty::U8
            | Ty::U16
            | Ty::U32
            | Ty::I8
            | Ty::I16
            | Ty::I32
            | Ty::I64
            | Ty::U64
            | Ty::F32
            | Ty::F64
            | Ty::String
            | Ty::Bytes
            | Ty::Null
            | Ty::Buf { .. }
            | Ty::Json => {}
        }
    }
}

fn alias_reaches<'a>(
    start: &str,
    current: &str,
    aliases: &BTreeMap<&'a str, &'a Ty>,
    path: &mut Vec<&'a str>,
) -> bool {
    let Some(ty) = aliases.get(current) else {
        return false;
    };
    let mut refs = Vec::new();
    collect_refs(ty, &mut refs);
    for next in refs {
        if next == start {
            return true;
        }
        if aliases.contains_key(next) && !path.contains(&next) {
            path.push(next);
            if alias_reaches(start, next, aliases, path) {
                return true;
            }
            path.pop();
        }
    }
    false
}

fn reference_cycle(
    start: &str,
    current: &str,
    graph: &BTreeMap<String, Vec<String>>,
    visiting: &mut BTreeSet<String>,
    path: &mut Vec<String>,
) -> bool {
    let Some(next_names) = graph.get(current) else {
        return false;
    };
    for next in next_names {
        if next == start {
            path.push(start.to_string());
            return true;
        }
        if !graph.contains_key(next) || !visiting.insert(next.clone()) {
            continue;
        }
        path.push(next.clone());
        if reference_cycle(start, next, graph, visiting, path) {
            return true;
        }
        path.pop();
        visiting.remove(next);
    }
    false
}

fn collect_refs<'a>(ty: &'a Ty, out: &mut Vec<&'a str>) {
    match ty {
        Ty::Ref { name } => out.push(name),
        Ty::Option { inner } | Ty::List { inner } => collect_refs(inner, out),
        Ty::Map { value } => collect_refs(value, out),
        Ty::Tuple { items } => {
            for item in items {
                collect_refs(item, out);
            }
        }
        _ => {}
    }
}

fn add_projected_name(
    names: &mut BTreeMap<String, Vec<String>>,
    name: impl Into<String>,
    owner: impl Into<String>,
) {
    names.entry(name.into()).or_default().push(owner.into());
}

fn report_projected_collisions(
    errors: &mut Vec<String>,
    target: &str,
    namespace: &str,
    names: BTreeMap<String, Vec<String>>,
) {
    for (name, owners) in names {
        if owners.len() > 1 {
            errors.push(format!(
                "{target} {namespace} `{name}` collides between {} — rename one of the declarations before generating bindings",
                owners.join(" and ")
            ));
        }
    }
}

fn attachment_types(manifest: &Manifest) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    loop {
        let before = found.len();
        for decl in &manifest.types {
            let contains = match decl {
                TypeDecl::Newtype { inner, .. } => ty_contains_attachment(inner, &found),
                TypeDecl::Struct { fields, .. } => fields
                    .iter()
                    .any(|field| ty_contains_attachment(&field.ty, &found)),
                TypeDecl::Enum { variants, .. } => variants.iter().any(|variant| {
                    variant
                        .fields
                        .iter()
                        .any(|field| ty_contains_attachment(&field.ty, &found))
                }),
                TypeDecl::StringEnum { .. } | TypeDecl::ErrorEnum { .. } => false,
            };
            if contains {
                found.insert(decl.name().to_string());
            }
        }
        if found.len() == before {
            return found;
        }
    }
}

fn ty_contains_attachment(ty: &Ty, attachment_types: &BTreeSet<String>) -> bool {
    match ty {
        Ty::Bytes | Ty::Buf { .. } => true,
        Ty::Option { inner } | Ty::List { inner } => {
            ty_contains_attachment(inner, attachment_types)
        }
        Ty::Map { value } => ty_contains_attachment(value, attachment_types),
        Ty::Tuple { items } => items
            .iter()
            .any(|item| ty_contains_attachment(item, attachment_types)),
        Ty::Ref { name } => attachment_types.contains(name),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emit::test_manifest::{exact_manifest, manifest};
    use rspyts_core::ir::{
        ConstDecl, Dtype, ErrorVariantDecl, FnDecl, MethodDecl, StaticDecl, VariantDecl,
    };

    fn base() -> Manifest {
        manifest()
    }

    fn method(name: &str, targets: Vec<Target>) -> MethodDecl {
        MethodDecl {
            name: name.to_string(),
            docs: String::new(),
            mutable: false,
            params: vec![],
            ret: Ty::Unit,
            err: None,
            targets,
        }
    }

    fn static_method(name: &str, targets: Vec<Target>) -> StaticDecl {
        StaticDecl {
            name: name.to_string(),
            docs: String::new(),
            params: vec![],
            ret: Ty::Unit,
            err: None,
            returns_self: false,
            targets,
        }
    }

    #[test]
    fn the_test_manifest_is_valid() {
        validate(&base()).expect("test manifest validates");
    }

    #[test]
    fn exact_manifest_abi_is_supported() {
        let manifest = base();
        assert_eq!(manifest.abi, rspyts_core::ABI_VERSION_STR);
        validate(&manifest).expect("current ABI manifest validates");
    }

    #[test]
    fn every_other_manifest_abi_is_rejected() {
        for abi in ["2.0", "2.1", "3.1", "4.0"] {
            let mut manifest = base();
            manifest.abi = abi.to_string();
            let message = validate(&manifest).unwrap_err().to_string();
            assert!(
                message.contains(&format!("unsupported manifest ABI `{abi}`")),
                "{message}"
            );
            assert!(message.contains("requires `3.0`"), "{message}");
        }
    }

    #[test]
    fn manifest_abi_parser_requires_numeric_major_dot_minor() {
        for abi in ["3", "3.", ".0", "3.0.1", "v3.0", "3.x", "3.-1"] {
            let message = validate_manifest_abi(abi).unwrap_err().to_string();
            assert!(message.contains("not in `major.minor` form"), "{message}");
        }
    }

    #[test]
    fn exact_scalars_tuples_and_mixed_variants_are_valid() {
        validate(&exact_manifest()).expect("exact type fixture validates");

        let mut m = base();
        m.functions[0].params[1].ty = Ty::Tuple {
            items: vec![
                Ty::I64,
                Ty::U64,
                Ty::U8,
                Ty::U16,
                Ty::U32,
                Ty::I8,
                Ty::I16,
                Ty::I32,
                Ty::F32,
                Ty::F64,
                Ty::String,
                Ty::Bool,
            ],
        };
        validate(&m).expect("arity-12 tuples validate");
    }

    #[test]
    fn tuple_arity_and_nested_positions_are_validated() {
        for count in [1, 13] {
            let mut m = base();
            m.functions[0].params[1].ty = Ty::Tuple {
                items: vec![Ty::U8; count],
            };
            let msg = validate(&m).unwrap_err().to_string();
            assert!(
                msg.contains(&format!(
                    "tuples must contain between 2 and 12 items, found {count}"
                )),
                "{msg}"
            );
        }

        let mut m = base();
        m.functions[0].params[1].ty = Ty::Tuple {
            items: vec![Ty::U8, Ty::Slice { dt: Dtype::U8 }],
        };
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("slices are only valid as top-level parameters"),
            "{msg}"
        );
    }

    #[test]
    fn buf_and_bytes_are_valid_in_owned_data_positions() {
        let mut m = base();
        m.functions[0].params[1].ty = Ty::Buf { dt: Dtype::F64 };
        m.functions[0].ret = Ty::Bytes;
        validate(&m).expect("owned attachments are legal in parameters and returns");
    }

    #[test]
    fn unresolved_ref_is_rejected() {
        let mut m = base();
        m.functions[0].params[2].ty = Ty::Ref {
            name: "Ghost".into(),
        };
        let msg = validate(&m).unwrap_err().to_string();
        assert!(msg.contains("undeclared type `Ghost`"), "{msg}");
    }

    #[test]
    fn error_enum_as_data_ref_is_rejected() {
        let mut m = base();
        m.functions[0].ret = Ty::Ref {
            name: "QueryError".into(),
        };
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("references error enum `QueryError` as a data type"),
            "{msg}"
        );
    }

    #[test]
    fn slice_in_return_and_fields_is_rejected() {
        let mut m = base();
        m.functions[0].ret = Ty::Slice { dt: Dtype::F64 };
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("slices are only valid as top-level parameters"),
            "{msg}"
        );

        let mut m = base();
        if let TypeDecl::Struct { fields, .. } = &mut m.types[1] {
            fields[0].ty = Ty::Slice { dt: Dtype::U8 };
        } else {
            panic!("types[1] should be the struct");
        }
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("struct `QueryOptions` field `minimum_value`"),
            "{msg}"
        );
    }

    #[test]
    fn nested_slice_in_param_is_rejected() {
        let mut m = base();
        m.functions[0].params[1].ty = Ty::Option {
            inner: Box::new(Ty::Slice { dt: Dtype::F64 }),
        };
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("slices are only valid as top-level parameters"),
            "{msg}"
        );
    }

    #[test]
    fn option_recurses_and_presence_is_explicit() {
        let mut m = base();
        m.functions[0].params[1].ty = Ty::Option {
            inner: Box::new(Ty::Slice { dt: Dtype::F64 }),
        };
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("slices are only valid as top-level parameters"),
            "{msg}"
        );

        let mut m = base();
        let TypeDecl::Struct { fields, .. } = &mut m.types[1] else {
            panic!("types[1] should be the struct");
        };
        fields[0].ty = Ty::Option {
            inner: Box::new(Ty::String),
        };
        fields[0].required = true;
        validate(&m).expect("a required Option field accepts null but not omission");

        let TypeDecl::Struct { fields, .. } = &mut m.types[1] else {
            panic!("types[1] should be the struct");
        };
        fields[0].ty = Ty::String;
        fields[0].required = false;
        let msg = validate(&m).unwrap_err().to_string();
        assert!(msg.contains("inconsistent required metadata"), "{msg}");
    }

    #[test]
    fn null_is_legal_in_data_fields() {
        let mut m = base();
        let TypeDecl::Struct { fields, .. } = &mut m.types[1] else {
            panic!("types[1] should be the struct");
        };
        fields[0].ty = Ty::Null;
        fields[0].required = true;
        let default = m
            .constants
            .iter_mut()
            .find(|constant| constant.name == "DEFAULT_OPTIONS")
            .unwrap();
        default.value["minimumValue"] = Value::Null;
        validate(&m).unwrap();
    }

    #[test]
    fn null_constants_reject_non_null_manifest_values() {
        let mut m = base();
        let index = m.constants.len();
        m.constants.push(rspyts_core::ir::ConstDecl {
            name: "NOT_NULL".to_string(),
            docs: String::new(),
            origin: "test".to_string(),
            ty: Ty::Null,
            value: Value::Bool(false),
        });
        let msg = validate(&m).unwrap_err().to_string();
        assert!(msg.contains("must contain the JSON null value"), "{msg}");

        m.constants[index].value = Value::Null;
        validate(&m).unwrap();
    }

    #[test]
    fn unknown_err_name_is_rejected() {
        let mut m = base();
        m.functions[0].err = Some("NoSuchError".into());
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("error type `NoSuchError` is not declared"),
            "{msg}"
        );

        let mut m = base();
        m.functions[0].err = Some("QueryOptions".into());
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("error type `QueryOptions` is not an error enum"),
            "{msg}"
        );
    }

    #[test]
    fn class_name_colliding_with_type_is_rejected() {
        let mut m = base();
        m.classes[0].name = "QueryOptions".into();
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("Python package root export `QueryOptions` collides between struct"),
            "{msg}"
        );
        assert!(
            msg.contains("TypeScript module root export `QueryOptions` collides between struct"),
            "{msg}"
        );
    }

    #[test]
    fn constant_name_colliding_with_function_is_rejected() {
        let mut m = base();
        m.constants[0].name = "process_values".into();
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("Python package root export `process_values` collides between constant"),
            "{msg}"
        );
        assert!(!msg.contains("TypeScript module root export `process_values`"));
    }

    #[test]
    fn generated_error_registry_names_are_target_aware_and_include_foreign_types() {
        let mut m = base();
        m.types.push(TypeDecl::ErrorEnum {
            name: "Query".into(),
            docs: String::new(),
            origin: "mapped-foreign-crate".into(),
            variants: vec![ErrorVariantDecl {
                name: "Other".into(),
                wire_code: "other".into(),
                docs: String::new(),
                fields: vec![],
            }],
        });
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("Python package root export `QUERY_ERROR_TYPES` collides"),
            "{msg}"
        );
        assert!(msg.contains("error enum `QueryError`"), "{msg}");
        assert!(msg.contains("error enum `Query`"), "{msg}");
        assert!(
            msg.contains("TypeScript module root export `QueryErrorTypes` collides"),
            "{msg}"
        );
    }

    #[test]
    fn generated_error_registries_collide_with_target_root_declarations() {
        let mut m = base();
        m.constants.push(ConstDecl {
            name: "QUERY_ERROR_TYPES".into(),
            docs: String::new(),
            origin: "test".into(),
            ty: Ty::String,
            value: Value::String(String::new()),
        });
        m.constants.push(ConstDecl {
            name: "QueryErrorTypes".into(),
            docs: String::new(),
            origin: "test".into(),
            ty: Ty::String,
            value: Value::String(String::new()),
        });
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("Python package root export `QUERY_ERROR_TYPES` collides"),
            "{msg}"
        );
        assert!(
            msg.contains("TypeScript module root export `QueryErrorTypes` collides"),
            "{msg}"
        );
        assert!(!msg.contains("Python package root export `QueryErrorTypes`"));
        assert!(!msg.contains("TypeScript module root export `QUERY_ERROR_TYPES`"));
    }

    #[test]
    fn synthesized_typescript_client_roots_collide_with_foreign_dtos() {
        let mut m = base();
        for name in ["DemoCrateClient", "createClient"] {
            m.types.push(TypeDecl::Struct {
                name: name.into(),
                docs: String::new(),
                origin: "mapped-foreign-crate".into(),
                fields: vec![],
            });
        }
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("TypeScript module root export `DemoCrateClient` collides"),
            "{msg}"
        );
        assert!(
            msg.contains("generated TypeScript client interface for crate `demo-crate`"),
            "{msg}"
        );
        assert!(
            msg.contains("TypeScript module root export `createClient` collides"),
            "{msg}"
        );
        assert!(msg.contains("mapped-foreign-crate"), "{msg}");
    }

    #[test]
    fn typescript_client_member_projection_collisions_are_rejected() {
        let mut m = base();
        m.functions.push(FnDecl {
            name: "render__summary".into(),
            docs: String::new(),
            params: vec![],
            ret: Ty::Unit,
            err: None,
            targets: vec![Target::Typescript],
        });
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("TypeScript client member `renderSummary` collides"),
            "{msg}"
        );
        assert!(msg.contains("function `render_summary`"), "{msg}");
        assert!(msg.contains("function `render__summary`"), "{msg}");
    }

    #[test]
    fn typescript_only_functions_do_not_collide_with_root_helpers() {
        let mut m = base();
        m.functions.push(FnDecl {
            name: "create_client".into(),
            docs: String::new(),
            params: vec![],
            ret: Ty::Unit,
            err: None,
            targets: vec![Target::Typescript],
        });
        validate(&m).expect("a client property and module-root function use separate namespaces");
    }

    #[test]
    fn class_runtime_members_and_host_projections_are_validated_per_target() {
        let mut m = base();
        let class = &mut m.classes[1];
        class.methods.push(method("close", vec![Target::Python]));
        class.methods.push(method("free", vec![Target::Typescript]));
        class
            .methods
            .push(method("constructor", vec![Target::Typescript]));
        class
            .methods
            .push(method("dispose", vec![Target::Typescript]));
        class
            .methods
            .push(method("read_value", vec![Target::Typescript]));
        class
            .methods
            .push(method("read__value", vec![Target::Typescript]));
        class
            .statics
            .push(static_method("handle", vec![Target::Python]));
        class
            .statics
            .push(static_method("prototype", vec![Target::Typescript]));

        let msg = validate(&m).unwrap_err().to_string();
        for expected in [
            "Python class `RunningStats` member `close` collides",
            "Python class `RunningStats` member `handle` collides",
            "TypeScript class `RunningStats` instance member `constructor` collides",
            "TypeScript class `RunningStats` instance member `dispose` collides",
            "TypeScript class `RunningStats` instance member `free` collides",
            "TypeScript class `RunningStats` instance member `readValue` collides",
            "TypeScript class `RunningStats` static member `prototype` collides",
        ] {
            assert!(msg.contains(expected), "missing `{expected}` in:\n{msg}");
        }
    }

    #[test]
    fn lifecycle_names_remain_legal_when_scoped_to_the_safe_host() {
        let mut m = base();
        let class = &mut m.classes[1];
        class
            .methods
            .push(method("close", vec![Target::Typescript]));
        class.methods.push(method("free", vec![Target::Python]));
        class
            .methods
            .push(method("constructor", vec![Target::Python]));
        class
            .methods
            .push(method("__init__", vec![Target::Typescript]));
        validate(&m).expect("target-scoped names that do not collide stay valid");
    }

    #[test]
    fn json_is_legal_in_every_data_position() {
        let mut m = base();
        m.functions[0].params[1].ty = Ty::Json;
        m.functions[0].ret = Ty::Json;
        m.constants[1].ty = Ty::Json;
        if let TypeDecl::Struct { fields, .. } = &mut m.types[1] {
            fields[0].ty = Ty::Json;
        } else {
            panic!("types[1] should be the struct");
        }
        validate(&m).expect("Json validates anywhere a data type is legal");
    }

    #[test]
    fn attachments_are_rejected_in_constants_and_error_data() {
        let mut m = base();
        m.constants[0].ty = Ty::Option {
            inner: Box::new(Ty::Bytes),
        };
        let variants = m
            .types
            .iter_mut()
            .find_map(|decl| match decl {
                TypeDecl::ErrorEnum { variants, .. } => Some(variants),
                _ => None,
            })
            .expect("manifest has an error enum");
        variants
            .iter_mut()
            .find(|variant| !variant.fields.is_empty())
            .expect("error enum has a data variant")
            .fields[0]
            .ty = Ty::Option {
            inner: Box::new(Ty::Buf { dt: Dtype::U8 }),
        };
        let msg = validate(&m).unwrap_err().to_string();
        assert!(msg.contains("constants have no attachment tail"), "{msg}");
        assert!(
            msg.contains("error envelopes cannot carry attachment tails"),
            "{msg}"
        );
    }

    #[test]
    fn duplicate_wire_names_are_rejected() {
        let mut m = base();
        if let TypeDecl::Struct { fields, .. } = &mut m.types[1] {
            fields[0].wire_name = "same".into();
            fields[1].wire_name = "same".into();
        } else {
            panic!("types[1] should be the struct");
        }
        m.functions[0].params[1].wire_name = "sameParam".into();
        m.functions[0].params[2].wire_name = "sameParam".into();
        let msg = validate(&m).unwrap_err().to_string();
        assert!(msg.contains("duplicate value `same`"), "{msg}");
        assert!(msg.contains("duplicate value `sameParam`"), "{msg}");
    }

    #[test]
    fn marker_shaped_wire_names_are_legal() {
        let mut m = base();
        if let TypeDecl::Struct { fields, .. } = &mut m.types[1] {
            fields[0].wire_name = "__proto__".into();
            fields[1].wire_name = "__rspyts_buf__".into();
            fields[2].wire_name = "__rspyts_json__".into();
        } else {
            panic!("types[1] should be the struct");
        }
        validate(&m).expect("schema-directed codecs make marker-shaped keys unambiguous");
    }

    #[test]
    fn enum_tags_and_projected_names_cannot_collide() {
        let mut m = base();
        let data_enum = m
            .types
            .iter_mut()
            .find(|decl| matches!(decl, TypeDecl::Enum { .. }))
            .expect("manifest has a data enum");
        if let TypeDecl::Enum {
            name,
            tag,
            variants,
            ..
        } = data_enum
        {
            *name = "Event".into();
            variants.push(VariantDecl {
                name: "Accepted".into(),
                wire_name: variants[0].wire_name.clone(),
                docs: String::new(),
                fields: variants[0].fields.clone(),
            });
            variants[0].fields[0].wire_name = tag.clone();
        } else {
            unreachable!();
        }
        m.types.push(TypeDecl::Struct {
            name: "EventAccepted".into(),
            docs: String::new(),
            origin: "test".into(),
            fields: vec![],
        });
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("variant wire names contain duplicate"),
            "{msg}"
        );
        assert!(msg.contains("uses discriminator key"), "{msg}");
        assert!(
            msg.contains("Python package root export `EventAccepted` collides"),
            "{msg}"
        );
        assert!(!msg.contains("TypeScript module root export `EventAccepted`"));
    }

    #[test]
    fn nonfinite_structured_float_constants_are_rejected() {
        let mut m = base();
        m.constants[0].ty = Ty::F64;
        m.constants[0].value = serde_json::Value::Null;
        let msg = validate(&m).unwrap_err().to_string();
        assert!(msg.contains("must contain a finite JSON number"), "{msg}");
    }

    #[test]
    fn statics_are_validated_like_methods() {
        let mut m = base();
        m.classes[0].statics[1].err = Some("Ghost".into());
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("class `Session` static `default_extension`"),
            "{msg}"
        );
        assert!(msg.contains("error type `Ghost` is not declared"), "{msg}");
    }

    #[test]
    fn factory_ret_is_ignored_but_non_factory_ret_is_checked() {
        // A factory's `ret` field is ignored (the envelope carries a
        // handle), so even a slice there does not trip validation...
        let mut m = base();
        m.classes[0].statics[0].ret = Ty::Slice { dt: Dtype::F64 };
        validate(&m).expect("factory ret is ignored");

        // ...while a non-factory static's return type is validated.
        let mut m = base();
        m.classes[0].statics[1].ret = Ty::Slice { dt: Dtype::F64 };
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("class `Session` static `default_extension` return type"),
            "{msg}"
        );
    }

    #[test]
    fn unconstructible_class_is_rejected() {
        let mut m = base();
        // Session is factory-only; demoting its factory leaves the
        // class with no way to be constructed.
        m.classes[0].statics[0].returns_self = false;
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("class `Session` has neither a constructor nor a factory"),
            "{msg}"
        );
    }

    #[test]
    fn empty_targets_are_rejected() {
        let mut m = base();
        m.functions[0].targets = vec![];
        m.classes[0].methods[0].targets = vec![];
        m.classes[0].statics[0].targets = vec![];
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("function `process_values`: empty target list"),
            "{msg}"
        );
        assert!(
            msg.contains("class `Session` method `progress`: empty target list"),
            "{msg}"
        );
        assert!(
            msg.contains("class `Session` static `open`: empty target list"),
            "{msg}"
        );
    }

    #[test]
    fn multiple_problems_are_all_reported() {
        let mut m = base();
        m.functions.push(FnDecl {
            name: "broken".into(),
            docs: String::new(),
            params: vec![],
            ret: Ty::Ref {
                name: "Ghost".into(),
            },
            err: Some("AlsoGhost".into()),
            targets: rspyts_core::ir::Target::all(),
        });
        let msg = validate(&m).unwrap_err().to_string();
        assert!(msg.contains("Ghost") && msg.contains("AlsoGhost"), "{msg}");
        assert!(msg.contains("2 problem(s)"), "{msg}");
    }

    #[test]
    fn recursive_newtype_aliases_are_rejected() {
        let mut m = base();
        m.types.push(TypeDecl::Newtype {
            name: "FirstId".into(),
            docs: String::new(),
            origin: "test".into(),
            inner: Ty::Ref {
                name: "SecondId".into(),
            },
        });
        m.types.push(TypeDecl::Newtype {
            name: "SecondId".into(),
            docs: String::new(),
            origin: "test".into(),
            inner: Ty::Option {
                inner: Box::new(Ty::Ref {
                    name: "FirstId".into(),
                }),
            },
        });
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("recursive through transparent aliases"),
            "{msg}"
        );
    }

    #[test]
    fn recursive_data_contracts_are_rejected() {
        let mut m = base();
        m.types.push(TypeDecl::Struct {
            name: "Node".into(),
            docs: String::new(),
            origin: "test".into(),
            fields: vec![FieldDecl {
                name: "children".into(),
                wire_name: "children".into(),
                docs: String::new(),
                ty: Ty::List {
                    inner: Box::new(Ty::Ref {
                        name: "Node".into(),
                    }),
                },
                required: true,
            }],
        });
        m.functions[0].ret = Ty::Ref {
            name: "Node".into(),
        };
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("recursive reference cycle `Node -> Node`"),
            "{msg}"
        );
    }
}
