#![cfg(not(target_arch = "wasm32"))]

mod fixtures;

use std::ffi::CStr;

use rspyts::ir::{BufferElement, FunctionDef, Manifest, ScalarValue, TypeDef, TypeRef, TypeShape};
use rspyts::runtime::ContractError;

fn discovered_manifest() -> Manifest {
    let result = fixtures::rspyts_contract();
    assert_eq!(result.status, rspyts::__private::DISCOVERY_SUCCESS);
    assert!(!result.payload.is_null());

    // SAFETY: discovery returns a live NUL-terminated allocation. This test
    // releases it exactly once with the paired free function.
    let payload = unsafe { CStr::from_ptr(result.payload) }
        .to_str()
        .expect("the discovery payload must be UTF-8")
        .to_owned();
    // SAFETY: `result.payload` came from `rspyts_contract` above.
    unsafe { fixtures::rspyts_contract_free(result.payload) };

    serde_json::from_str(&payload).expect("the discovery payload must be a manifest")
}

fn type_named<'a>(manifest: &'a Manifest, name: &str) -> &'a TypeDef {
    manifest
        .types
        .iter()
        .find(|item| item.name == name)
        .unwrap_or_else(|| panic!("missing type {name}"))
}

fn function_named<'a>(manifest: &'a Manifest, name: &str) -> &'a FunctionDef {
    manifest
        .functions
        .iter()
        .find(|item| item.host_name == name)
        .unwrap_or_else(|| panic!("missing function {name}"))
}

fn field_named<'a>(shape: &'a TypeShape, name: &str) -> &'a rspyts::ir::FieldDef {
    let TypeShape::Struct { fields } = shape else {
        panic!("expected a struct shape");
    };
    fields
        .iter()
        .find(|field| field.wire_name == name)
        .unwrap_or_else(|| panic!("missing field {name}"))
}

#[test]
fn discovery_returns_the_complete_application_contract() {
    let manifest = discovered_manifest();

    assert_eq!(manifest.ir_version, rspyts::ir::IR_VERSION);
    assert_eq!(manifest.package_name, env!("CARGO_PKG_NAME"));
    assert_eq!(manifest.package_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(manifest.module_name, "native");
    assert!(
        manifest
            .types
            .iter()
            .all(|item| item.owner.0 == env!("CARGO_PKG_NAME")
                && item.rust_module.starts_with("contract::fixtures"))
    );
    assert!(manifest.functions.iter().all(|item| {
        item.owner.0 == env!("CARGO_PKG_NAME") && item.rust_module == "contract::fixtures"
    }));
    assert_eq!(
        manifest
            .types
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>(),
        [
            "JobContext",
            "JobEvent",
            "JobId",
            "JobRequest",
            "JobResult",
            "RunMode",
            "InlineValue",
        ]
    );
    assert_eq!(
        manifest
            .functions
            .iter()
            .map(|item| item.host_name.as_str())
            .collect::<Vec<_>>(),
        ["executeJob", "reverseBytes", "scaleSamples"]
    );
    assert_eq!(
        manifest
            .resources
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>(),
        ["Counter"]
    );
    assert_eq!(
        manifest
            .constants
            .iter()
            .map(|item| item.host_name.as_str())
            .collect::<Vec<_>>(),
        ["CONTRACT_REVISION", "DEFAULT_LABEL"]
    );
    assert_eq!(
        manifest
            .errors
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>(),
        ["JobError"]
    );

    let encoded = serde_json::to_string(&manifest).expect("manifest must serialize");
    let decoded: Manifest = serde_json::from_str(&encoded).expect("manifest must deserialize");
    assert_eq!(decoded, manifest);

    let inline = type_named(&manifest, "InlineValue");
    assert_eq!(inline.rust_module, "contract::fixtures::inline");
    assert_eq!(
        manifest
            .namespace(&inline.owner, &inline.rust_module)
            .python_segments(),
        ["fixtures", "inline"]
    );
}

#[test]
fn request_fields_preserve_names_constraints_defaults_and_boundaries() {
    let manifest = discovered_manifest();
    let request = type_named(&manifest, "JobRequest");

    let display_name = field_named(&request.shape, "displayName");
    assert_eq!(display_name.rust_name, "display_name");
    assert_eq!(display_name.constraints.min_length, Some(2));
    assert_eq!(display_name.constraints.max_length, Some(64));

    let count = field_named(&request.shape, "count");
    assert_eq!(count.constraints.ge, Some(1));
    assert_eq!(count.constraints.le, Some(100));
    assert_eq!(field_named(&request.shape, "payload").ty, TypeRef::Bytes);
    assert_eq!(
        field_named(&request.shape, "samples").ty,
        TypeRef::Buffer {
            element: BufferElement::F64,
        }
    );

    let dry_run = field_named(&request.shape, "dryRun");
    assert!(!dry_run.required);
    assert_eq!(dry_run.default, Some(ScalarValue::Bool(false)));

    let context = field_named(&request.shape, "context");
    assert!(!context.required);
    assert_eq!(
        context.ty,
        TypeRef::Option {
            item: Box::new(TypeRef::Named {
                identity: type_named(&manifest, "JobContext").identity(),
            }),
        }
    );
}

#[test]
fn context_fields_cover_supported_composite_types() {
    let manifest = discovered_manifest();
    let context = type_named(&manifest, "JobContext");

    assert_eq!(
        field_named(&context.shape, "createdAt").ty,
        TypeRef::DateTime
    );
    assert_eq!(field_named(&context.shape, "metadata").ty, TypeRef::Json);
    assert_eq!(
        field_named(&context.shape, "labels").ty,
        TypeRef::Map {
            value: Box::new(TypeRef::String),
        }
    );
    assert_eq!(
        field_named(&context.shape, "range").ty,
        TypeRef::Tuple {
            items: vec![
                TypeRef::Int {
                    signed: true,
                    bits: 32,
                },
                TypeRef::Int {
                    signed: true,
                    bits: 32,
                },
            ],
        }
    );
    assert_eq!(
        field_named(&context.shape, "note").ty,
        TypeRef::Option {
            item: Box::new(TypeRef::String),
        }
    );
    assert_eq!(
        field_named(&context.shape, "fingerprint").ty,
        TypeRef::FixedBytes { length: 16 }
    );
}

#[test]
fn enums_and_aliases_preserve_their_wire_shapes() {
    let manifest = discovered_manifest();
    let run_mode = type_named(&manifest, "RunMode");
    let TypeShape::StringEnum { variants } = &run_mode.shape else {
        panic!("RunMode must be a string enum");
    };
    assert_eq!(
        variants
            .iter()
            .map(|variant| variant.wire_name.as_str())
            .collect::<Vec<_>>(),
        ["fast", "deterministic"]
    );

    let event = type_named(&manifest, "JobEvent");
    let TypeShape::TaggedEnum { tag, variants } = &event.shape else {
        panic!("JobEvent must be a tagged enum");
    };
    assert_eq!(tag, "kind");
    assert_eq!(variants[0].wire_name, "completed");
    assert_eq!(variants[0].fields[0].wire_name, "acceptedCount");

    assert_eq!(
        type_named(&manifest, "JobId").shape,
        TypeShape::Alias {
            target: TypeRef::String,
        }
    );
}

#[test]
fn functions_preserve_names_boundaries_and_typed_errors() {
    let manifest = discovered_manifest();
    let execute = function_named(&manifest, "executeJob");
    assert_eq!(execute.rust_name, "execute_job");
    assert!(matches!(execute.returns, TypeRef::Named { .. }));
    assert_eq!(execute.error, Some(manifest.errors[0].identity()));

    let reverse = function_named(&manifest, "reverseBytes");
    assert_eq!(reverse.params[0].ty, TypeRef::Bytes);
    assert_eq!(reverse.returns, TypeRef::Bytes);

    let scale = function_named(&manifest, "scaleSamples");
    assert_eq!(
        scale.params[0].ty,
        TypeRef::Buffer {
            element: BufferElement::F64,
        }
    );
    assert_eq!(scale.params[1].host_name, "factor");
    assert_eq!(
        scale.returns,
        TypeRef::Buffer {
            element: BufferElement::F64,
        }
    );
}

#[test]
fn resources_constants_and_error_codes_are_exact() {
    let manifest = discovered_manifest();
    let counter = &manifest.resources[0];

    assert_eq!(
        counter
            .constructors
            .iter()
            .map(|item| item.host_name.as_str())
            .collect::<Vec<_>>(),
        ["new", "fromZero"]
    );
    assert_eq!(
        counter
            .methods
            .iter()
            .map(|item| item.host_name.as_str())
            .collect::<Vec<_>>(),
        ["add"]
    );
    assert_eq!(
        counter.constructors[0].error,
        Some(manifest.errors[0].identity())
    );
    assert!(
        counter
            .methods
            .iter()
            .all(|method| method.host_name != "reset")
    );

    assert_eq!(manifest.constants[0].value, serde_json::json!(7));
    assert_eq!(
        manifest.constants[1].value,
        serde_json::json!("integration")
    );
    assert_eq!(fixtures::JobError::NegativeStart.code(), "negative_start");
}
