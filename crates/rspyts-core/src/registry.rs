//! `inventory`-based registration and deterministic manifest assembly.
//!
//! Rust's `inventory` is process-wide: linking crate B also links every
//! registration submitted by dependency crate A. Registrations therefore
//! carry their defining Cargo package. [`build_manifest`] keeps B's own
//! public declarations as roots and follows only the foreign data/error
//! types that those roots reference. Dependency functions, classes,
//! constants, and unrelated types never leak into B's contract.

use crate::ir::{ClassDecl, ConstDecl, FieldDecl, FnDecl, Manifest, ParamDecl, Ty, TypeDecl};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

type TypeId = (String, String);

/// Registration record for a `#[bridge]` data or error type.
pub struct RegisteredType {
    /// Cargo package that contains the macro expansion.
    pub origin: &'static str,
    pub build: fn() -> TypeDecl,
}
inventory::collect!(RegisteredType);

/// Registration record for a `#[bridge]` constant.
pub struct RegisteredConst {
    /// Cargo package that contains the macro expansion.
    pub origin: &'static str,
    pub build: fn() -> ConstDecl,
}
inventory::collect!(RegisteredConst);

/// Registration record for a `#[bridge]` free function.
pub struct RegisteredFn {
    /// Cargo package that contains the macro expansion.
    pub origin: &'static str,
    pub build: fn() -> FnDecl,
}
inventory::collect!(RegisteredFn);

/// Registration record for a `#[bridge] impl` class.
pub struct RegisteredClass {
    /// Cargo package that contains the macro expansion.
    pub origin: &'static str,
    pub build: fn() -> ClassDecl,
}
inventory::collect!(RegisteredClass);

/// Assemble the manifest exported by `crate_name`.
///
/// The output is deterministic and contains:
///
/// - every type, function, class, and constant declared by `crate_name`;
/// - foreign types reached transitively from those local declarations;
/// - no other declarations from linked dependency inventories.
///
/// Invalid inventory graphs panic with a deterministic diagnostic. This is
/// the same generate-time failure boundary used for duplicate declarations.
pub fn build_manifest(crate_name: &str, crate_version: &str) -> Manifest {
    let type_index = type_registry();

    // Filter on registration metadata before calling the builders. Besides
    // being cheaper, this guarantees a dependency constant is never
    // evaluated merely because the dependency was linked.
    let mut functions: Vec<FnDecl> = inventory::iter::<RegisteredFn>()
        .filter(|registration| registration.origin == crate_name)
        .map(|registration| (registration.build)())
        .collect();
    let mut classes: Vec<ClassDecl> = inventory::iter::<RegisteredClass>()
        .filter(|registration| registration.origin == crate_name)
        .map(|registration| (registration.build)())
        .collect();
    let mut constants: Vec<ConstDecl> = inventory::iter::<RegisteredConst>()
        .filter(|registration| registration.origin == crate_name)
        .map(|registration| {
            let declaration = (registration.build)();
            assert_origin(
                "constant",
                registration.origin,
                &declaration.origin,
                &declaration.name,
            );
            declaration
        })
        .collect();

    functions.sort_by(|a, b| a.name.cmp(&b.name));
    classes.sort_by(|a, b| a.name.cmp(&b.name));
    constants.sort_by(|a, b| a.name.cmp(&b.name));

    assert_unique(
        "function",
        functions
            .iter()
            .map(|declaration| declaration.name.as_str()),
    );
    assert_unique(
        "class",
        classes.iter().map(|declaration| declaration.name.as_str()),
    );
    assert_unique(
        "constant",
        constants
            .iter()
            .map(|declaration| declaration.name.as_str()),
    );

    let mut pending: BTreeSet<TypeId> = type_index
        .declarations
        .keys()
        .filter(|(origin, _)| origin == crate_name)
        .cloned()
        .collect();

    for function in &mut functions {
        let context = format!("function `{}`", function.name);
        normalize_params(&mut function.params, &context, type_index, &mut pending);
        normalize_ty(
            &mut function.ret,
            &format!("{context} return"),
            type_index,
            &mut pending,
        );
        normalize_error(&mut function.err, &context, type_index, &mut pending);
    }
    for class in &mut classes {
        normalize_class(class, type_index, &mut pending);
    }
    for constant in &mut constants {
        normalize_ty(
            &mut constant.ty,
            &format!("constant `{}`", constant.name),
            type_index,
            &mut pending,
        );
    }

    let mut included = BTreeSet::new();
    let mut types = Vec::new();
    while let Some(type_id) = pending.pop_first() {
        if !included.insert(type_id.clone()) {
            continue;
        }
        let mut declaration = type_index
            .declarations
            .get(&type_id)
            .expect("rspyts: queued type disappeared from the inventory index")
            .clone();
        normalize_type_decl(&mut declaration, type_index, &mut pending);
        types.push(declaration);
    }

    // Emitters identify declarations by their public name. Two reachable
    // origins with the same name would therefore be ambiguous even though
    // inventory resolution itself is qualified.
    types.sort_by(|a, b| {
        a.name()
            .cmp(b.name())
            .then_with(|| a.origin().cmp(b.origin()))
    });
    assert_unique_type_names(&types);

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

/// Types-only inventory view shared by manifest construction and native
/// schema-directed codecs. It never evaluates constants or calls
/// [`build_manifest`], so it is safe during constant serialization.
pub(crate) struct TypeRegistry {
    declarations: BTreeMap<TypeId, TypeDecl>,
    by_name: BTreeMap<String, Vec<TypeId>>,
}

static TYPE_REGISTRY: OnceLock<TypeRegistry> = OnceLock::new();

pub(crate) fn type_registry() -> &'static TypeRegistry {
    TYPE_REGISTRY.get_or_init(TypeRegistry::from_inventory)
}

impl TypeRegistry {
    fn from_inventory() -> Self {
        let mut declarations = BTreeMap::new();
        for registration in inventory::iter::<RegisteredType>() {
            let declaration = (registration.build)();
            assert_origin(
                "type",
                registration.origin,
                declaration.origin(),
                declaration.name(),
            );
            let type_id = (
                registration.origin.to_string(),
                declaration.name().to_string(),
            );
            if declarations.insert(type_id.clone(), declaration).is_some() {
                panic!(
                    "rspyts: duplicate bridged type `{}` from origin `{}`",
                    type_id.1, type_id.0
                );
            }
        }

        let mut by_name = BTreeMap::<String, Vec<TypeId>>::new();
        for type_id in declarations.keys() {
            by_name
                .entry(type_id.1.clone())
                .or_default()
                .push(type_id.clone());
        }
        Self {
            declarations,
            by_name,
        }
    }

    fn resolve(&self, reference: &str, context: &str) -> TypeId {
        if let Some((origin, name)) = Ty::split_qualified_ref(reference) {
            let type_id = (origin.to_string(), name.to_string());
            if self.declarations.contains_key(&type_id) {
                return type_id;
            }
            panic!("rspyts: {context} references unresolved type `{origin}::{name}`");
        }

        let Some(candidates) = self.by_name.get(reference) else {
            panic!("rspyts: {context} references unresolved type `{reference}`");
        };
        if candidates.len() == 1 {
            return candidates[0].clone();
        }
        let origins = candidates
            .iter()
            .map(|(origin, _)| origin.as_str())
            .collect::<Vec<_>>()
            .join("`, `");
        panic!(
            "rspyts: {context} has ambiguous unqualified type reference `{reference}`; \
             candidates are from origins `{origins}`"
        );
    }

    /// Resolve a raw inventory-time `Ty::Ref` name to its declaration.
    ///
    /// Qualified references use exact `(origin, name)` identity. Legacy
    /// unqualified references are accepted only when the name is globally
    /// unique across the linked type inventory.
    pub(crate) fn declaration_for_ref(&self, reference: &str, context: &str) -> &TypeDecl {
        let type_id = self.resolve(reference, context);
        self.declarations
            .get(&type_id)
            .expect("rspyts: resolved type disappeared from the inventory registry")
    }
}

fn assert_origin(what: &str, registered: &str, declared: &str, name: &str) {
    assert!(
        registered == declared,
        "rspyts: bridged {what} `{name}` registered from origin `{registered}` but declared origin `{declared}`"
    );
}

fn normalize_type_decl(
    declaration: &mut TypeDecl,
    index: &TypeRegistry,
    pending: &mut BTreeSet<TypeId>,
) {
    let context = format!(
        "type `{}` from origin `{}`",
        declaration.name(),
        declaration.origin()
    );
    match declaration {
        TypeDecl::Newtype { inner, .. } => normalize_ty(inner, &context, index, pending),
        TypeDecl::Struct { fields, .. } => normalize_fields(fields, &context, index, pending),
        TypeDecl::Enum { variants, .. } => {
            for variant in variants {
                normalize_fields(
                    &mut variant.fields,
                    &format!("{context} variant `{}`", variant.name),
                    index,
                    pending,
                );
            }
        }
        TypeDecl::ErrorEnum { variants, .. } => {
            for variant in variants {
                normalize_fields(
                    &mut variant.fields,
                    &format!("{context} variant `{}`", variant.name),
                    index,
                    pending,
                );
            }
        }
        TypeDecl::StringEnum { .. } => {}
    }
}

fn normalize_class(class: &mut ClassDecl, index: &TypeRegistry, pending: &mut BTreeSet<TypeId>) {
    let context = format!("class `{}`", class.name);
    if let Some(constructor) = &mut class.constructor {
        normalize_params(
            &mut constructor.params,
            &format!("{context} constructor"),
            index,
            pending,
        );
        normalize_error(
            &mut constructor.err,
            &format!("{context} constructor"),
            index,
            pending,
        );
    }
    for method in &mut class.methods {
        let member_context = format!("{context} method `{}`", method.name);
        normalize_params(&mut method.params, &member_context, index, pending);
        normalize_ty(
            &mut method.ret,
            &format!("{member_context} return"),
            index,
            pending,
        );
        normalize_error(&mut method.err, &member_context, index, pending);
    }
    for method in &mut class.statics {
        let member_context = format!("{context} static `{}`", method.name);
        normalize_params(&mut method.params, &member_context, index, pending);
        normalize_ty(
            &mut method.ret,
            &format!("{member_context} return"),
            index,
            pending,
        );
        normalize_error(&mut method.err, &member_context, index, pending);
    }
}

fn normalize_fields(
    fields: &mut [FieldDecl],
    context: &str,
    index: &TypeRegistry,
    pending: &mut BTreeSet<TypeId>,
) {
    for field in fields {
        normalize_ty(
            &mut field.ty,
            &format!("{context} field `{}`", field.name),
            index,
            pending,
        );
    }
}

fn normalize_params(
    params: &mut [ParamDecl],
    context: &str,
    index: &TypeRegistry,
    pending: &mut BTreeSet<TypeId>,
) {
    for parameter in params {
        normalize_ty(
            &mut parameter.ty,
            &format!("{context} parameter `{}`", parameter.name),
            index,
            pending,
        );
    }
}

fn normalize_ty(ty: &mut Ty, context: &str, index: &TypeRegistry, pending: &mut BTreeSet<TypeId>) {
    match ty {
        Ty::Option { inner } | Ty::List { inner } => {
            normalize_ty(inner, context, index, pending);
        }
        Ty::Map { value } => normalize_ty(value, context, index, pending),
        Ty::Tuple { items } => {
            for item in items {
                normalize_ty(item, context, index, pending);
            }
        }
        Ty::Ref { name } => {
            let type_id = index.resolve(name, context);
            *name = type_id.1.clone();
            pending.insert(type_id);
        }
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
        | Ty::Unit
        | Ty::Null
        | Ty::Json
        | Ty::Buf { .. }
        | Ty::Slice { .. } => {}
    }
}

fn normalize_error(
    error: &mut Option<String>,
    context: &str,
    index: &TypeRegistry,
    pending: &mut BTreeSet<TypeId>,
) {
    let Some(name) = error else { return };
    let error_context = format!("{context} error");
    let declaration = index.declaration_for_ref(name, &error_context);
    let type_id = index.resolve(name, &error_context);
    assert!(
        matches!(declaration, TypeDecl::ErrorEnum { .. }),
        "rspyts: {context} error type `{}::{}` is not an error enum",
        type_id.0,
        type_id.1
    );
    *name = type_id.1.clone();
    pending.insert(type_id);
}

fn assert_unique_type_names(types: &[TypeDecl]) {
    for pair in types.windows(2) {
        if pair[0].name() == pair[1].name() {
            panic!(
                "rspyts: reachable type name `{}` is declared by both `{}` and `{}`; \
                 public manifest type names must be unique",
                pair[0].name(),
                pair[0].origin(),
                pair[1].origin(),
            );
        }
    }
}

fn assert_unique<'a>(what: &str, sorted_names: impl Iterator<Item = &'a str>) {
    let mut previous = None;
    for name in sorted_names {
        if previous == Some(name) {
            panic!(
                "rspyts: duplicate bridged {what} name `{name}` — names must be unique per crate"
            );
        }
        previous = Some(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{CtorDecl, Target};

    const LOCAL: &str = "demo-crate";
    const FOREIGN: &str = "hardware-crate";

    fn struct_decl(origin: &str, name: &str, fields: Vec<FieldDecl>) -> TypeDecl {
        TypeDecl::Struct {
            name: name.to_string(),
            docs: String::new(),
            origin: origin.to_string(),
            fields,
        }
    }

    fn field(name: &str, ty: Ty) -> FieldDecl {
        FieldDecl {
            name: name.to_string(),
            wire_name: name.to_string(),
            docs: String::new(),
            ty,
            required: true,
        }
    }

    fn fn_decl(name: &str, ret: Ty) -> FnDecl {
        FnDecl {
            name: name.to_string(),
            docs: String::new(),
            params: Vec::new(),
            ret,
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

    fn local_zebra() -> TypeDecl {
        struct_decl(LOCAL, "ZebraConfig", Vec::new())
    }
    fn local_apple() -> TypeDecl {
        struct_decl(LOCAL, "AppleConfig", Vec::new())
    }
    fn foreign_used() -> TypeDecl {
        struct_decl(
            FOREIGN,
            "RecordedSignal",
            vec![field(
                "metadata",
                Ty::qualified_ref(FOREIGN, "SignalMetadata"),
            )],
        )
    }
    fn foreign_nested() -> TypeDecl {
        struct_decl(FOREIGN, "SignalMetadata", Vec::new())
    }
    fn foreign_unused() -> TypeDecl {
        struct_decl(FOREIGN, "UnrelatedDeviceCatalog", Vec::new())
    }
    fn local_zulu_fn() -> FnDecl {
        fn_decl("zulu_op", Ty::Unit)
    }
    fn local_alpha_fn() -> FnDecl {
        let mut declaration = fn_decl("alpha_op", Ty::qualified_ref(FOREIGN, "RecordedSignal"));
        declaration.targets = vec![Target::Python];
        declaration
    }
    fn foreign_fn() -> FnDecl {
        panic!("a foreign function builder must not run")
    }
    fn local_yak_class() -> ClassDecl {
        class_decl("Yak")
    }
    fn local_bee_class() -> ClassDecl {
        class_decl("Bee")
    }
    fn foreign_class() -> ClassDecl {
        panic!("a foreign class builder must not run")
    }
    fn local_const() -> ConstDecl {
        ConstDecl {
            name: "LOCAL_LIMIT".to_string(),
            docs: String::new(),
            origin: LOCAL.to_string(),
            ty: Ty::U32,
            value: serde_json::json!(7),
        }
    }
    fn foreign_const() -> ConstDecl {
        panic!("a foreign constant builder must not run")
    }

    inventory::submit! { RegisteredType { origin: LOCAL, build: local_zebra } }
    inventory::submit! { RegisteredType { origin: LOCAL, build: local_apple } }
    inventory::submit! { RegisteredType { origin: FOREIGN, build: foreign_used } }
    inventory::submit! { RegisteredType { origin: FOREIGN, build: foreign_nested } }
    inventory::submit! { RegisteredType { origin: FOREIGN, build: foreign_unused } }
    inventory::submit! { RegisteredFn { origin: LOCAL, build: local_zulu_fn } }
    inventory::submit! { RegisteredFn { origin: LOCAL, build: local_alpha_fn } }
    inventory::submit! { RegisteredFn { origin: FOREIGN, build: foreign_fn } }
    inventory::submit! { RegisteredClass { origin: LOCAL, build: local_yak_class } }
    inventory::submit! { RegisteredClass { origin: LOCAL, build: local_bee_class } }
    inventory::submit! { RegisteredClass { origin: FOREIGN, build: foreign_class } }
    inventory::submit! { RegisteredConst { origin: LOCAL, build: local_const } }
    inventory::submit! { RegisteredConst { origin: FOREIGN, build: foreign_const } }

    #[test]
    fn manifest_is_local_roots_plus_reachable_foreign_types() {
        let manifest = build_manifest(LOCAL, "1.2.3");
        assert_eq!(manifest.abi, crate::ABI_VERSION_STR);
        assert_eq!(manifest.crate_name, LOCAL);
        assert_eq!(manifest.crate_version, "1.2.3");

        let type_names = manifest
            .types
            .iter()
            .map(TypeDecl::name)
            .collect::<Vec<_>>();
        assert_eq!(
            type_names,
            [
                "AppleConfig",
                "RecordedSignal",
                "SignalMetadata",
                "ZebraConfig"
            ]
        );
        assert!(!type_names.contains(&"UnrelatedDeviceCatalog"));

        assert_eq!(
            manifest
                .functions
                .iter()
                .map(|declaration| declaration.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha_op", "zulu_op"]
        );
        assert_eq!(manifest.functions[0].targets, [Target::Python]);
        assert_eq!(
            manifest
                .classes
                .iter()
                .map(|declaration| declaration.name.as_str())
                .collect::<Vec<_>>(),
            ["Bee", "Yak"]
        );
        assert_eq!(manifest.constants.len(), 1);
        assert_eq!(manifest.constants[0].name, "LOCAL_LIMIT");

        // Qualified inventory identities are an implementation detail and
        // must never cross the public manifest boundary.
        assert_eq!(
            manifest.functions[0].ret,
            Ty::Ref {
                name: "RecordedSignal".to_string()
            }
        );
        let recorded = manifest
            .types
            .iter()
            .find(|declaration| declaration.name() == "RecordedSignal")
            .unwrap();
        let TypeDecl::Struct { fields, .. } = recorded else {
            panic!("expected a struct")
        };
        assert_eq!(
            fields[0].ty,
            Ty::Ref {
                name: "SignalMetadata".to_string()
            }
        );
    }

    #[test]
    fn build_manifest_is_deterministic() {
        assert_eq!(
            build_manifest(LOCAL, "1.2.3"),
            build_manifest(LOCAL, "1.2.3"),
        );
    }

    #[test]
    fn qualified_resolution_rejects_unknown_origin() {
        let index = TypeRegistry::from_inventory();
        let panic = std::panic::catch_unwind(|| {
            index.resolve(
                &match Ty::qualified_ref("missing-crate", "RecordedSignal") {
                    Ty::Ref { name } => name,
                    _ => unreachable!(),
                },
                "test declaration",
            )
        })
        .unwrap_err();
        let message = panic_message(panic);
        assert!(message.contains(
            "test declaration references unresolved type `missing-crate::RecordedSignal`"
        ));
    }

    #[test]
    fn unqualified_resolution_rejects_ambiguous_names() {
        let local = struct_decl("one", "Shared", Vec::new());
        let foreign = struct_decl("two", "Shared", Vec::new());
        let index = TypeRegistry {
            declarations: BTreeMap::from([
                (("one".to_string(), "Shared".to_string()), local),
                (("two".to_string(), "Shared".to_string()), foreign),
            ]),
            by_name: BTreeMap::from([(
                "Shared".to_string(),
                vec![
                    ("one".to_string(), "Shared".to_string()),
                    ("two".to_string(), "Shared".to_string()),
                ],
            )]),
        };
        let panic =
            std::panic::catch_unwind(|| index.resolve("Shared", "test declaration")).unwrap_err();
        assert!(panic_message(panic).contains(
            "ambiguous unqualified type reference `Shared`; candidates are from origins `one`, `two`"
        ));
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

    fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| {
                payload
                    .downcast_ref::<&str>()
                    .map(|value| (*value).to_string())
            })
            .unwrap_or_default()
    }
}
