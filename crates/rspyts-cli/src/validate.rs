//! Manifest validation.
//!
//! The `#[bridge]` macros reject these shapes at compile time, so a
//! well-formed module never trips validation — this is the CLI's
//! defense against hand-rolled shims, macro bugs, and version skew.
//! Every check reports the offending item by name; all problems are
//! collected before failing so one run shows everything.

use rspyts_core::ir::{FieldDecl, Manifest, ParamDecl, Target, Ty, TypeDecl};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// What kind of declaration a name refers to.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Struct,
    Enum,
    StringEnum,
    ErrorEnum,
}

impl Kind {
    fn describe(self) -> &'static str {
        match self {
            Kind::Struct => "struct",
            Kind::Enum => "enum",
            Kind::StringEnum => "string enum",
            Kind::ErrorEnum => "error enum",
        }
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
    let mut v = Validator {
        kinds: BTreeMap::new(),
        errors: Vec::new(),
    };
    for ty in &manifest.types {
        let kind = match ty {
            TypeDecl::Struct { .. } => Kind::Struct,
            TypeDecl::Enum { .. } => Kind::Enum,
            TypeDecl::StringEnum { .. } => Kind::StringEnum,
            TypeDecl::ErrorEnum { .. } => Kind::ErrorEnum,
        };
        v.kinds.insert(ty.name().to_string(), kind);
    }

    // Types, classes, functions, and constants all land in the one
    // generated module namespace, so their names must be unique.
    let mut namespace: BTreeMap<&str, Vec<&'static str>> = BTreeMap::new();
    for ty in &manifest.types {
        let kind = v.kinds[ty.name()];
        namespace
            .entry(ty.name())
            .or_default()
            .push(kind.describe());
    }
    for class in &manifest.classes {
        namespace.entry(&class.name).or_default().push("class");
    }
    for f in &manifest.functions {
        namespace.entry(&f.name).or_default().push("function");
    }
    for c in &manifest.constants {
        namespace.entry(&c.name).or_default().push("constant");
    }
    for (name, kinds) in &namespace {
        if kinds.len() > 1 {
            v.errors.push(format!(
                "`{name}` collides with itself: declared as {} — types, classes, functions, and \
                 constants share the generated module namespace",
                kinds.join(" and ")
            ));
        }
    }

    for ty in &manifest.types {
        match ty {
            TypeDecl::Struct { name, fields, .. } => {
                v.check_fields(&format!("struct `{name}`"), fields);
            }
            TypeDecl::Enum { name, variants, .. } => {
                for variant in variants {
                    v.check_fields(
                        &format!("enum `{name}` variant `{}`", variant.name),
                        &variant.fields,
                    );
                }
            }
            TypeDecl::StringEnum { .. } => {}
            TypeDecl::ErrorEnum { name, variants, .. } => {
                for variant in variants {
                    v.check_fields(
                        &format!("error enum `{name}` variant `{}`", variant.name),
                        &variant.fields,
                    );
                }
            }
        }
    }

    for c in &manifest.constants {
        v.check(&c.ty, Pos::Field, &format!("constant `{}`", c.name));
    }

    for f in &manifest.functions {
        let ctx = format!("function `{}`", f.name);
        v.check_params(&ctx, &f.params);
        v.check(&f.ret, Pos::ReturnTop, &format!("{ctx} return type"));
        v.check_err(&ctx, f.err.as_deref());
        v.check_targets(&ctx, &f.targets);
    }

    for class in &manifest.classes {
        if let Some(ctor) = &class.constructor {
            let ctx = format!("class `{}` constructor", class.name);
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
            v.check_params(&ctx, &m.params);
            v.check(&m.ret, Pos::ReturnTop, &format!("{ctx} return type"));
            v.check_err(&ctx, m.err.as_deref());
            v.check_targets(&ctx, &m.targets);
        }
        for s in &class.statics {
            let ctx = format!("class `{}` static `{}`", class.name, s.name);
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
    errors: Vec<String>,
}

impl Validator {
    fn check_params(&mut self, ctx: &str, params: &[ParamDecl]) {
        for p in params {
            self.check(
                &p.ty,
                Pos::ParamTop,
                &format!("{ctx} parameter `{}`", p.name),
            );
        }
    }

    fn check_fields(&mut self, ctx: &str, fields: &[FieldDecl]) {
        for f in fields {
            self.check(&f.ty, Pos::Field, &format!("{ctx} field `{}`", f.name));
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
            Ty::Buf { .. } => {
                if matches!(pos, Pos::ParamTop | Pos::ParamNested) {
                    self.errors.push(format!(
                        "{ctx}: `Buf` is return-only — accept a `&[T]` slice parameter instead"
                    ));
                }
            }
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
            // `Json` is schemaless passthrough: legal anywhere data is.
            Ty::Bool
            | Ty::U8
            | Ty::U16
            | Ty::U32
            | Ty::I8
            | Ty::I16
            | Ty::I32
            | Ty::F32
            | Ty::F64
            | Ty::String
            | Ty::Json => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emit::test_manifest::manifest;
    use rspyts_core::ir::{Dtype, FnDecl};

    fn base() -> Manifest {
        manifest()
    }

    #[test]
    fn the_test_manifest_is_valid() {
        validate(&base()).expect("test manifest validates");
    }

    #[test]
    fn buf_in_param_is_rejected() {
        let mut m = base();
        m.functions[0].params[1].ty = Ty::Buf { dt: Dtype::F64 };
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("function `analyze_signal` parameter `sample_rate`"),
            "{msg}"
        );
        assert!(msg.contains("`Buf` is return-only"), "{msg}");
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
            name: "AnalysisError".into(),
        };
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("references error enum `AnalysisError` as a data type"),
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
            msg.contains("struct `AnalysisParams` field `min_duration_s`"),
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
    fn unknown_err_name_is_rejected() {
        let mut m = base();
        m.functions[0].err = Some("NoSuchError".into());
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("error type `NoSuchError` is not declared"),
            "{msg}"
        );

        let mut m = base();
        m.functions[0].err = Some("AnalysisParams".into());
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("error type `AnalysisParams` is not an error enum"),
            "{msg}"
        );
    }

    #[test]
    fn class_name_colliding_with_type_is_rejected() {
        let mut m = base();
        m.classes[0].name = "AnalysisParams".into();
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("`AnalysisParams` collides with itself: declared as struct and class"),
            "{msg}"
        );
    }

    #[test]
    fn constant_name_colliding_with_function_is_rejected() {
        let mut m = base();
        m.constants[0].name = "analyze_signal".into();
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains(
                "`analyze_signal` collides with itself: declared as function and constant"
            ),
            "{msg}"
        );
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
    fn statics_are_validated_like_methods() {
        let mut m = base();
        m.classes[0].statics[0].params[0].ty = Ty::Buf { dt: Dtype::F64 };
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("class `Recording` static `open` parameter `path`"),
            "{msg}"
        );
        assert!(msg.contains("`Buf` is return-only"), "{msg}");

        let mut m = base();
        m.classes[0].statics[1].err = Some("Ghost".into());
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("class `Recording` static `default_extension`"),
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
            msg.contains("class `Recording` static `default_extension` return type"),
            "{msg}"
        );
    }

    #[test]
    fn unconstructible_class_is_rejected() {
        let mut m = base();
        // Recording is factory-only; demoting its factory leaves the
        // class with no way to be constructed.
        m.classes[0].statics[0].returns_self = false;
        let msg = validate(&m).unwrap_err().to_string();
        assert!(
            msg.contains("class `Recording` has neither a constructor nor a factory"),
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
            msg.contains("function `analyze_signal`: empty target list"),
            "{msg}"
        );
        assert!(
            msg.contains("class `Recording` method `duration_s`: empty target list"),
            "{msg}"
        );
        assert!(
            msg.contains("class `Recording` static `open`: empty target list"),
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
}
