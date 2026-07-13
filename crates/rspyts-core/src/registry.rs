//! `inventory`-based registration of bridged items and deterministic
//! manifest assembly.
//!
//! Each `#[bridge]` expansion submits a small registration record whose
//! builder function produces the corresponding IR declaration. The
//! `rspyts::export!()`-generated `rspyts_manifest()` export calls
//! [`build_manifest`] to assemble and sort everything.

use crate::ir::{ClassDecl, ConstDecl, FnDecl, Manifest, TypeDecl};

/// Registration record for a `#[bridge]` data or error type.
pub struct RegisteredType {
    pub build: fn() -> TypeDecl,
}
inventory::collect!(RegisteredType);

/// Registration record for a `#[bridge]` constant.
pub struct RegisteredConst {
    pub build: fn() -> ConstDecl,
}
inventory::collect!(RegisteredConst);

/// Registration record for a `#[bridge]` free function.
pub struct RegisteredFn {
    pub build: fn() -> FnDecl,
}
inventory::collect!(RegisteredFn);

/// Registration record for a `#[bridge] impl` class.
pub struct RegisteredClass {
    pub build: fn() -> ClassDecl,
}
inventory::collect!(RegisteredClass);

/// Assemble the manifest for the current module.
///
/// Deterministic: each section is sorted lexicographically by name.
/// Duplicate names within a section are a programming error (two types
/// with the same name in one bridged crate) and panic with a clear
/// message — this surfaces at `rspyts generate` time, never in production
/// callers.
pub fn build_manifest(crate_name: &str, crate_version: &str) -> Manifest {
    let mut types: Vec<TypeDecl> = inventory::iter::<RegisteredType>()
        .map(|r| (r.build)())
        .collect();
    let mut functions: Vec<FnDecl> = inventory::iter::<RegisteredFn>()
        .map(|r| (r.build)())
        .collect();
    let mut classes: Vec<ClassDecl> = inventory::iter::<RegisteredClass>()
        .map(|r| (r.build)())
        .collect();
    let mut constants: Vec<ConstDecl> = inventory::iter::<RegisteredConst>()
        .map(|r| (r.build)())
        .collect();

    types.sort_by(|a, b| a.name().cmp(b.name()));
    functions.sort_by(|a, b| a.name.cmp(&b.name));
    classes.sort_by(|a, b| a.name.cmp(&b.name));
    constants.sort_by(|a, b| a.name.cmp(&b.name));

    assert_unique("type", types.iter().map(|t| t.name()));
    assert_unique("function", functions.iter().map(|f| f.name.as_str()));
    assert_unique("class", classes.iter().map(|c| c.name.as_str()));
    assert_unique("constant", constants.iter().map(|c| c.name.as_str()));

    Manifest {
        abi: crate::ABI_VERSION_STR.to_string(),
        crate_name: crate_name.to_string(),
        crate_version: crate_version.to_string(),
        types,
        constants,
        functions,
        classes,
    }
}

fn assert_unique<'a>(what: &str, sorted_names: impl Iterator<Item = &'a str>) {
    let mut prev: Option<&str> = None;
    for name in sorted_names {
        if prev == Some(name) {
            panic!(
                "rspyts: duplicate bridged {what} name `{name}` — names must be unique per crate"
            );
        }
        prev = Some(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{CtorDecl, Target, Ty};

    // The registrations below are global to the test binary, so
    // `build_manifest` sees exactly this fixed, deliberately unsorted set
    // (no other module in this crate submits to inventory). A duplicate
    // registration cannot be tested through inventory without breaking
    // every other manifest test in the binary, so uniqueness is asserted
    // directly against `assert_unique` instead.

    fn ty_decl(name: &str) -> TypeDecl {
        TypeDecl::Struct {
            name: name.to_string(),
            docs: String::new(),
            origin: "test-crate".to_string(),
            fields: Vec::new(),
        }
    }

    fn fn_decl(name: &str) -> FnDecl {
        FnDecl {
            name: name.to_string(),
            docs: String::new(),
            params: Vec::new(),
            ret: Ty::Unit,
            err: None,
            targets: Target::all(),
        }
    }

    fn class_decl(name: &str) -> ClassDecl {
        ClassDecl {
            name: name.to_string(),
            docs: String::new(),
            constructor: Some(CtorDecl {
                docs: String::new(),
                params: Vec::new(),
                err: None,
            }),
            methods: Vec::new(),
            statics: Vec::new(),
        }
    }

    fn ty_zebra() -> TypeDecl {
        ty_decl("ZebraConfig")
    }
    fn ty_apple() -> TypeDecl {
        ty_decl("AppleConfig")
    }
    fn fn_zulu() -> FnDecl {
        fn_decl("zulu_op")
    }
    fn fn_alpha() -> FnDecl {
        fn_decl("alpha_op")
    }
    fn class_yak() -> ClassDecl {
        class_decl("Yak")
    }
    fn class_bee() -> ClassDecl {
        class_decl("Bee")
    }

    inventory::submit! { RegisteredType { build: ty_zebra } }
    inventory::submit! { RegisteredType { build: ty_apple } }
    inventory::submit! { RegisteredFn { build: fn_zulu } }
    inventory::submit! { RegisteredFn { build: fn_alpha } }
    inventory::submit! { RegisteredClass { build: class_yak } }
    inventory::submit! { RegisteredClass { build: class_bee } }

    #[test]
    fn build_manifest_sorts_every_section_by_name() {
        let manifest = build_manifest("demo-crate", "1.2.3");
        assert_eq!(manifest.abi, crate::ABI_VERSION_STR);
        assert_eq!(manifest.crate_name, "demo-crate");
        assert_eq!(manifest.crate_version, "1.2.3");

        let type_names: Vec<&str> = manifest.types.iter().map(|t| t.name()).collect();
        assert_eq!(type_names, ["AppleConfig", "ZebraConfig"]);
        let fn_names: Vec<&str> = manifest.functions.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(fn_names, ["alpha_op", "zulu_op"]);
        let class_names: Vec<&str> = manifest.classes.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(class_names, ["Bee", "Yak"]);
    }

    #[test]
    fn build_manifest_is_deterministic() {
        assert_eq!(
            build_manifest("demo-crate", "1.2.3"),
            build_manifest("demo-crate", "1.2.3"),
        );
    }

    #[test]
    fn assert_unique_accepts_unique_sorted_names() {
        assert_unique("type", ["alpha", "beta", "gamma"].into_iter());
        assert_unique("type", std::iter::empty());
    }

    #[test]
    #[should_panic(expected = "duplicate bridged function name `dup`")]
    fn assert_unique_panics_on_adjacent_duplicates() {
        assert_unique("function", ["alpha", "dup", "dup", "omega"].into_iter());
    }
}
