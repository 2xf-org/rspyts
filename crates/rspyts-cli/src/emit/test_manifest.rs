//! The manifest every emitter golden test renders: one struct with an
//! `Option` field and a `Json` field; a string enum; a data enum; an
//! error enum with data fields; a foreign-origin struct; one constant of
//! every rendered kind; one TypeScript-only function; a factory-only
//! class with a Python-only method; and one class with a constructor
//! plus a factory. Kept in one place so the validator tests and all
//! three emitters agree on the fixture.

use rspyts_core::ir::{
    ClassDecl, ConstDecl, CtorDecl, Dtype, ErrorVariantDecl, FieldDecl, FnDecl, Manifest,
    MethodDecl, ParamDecl, StaticDecl, StringVariantDecl, Target, Ty, TypeDecl, VariantDecl,
};
use serde_json::json;

fn field(name: &str, wire: &str, docs: &str, ty: Ty, optional: bool) -> FieldDecl {
    FieldDecl {
        name: name.to_string(),
        wire_name: wire.to_string(),
        docs: docs.to_string(),
        ty,
        optional,
    }
}

fn param(name: &str, wire: &str, ty: Ty) -> ParamDecl {
    ParamDecl {
        name: name.to_string(),
        wire_name: wire.to_string(),
        ty,
    }
}

/// The dependency crate one fixture type originates from; the imports
/// tests map it, the default goldens leave it unmapped.
pub const FOREIGN_ORIGIN: &str = "example-catalog";

/// Build the fixture. Sections are name-sorted, mirroring ABI §7.
pub fn manifest() -> Manifest {
    Manifest {
        abi: "0.1".to_string(),
        crate_name: "demo-crate".to_string(),
        crate_version: "0.1.0".to_string(),
        types: vec![
            TypeDecl::ErrorEnum {
                name: "AnalysisError".to_string(),
                docs: String::new(),
                origin: "demo-crate".to_string(),
                variants: vec![
                    ErrorVariantDecl {
                        name: "InvalidSampleRate".to_string(),
                        wire_code: "invalidSampleRate".to_string(),
                        docs: "The sample rate must be positive.".to_string(),
                        fields: vec![],
                    },
                    ErrorVariantDecl {
                        name: "WindowTooLarge".to_string(),
                        wire_code: "windowTooLarge".to_string(),
                        docs: String::new(),
                        fields: vec![field("max", "max", "", Ty::U32, false)],
                    },
                ],
            },
            TypeDecl::Struct {
                name: "AnalysisParams".to_string(),
                docs: "Parameters controlling the analysis pass.".to_string(),
                origin: "demo-crate".to_string(),
                fields: vec![
                    field(
                        "min_duration_s",
                        "minDurationS",
                        "Minimum duration, in seconds.",
                        Ty::F64,
                        false,
                    ),
                    field(
                        "threshold",
                        "threshold",
                        "",
                        Ty::Option {
                            inner: Box::new(Ty::F64),
                        },
                        true,
                    ),
                    field("metadata", "metadata", "", Ty::Json, false),
                ],
            },
            TypeDecl::Struct {
                name: "CatalogInfo".to_string(),
                docs: "Catalog description reported by the device.".to_string(),
                origin: FOREIGN_ORIGIN.to_string(),
                fields: vec![
                    field("vendor", "vendor", "", Ty::String, false),
                    field("channel_count", "channelCount", "", Ty::U16, false),
                ],
            },
            TypeDecl::StringEnum {
                name: "Severity".to_string(),
                docs: String::new(),
                origin: "demo-crate".to_string(),
                variants: ["Low", "Medium", "High"]
                    .iter()
                    .map(|name| StringVariantDecl {
                        name: name.to_string(),
                        wire_name: name.to_lowercase(),
                        docs: String::new(),
                    })
                    .collect(),
            },
            TypeDecl::Enum {
                name: "ThresholdEvent".to_string(),
                docs: "Signal threshold transitions.".to_string(),
                origin: "demo-crate".to_string(),
                tag: "kind".to_string(),
                variants: vec![
                    VariantDecl {
                        name: "Crossed".to_string(),
                        wire_name: "crossed".to_string(),
                        docs: String::new(),
                        fields: vec![
                            field("at_sample", "atSample", "", Ty::U32, false),
                            field("value", "value", "", Ty::F64, false),
                        ],
                    },
                    VariantDecl {
                        name: "Cleared".to_string(),
                        wire_name: "cleared".to_string(),
                        docs: String::new(),
                        fields: vec![field("at_sample", "atSample", "", Ty::U32, false)],
                    },
                ],
            },
        ],
        constants: vec![
            ConstDecl {
                name: "DEFAULT_PARAMS".to_string(),
                docs: "Baseline analysis parameters.".to_string(),
                origin: "demo-crate".to_string(),
                ty: Ty::Ref {
                    name: "AnalysisParams".to_string(),
                },
                value: json!({
                    "minDurationS": 0.5,
                    "threshold": null,
                    "metadata": {"rev": 2}
                }),
            },
            ConstDecl {
                name: "DEFAULT_THRESHOLD".to_string(),
                docs: String::new(),
                origin: "demo-crate".to_string(),
                ty: Ty::F64,
                value: json!(0.75),
            },
            ConstDecl {
                name: "ENGINE_NAME".to_string(),
                docs: "Name reported by the analysis engine.".to_string(),
                origin: "demo-crate".to_string(),
                ty: Ty::String,
                value: json!("neuro-engine"),
            },
            ConstDecl {
                name: "SUPPORTED_UNITS".to_string(),
                docs: String::new(),
                origin: "demo-crate".to_string(),
                ty: Ty::List {
                    inner: Box::new(Ty::String),
                },
                value: json!(["uV", "mV"]),
            },
        ],
        functions: vec![
            FnDecl {
                name: "analyze_signal".to_string(),
                docs: "Analyze a signal buffer.".to_string(),
                params: vec![
                    param("samples", "samples", Ty::Slice { dt: Dtype::F64 }),
                    param("sample_rate", "sampleRate", Ty::U32),
                    param(
                        "params",
                        "params",
                        Ty::Ref {
                            name: "AnalysisParams".to_string(),
                        },
                    ),
                ],
                ret: Ty::Buf { dt: Dtype::F64 },
                err: Some("AnalysisError".to_string()),
                targets: Target::all(),
            },
            FnDecl {
                name: "render_report".to_string(),
                docs: "Render an HTML report.".to_string(),
                params: vec![],
                ret: Ty::String,
                err: None,
                targets: vec![Target::Typescript],
            },
        ],
        classes: vec![
            // Factory-only: no constructor, built through `open`.
            ClassDecl {
                name: "Recording".to_string(),
                docs: "A recorded session backed by a native handle.".to_string(),
                constructor: None,
                methods: vec![
                    MethodDecl {
                        name: "duration_s".to_string(),
                        docs: "Total duration in seconds.".to_string(),
                        mutable: false,
                        params: vec![],
                        ret: Ty::F64,
                        err: None,
                        targets: Target::all(),
                    },
                    MethodDecl {
                        name: "info".to_string(),
                        docs: String::new(),
                        mutable: false,
                        params: vec![],
                        ret: Ty::Ref {
                            name: "CatalogInfo".to_string(),
                        },
                        err: None,
                        targets: Target::all(),
                    },
                    MethodDecl {
                        name: "preload".to_string(),
                        docs: "Eagerly load samples into memory.".to_string(),
                        mutable: true,
                        params: vec![],
                        ret: Ty::Unit,
                        err: None,
                        targets: vec![Target::Python],
                    },
                ],
                statics: vec![
                    StaticDecl {
                        name: "open".to_string(),
                        docs: "Open a recording from disk.".to_string(),
                        params: vec![param("path", "path", Ty::String)],
                        ret: Ty::Unit,
                        err: Some("AnalysisError".to_string()),
                        returns_self: true,
                        targets: Target::all(),
                    },
                    StaticDecl {
                        name: "default_extension".to_string(),
                        docs: String::new(),
                        params: vec![],
                        ret: Ty::String,
                        err: None,
                        returns_self: false,
                        targets: Target::all(),
                    },
                ],
            },
            ClassDecl {
                name: "RunningStats".to_string(),
                docs: "Streaming statistics over a sliding window.".to_string(),
                constructor: Some(CtorDecl {
                    docs: String::new(),
                    params: vec![param("window", "window", Ty::U32)],
                    err: None,
                }),
                methods: vec![
                    MethodDecl {
                        name: "push".to_string(),
                        docs: String::new(),
                        mutable: true,
                        params: vec![param("chunk", "chunk", Ty::Slice { dt: Dtype::F64 })],
                        ret: Ty::Unit,
                        err: None,
                        targets: Target::all(),
                    },
                    MethodDecl {
                        name: "snapshot".to_string(),
                        docs: "Snapshot current state.".to_string(),
                        mutable: false,
                        params: vec![],
                        ret: Ty::Ref {
                            name: "AnalysisParams".to_string(),
                        },
                        err: None,
                        targets: Target::all(),
                    },
                ],
                statics: vec![StaticDecl {
                    name: "resumed".to_string(),
                    docs: "Rebuild from a snapshot.".to_string(),
                    params: vec![param(
                        "state",
                        "state",
                        Ty::Ref {
                            name: "AnalysisParams".to_string(),
                        },
                    )],
                    ret: Ty::Unit,
                    err: None,
                    returns_self: true,
                    targets: Target::all(),
                }],
            },
        ],
    }
}

/// The manifest-hash the emitters would receive for this fixture: the
/// SHA-256 of its serialized JSON, exactly as in the real pipeline.
pub fn manifest_hash(m: &Manifest) -> String {
    let json = serde_json::to_vec(m).expect("manifest serializes");
    super::util::manifest_hash_hex(&json)
}
