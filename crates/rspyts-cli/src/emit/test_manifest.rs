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
pub const FOREIGN_ORIGIN: &str = "shared-types";

/// Build the fixture. Sections are name-sorted, mirroring ABI §7.
pub fn manifest() -> Manifest {
    Manifest {
        abi: "2.0".to_string(),
        crate_name: "demo-crate".to_string(),
        crate_version: "0.1.0".to_string(),
        types: vec![
            TypeDecl::ErrorEnum {
                name: "QueryError".to_string(),
                docs: String::new(),
                origin: "demo-crate".to_string(),
                variants: vec![
                    ErrorVariantDecl {
                        name: "InvalidBatchSize".to_string(),
                        wire_code: "invalidBatchSize".to_string(),
                        docs: "The batch size must be positive.".to_string(),
                        fields: vec![],
                    },
                    ErrorVariantDecl {
                        name: "BatchTooLarge".to_string(),
                        wire_code: "batchTooLarge".to_string(),
                        docs: String::new(),
                        fields: vec![field("max", "max", "", Ty::U32, false)],
                    },
                ],
            },
            TypeDecl::Struct {
                name: "QueryOptions".to_string(),
                docs: "Options controlling value processing.".to_string(),
                origin: "demo-crate".to_string(),
                fields: vec![
                    field(
                        "minimum_value",
                        "minimumValue",
                        "Minimum value to include.",
                        Ty::F64,
                        false,
                    ),
                    field(
                        "tolerance",
                        "tolerance",
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
                name: "SourceInfo".to_string(),
                docs: "Description of an input source.".to_string(),
                origin: FOREIGN_ORIGIN.to_string(),
                fields: vec![
                    field("name", "name", "", Ty::String, false),
                    field("field_count", "fieldCount", "", Ty::U16, false),
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
                name: "ValueEvent".to_string(),
                docs: "Value-processing transitions.".to_string(),
                origin: "demo-crate".to_string(),
                tag: "kind".to_string(),
                variants: vec![
                    VariantDecl {
                        name: "Accepted".to_string(),
                        wire_name: "accepted".to_string(),
                        docs: String::new(),
                        fields: vec![
                            field("index", "index", "", Ty::U32, false),
                            field("value", "value", "", Ty::F64, false),
                        ],
                    },
                    VariantDecl {
                        name: "Rejected".to_string(),
                        wire_name: "rejected".to_string(),
                        docs: String::new(),
                        fields: vec![field("index", "index", "", Ty::U32, false)],
                    },
                ],
            },
        ],
        constants: vec![
            ConstDecl {
                name: "DEFAULT_OPTIONS".to_string(),
                docs: "Baseline processing options.".to_string(),
                origin: "demo-crate".to_string(),
                ty: Ty::Ref {
                    name: "QueryOptions".to_string(),
                },
                value: json!({
                    "minimumValue": 0.5,
                    "tolerance": null,
                    "metadata": {"rev": 2}
                }),
            },
            ConstDecl {
                name: "DEFAULT_LIMIT".to_string(),
                docs: String::new(),
                origin: "demo-crate".to_string(),
                ty: Ty::F64,
                value: json!(0.75),
            },
            ConstDecl {
                name: "PROCESSOR_NAME".to_string(),
                docs: "Name reported by the value processor.".to_string(),
                origin: "demo-crate".to_string(),
                ty: Ty::String,
                value: json!("vector-processor"),
            },
            ConstDecl {
                name: "SUPPORTED_FORMATS".to_string(),
                docs: String::new(),
                origin: "demo-crate".to_string(),
                ty: Ty::List {
                    inner: Box::new(Ty::String),
                },
                value: json!(["csv", "json"]),
            },
        ],
        functions: vec![
            FnDecl {
                name: "process_values".to_string(),
                docs: "Process a buffer of numeric values.".to_string(),
                params: vec![
                    param("values", "values", Ty::Slice { dt: Dtype::F64 }),
                    param("batch_size", "batchSize", Ty::U32),
                    param(
                        "options",
                        "options",
                        Ty::Ref {
                            name: "QueryOptions".to_string(),
                        },
                    ),
                ],
                ret: Ty::Buf { dt: Dtype::F64 },
                err: Some("QueryError".to_string()),
                targets: Target::all(),
            },
            FnDecl {
                name: "render_summary".to_string(),
                docs: "Render an HTML summary.".to_string(),
                params: vec![],
                ret: Ty::String,
                err: None,
                targets: vec![Target::Typescript],
            },
        ],
        classes: vec![
            // Factory-only: no constructor, built through `open`.
            ClassDecl {
                name: "Session".to_string(),
                docs: "A processing session backed by a native handle.".to_string(),
                constructor: None,
                methods: vec![
                    MethodDecl {
                        name: "progress".to_string(),
                        docs: "Current completion ratio.".to_string(),
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
                            name: "SourceInfo".to_string(),
                        },
                        err: None,
                        targets: Target::all(),
                    },
                    MethodDecl {
                        name: "warm_up".to_string(),
                        docs: "Prepare the session for processing.".to_string(),
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
                        docs: "Open a processing session from disk.".to_string(),
                        params: vec![param("path", "path", Ty::String)],
                        ret: Ty::Unit,
                        err: Some("QueryError".to_string()),
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
                            name: "QueryOptions".to_string(),
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
                            name: "QueryOptions".to_string(),
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

/// Build a focused attachment/newtype fixture without expanding the broad
/// emitter goldens above. This keeps binary projection tests readable while
/// still making every emitter consume the same manifest shape.
pub fn binary_manifest() -> Manifest {
    let mut m = manifest();
    m.types.push(TypeDecl::Newtype {
        name: "PacketId".to_string(),
        docs: "Stable packet identifier.".to_string(),
        origin: "demo-crate".to_string(),
        inner: Ty::U32,
    });
    m.types.push(TypeDecl::Struct {
        name: "BinaryPacket".to_string(),
        docs: "Nested binary attachment fixture.".to_string(),
        origin: "demo-crate".to_string(),
        fields: vec![
            field(
                "id",
                "id",
                "",
                Ty::Ref {
                    name: "PacketId".to_string(),
                },
                false,
            ),
            field("payload", "payload", "", Ty::Bytes, false),
            field("samples", "samples", "", Ty::Buf { dt: Dtype::F64 }, false),
            field(
                "chunks",
                "chunks",
                "",
                Ty::List {
                    inner: Box::new(Ty::Buf { dt: Dtype::I16 }),
                },
                false,
            ),
            field(
                "channels",
                "channels",
                "",
                Ty::Map {
                    value: Box::new(Ty::Buf { dt: Dtype::U8 }),
                },
                false,
            ),
        ],
    });
    m.functions.push(FnDecl {
        name: "echo_binary_packet".to_string(),
        docs: String::new(),
        params: vec![param(
            "value",
            "value",
            Ty::Ref {
                name: "BinaryPacket".to_string(),
            },
        )],
        ret: Ty::Ref {
            name: "BinaryPacket".to_string(),
        },
        err: None,
        targets: Target::all(),
    });
    m.functions.push(FnDecl {
        name: "echo_bytes".to_string(),
        docs: String::new(),
        params: vec![param("value", "value", Ty::Bytes)],
        ret: Ty::Bytes,
        err: None,
        targets: Target::all(),
    });
    m.functions.push(FnDecl {
        name: "echo_u8".to_string(),
        docs: String::new(),
        params: vec![param("value", "value", Ty::Buf { dt: Dtype::U8 })],
        ret: Ty::Buf { dt: Dtype::U8 },
        err: None,
        targets: Target::all(),
    });
    m
}

/// Build the exact-integer, tuple, and mixed-enum fixture shared by all
/// emitters. It deliberately nests exact values through every structural
/// container so converters cannot accidentally handle only top-level cases.
pub fn exact_manifest() -> Manifest {
    let mut m = manifest();
    m.types.push(TypeDecl::Newtype {
        name: "SequenceId".to_string(),
        docs: String::new(),
        origin: "demo-crate".to_string(),
        inner: Ty::U64,
    });
    m.types.push(TypeDecl::Struct {
        name: "ExactRecord".to_string(),
        docs: String::new(),
        origin: "demo-crate".to_string(),
        fields: vec![
            field(
                "sequence",
                "sequence",
                "",
                Ty::Ref {
                    name: "SequenceId".into(),
                },
                false,
            ),
            field("delta", "delta", "", Ty::I64, false),
            field(
                "pair",
                "pair",
                "",
                Ty::Tuple {
                    items: vec![Ty::I64, Ty::U64],
                },
                false,
            ),
            field(
                "nested",
                "nested",
                "",
                Ty::List {
                    inner: Box::new(Ty::Map {
                        value: Box::new(Ty::Option {
                            inner: Box::new(Ty::U64),
                        }),
                    }),
                },
                false,
            ),
        ],
    });
    m.types.push(TypeDecl::Enum {
        name: "MixedResult".to_string(),
        docs: String::new(),
        origin: "demo-crate".to_string(),
        tag: "type".to_string(),
        variants: vec![
            VariantDecl {
                name: "Pending".to_string(),
                wire_name: "pending".to_string(),
                docs: String::new(),
                fields: vec![],
            },
            VariantDecl {
                name: "Ready".to_string(),
                wire_name: "ready".to_string(),
                docs: String::new(),
                fields: vec![field("total", "total", "", Ty::U64, false)],
            },
        ],
    });
    if let TypeDecl::ErrorEnum { variants, .. } = &mut m.types[0] {
        variants[1]
            .fields
            .push(field("exact_limit", "exactLimit", "", Ty::U64, false));
    }
    m.constants.push(ConstDecl {
        name: "MAX_SEQUENCE".to_string(),
        docs: String::new(),
        origin: "demo-crate".to_string(),
        ty: Ty::Ref {
            name: "SequenceId".to_string(),
        },
        value: json!(u64::MAX.to_string()),
    });
    m.constants.push(ConstDecl {
        name: "EXACT_PAIR".to_string(),
        docs: String::new(),
        origin: "demo-crate".to_string(),
        ty: Ty::Tuple {
            items: vec![Ty::I64, Ty::U64],
        },
        value: json!([i64::MIN.to_string(), u64::MAX.to_string()]),
    });
    m.functions.push(FnDecl {
        name: "round_trip_exact".to_string(),
        docs: String::new(),
        params: vec![param(
            "value",
            "value",
            Ty::Ref {
                name: "ExactRecord".to_string(),
            },
        )],
        ret: Ty::Ref {
            name: "ExactRecord".to_string(),
        },
        err: None,
        targets: Target::all(),
    });
    m.functions.push(FnDecl {
        name: "round_trip_mixed".to_string(),
        docs: String::new(),
        params: vec![param(
            "value",
            "value",
            Ty::Ref {
                name: "MixedResult".to_string(),
            },
        )],
        ret: Ty::Ref {
            name: "MixedResult".to_string(),
        },
        err: None,
        targets: Target::all(),
    });
    m.functions.push(FnDecl {
        name: "round_trip_tuple".to_string(),
        docs: String::new(),
        params: vec![param(
            "value",
            "value",
            Ty::Tuple {
                items: vec![Ty::I64, Ty::U64],
            },
        )],
        ret: Ty::Tuple {
            items: vec![Ty::I64, Ty::U64],
        },
        err: None,
        targets: Target::all(),
    });
    m
}

/// The manifest-hash the emitters would receive for this fixture: the
/// SHA-256 of its serialized JSON, exactly as in the real pipeline.
pub fn manifest_hash(m: &Manifest) -> String {
    let json = serde_json::to_vec(m).expect("manifest serializes");
    super::util::manifest_hash_hex(&json)
}
