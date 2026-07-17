use std::collections::BTreeMap;

use rspyts::__private::{BufferValue, WireValue};
use rspyts::backend::normalize_wire;
use rspyts::ir::{
    BufferElement, CargoPackageId, DefinitionId, EnumVariantDef, FieldConstraints, FieldDef,
    ScalarValue, TypeDef, TypeRef, TypeShape,
};

fn field(name: &str, ty: TypeRef, required: bool) -> FieldDef {
    FieldDef {
        rust_name: name.to_owned(),
        wire_name: name.to_owned(),
        docs: None,
        ty,
        required,
        default: None,
        constraints: Default::default(),
    }
}

fn owner() -> CargoPackageId {
    CargoPackageId::new("schema-tests")
}

fn named(id: &str) -> TypeRef {
    TypeRef::Named {
        identity: DefinitionId::new("schema-tests", id),
    }
}

#[test]
fn nested_contract_restores_integer_sign_and_preserves_buffer_dtype() {
    let types = vec![TypeDef {
        owner: owner(),
        id: "Summary".to_owned(),
        name: "Summary".to_owned(),
        docs: None,
        shape: TypeShape::Struct {
            fields: vec![
                field(
                    "count",
                    TypeRef::Int {
                        signed: false,
                        bits: 64,
                    },
                    true,
                ),
                field(
                    "samples",
                    TypeRef::Buffer {
                        element: BufferElement::F64,
                    },
                    true,
                ),
            ],
        },
    }];
    let value = WireValue::Object(BTreeMap::from([
        ("count".to_owned(), WireValue::I64(7)),
        (
            "samples".to_owned(),
            WireValue::Buffer(BufferValue::F64(vec![1.0, 2.0])),
        ),
    ]));

    let normalized = normalize_wire(&value, &named("Summary"), &types).unwrap();

    let WireValue::Object(fields) = normalized else {
        panic!("expected normalized object");
    };
    assert_eq!(fields["count"], WireValue::U64(7));
    assert_eq!(
        fields["samples"],
        WireValue::Buffer(BufferValue::F64(vec![1.0, 2.0]))
    );
}

#[test]
fn integer_width_and_buffer_dtype_mismatches_are_rejected() {
    let integer = normalize_wire(
        &WireValue::I64(128),
        &TypeRef::Int {
            signed: true,
            bits: 8,
        },
        &[],
    )
    .unwrap_err();
    assert!(integer.to_string().contains("i8"));

    let buffer = normalize_wire(
        &WireValue::Buffer(BufferValue::F32(vec![1.0])),
        &TypeRef::Buffer {
            element: BufferElement::F64,
        },
        &[],
    )
    .unwrap_err();
    assert!(buffer.to_string().contains("expected f64"));
}

#[test]
fn integer_wire_values_widen_to_floats_without_relaxing_float_validation() {
    assert_eq!(
        normalize_wire(&WireValue::I64(-42), &TypeRef::Float { bits: 64 }, &[]).unwrap(),
        WireValue::F64(-42.0),
    );
    assert_eq!(
        normalize_wire(&WireValue::U64(42), &TypeRef::Float { bits: 32 }, &[]).unwrap(),
        WireValue::F64(42.0),
    );

    assert!(
        normalize_wire(
            &WireValue::F64(f64::INFINITY),
            &TypeRef::Float { bits: 64 },
            &[],
        )
        .is_err()
    );
    assert!(normalize_wire(&WireValue::F64(f64::MAX), &TypeRef::Float { bits: 32 }, &[],).is_err());
}

#[test]
fn json_rejects_non_json_wire_categories_at_any_depth() {
    let value = WireValue::Object(BTreeMap::from([(
        "nested".to_owned(),
        WireValue::Sequence(vec![WireValue::Bytes(vec![1, 2, 3])]),
    )]));
    let error = normalize_wire(&value, &TypeRef::Json, &[]).unwrap_err();
    assert!(error.to_string().contains("$.nested[0]"));
}

#[test]
fn json_numbers_are_finite_and_safe_at_every_depth() {
    let safe = WireValue::Object(BTreeMap::from([(
        "nested".to_owned(),
        WireValue::Sequence(vec![
            WireValue::I64(-9_007_199_254_740_991),
            WireValue::U64(9_007_199_254_740_991),
            WireValue::F64(1.25),
            WireValue::F64(9_007_199_254_740_991.0),
        ]),
    )]));
    assert_eq!(normalize_wire(&safe, &TypeRef::Json, &[]).unwrap(), safe);

    for (value, path_is_preserved) in [
        (WireValue::I64(-9_007_199_254_740_992), true),
        (WireValue::U64(9_007_199_254_740_992), true),
        (WireValue::F64(9_007_199_254_740_992.0), true),
        (WireValue::F64(f64::INFINITY), false),
        (WireValue::F64(f64::NAN), false),
    ] {
        let nested = WireValue::Object(BTreeMap::from([(
            "nested".to_owned(),
            WireValue::Sequence(vec![WireValue::Null, value]),
        )]));
        let error = normalize_wire(&nested, &TypeRef::Json, &[]).unwrap_err();
        if path_is_preserved {
            assert!(
                error.to_string().contains("$.nested[1]"),
                "unexpected error: {error}"
            );
        } else {
            assert!(
                error
                    .to_string()
                    .contains("structured floating-point values must be finite"),
                "unexpected error: {error}"
            );
        }
    }
}

#[test]
fn tagged_enums_are_strict_and_keep_the_declared_tag() {
    let types = vec![TypeDef {
        owner: owner(),
        id: "Selection".to_owned(),
        name: "Selection".to_owned(),
        docs: None,
        shape: TypeShape::TaggedEnum {
            tag: "kind".to_owned(),
            variants: vec![EnumVariantDef {
                rust_name: "Included".to_owned(),
                wire_name: "included".to_owned(),
                docs: None,
                fields: vec![field(
                    "sequence",
                    TypeRef::Int {
                        signed: false,
                        bits: 32,
                    },
                    true,
                )],
            }],
        },
    }];
    let value = WireValue::Object(BTreeMap::from([
        ("kind".to_owned(), WireValue::String("included".to_owned())),
        ("sequence".to_owned(), WireValue::I64(4)),
    ]));

    let normalized = normalize_wire(&value, &named("Selection"), &types).unwrap();
    assert_eq!(
        normalized,
        WireValue::Object(BTreeMap::from([
            ("kind".to_owned(), WireValue::String("included".to_owned()),),
            ("sequence".to_owned(), WireValue::U64(4)),
        ]))
    );
}

#[test]
fn optional_null_skips_constraints_for_the_non_null_item() {
    let types = vec![
        TypeDef {
            owner: owner(),
            id: "OptionalString".to_owned(),
            name: "OptionalString".to_owned(),
            docs: None,
            shape: TypeShape::Alias {
                target: TypeRef::Option {
                    item: Box::new(TypeRef::String),
                },
            },
        },
        TypeDef {
            owner: owner(),
            id: "OptionalLabel".to_owned(),
            name: "OptionalLabel".to_owned(),
            docs: None,
            shape: TypeShape::Struct {
                fields: vec![
                    FieldDef {
                        rust_name: "label".to_owned(),
                        wire_name: "label".to_owned(),
                        docs: None,
                        ty: TypeRef::Option {
                            item: Box::new(TypeRef::String),
                        },
                        required: false,
                        default: None,
                        constraints: FieldConstraints {
                            min_length: Some(1),
                            ..Default::default()
                        },
                    },
                    FieldDef {
                        rust_name: "aliased_label".to_owned(),
                        wire_name: "aliasedLabel".to_owned(),
                        docs: None,
                        ty: named("OptionalString"),
                        required: false,
                        default: None,
                        constraints: FieldConstraints {
                            literal: Some(ScalarValue::String("ready".to_owned())),
                            min_length: Some(1),
                            ..Default::default()
                        },
                    },
                ],
            },
        },
    ];
    let value = WireValue::Object(BTreeMap::from([
        ("label".to_owned(), WireValue::Null),
        ("aliasedLabel".to_owned(), WireValue::Null),
    ]));

    assert_eq!(
        normalize_wire(&value, &named("OptionalLabel"), &types).unwrap(),
        value
    );
}

#[test]
fn missing_defaults_are_inserted_and_constraints_use_field_paths() {
    let types = vec![TypeDef {
        owner: owner(),
        id: "Batch".to_owned(),
        name: "Batch".to_owned(),
        docs: None,
        shape: TypeShape::Struct {
            fields: vec![FieldDef {
                rust_name: "quantity".to_owned(),
                wire_name: "quantity".to_owned(),
                docs: None,
                ty: TypeRef::Int {
                    signed: false,
                    bits: 64,
                },
                required: false,
                default: Some(ScalarValue::I64(1)),
                constraints: FieldConstraints {
                    ge: Some(1),
                    ..Default::default()
                },
            }],
        },
    }];

    let normalized =
        normalize_wire(&WireValue::Object(BTreeMap::new()), &named("Batch"), &types).unwrap();
    assert_eq!(
        normalized,
        WireValue::Object(BTreeMap::from([
            ("quantity".to_owned(), WireValue::U64(1),)
        ]))
    );

    let error = normalize_wire(
        &WireValue::Object(BTreeMap::from([("quantity".to_owned(), WireValue::U64(0))])),
        &named("Batch"),
        &types,
    )
    .unwrap_err();
    assert!(error.to_string().contains("$.quantity"));
    assert!(error.to_string().contains(">= 1"));
}

#[test]
fn datetime_requires_an_aware_rfc3339_value() {
    let aware = WireValue::String("2026-07-16T12:30:00-04:00".to_owned());
    assert_eq!(
        normalize_wire(&aware, &TypeRef::DateTime, &[]).unwrap(),
        aware
    );

    let error = normalize_wire(
        &WireValue::String("2026-07-16T12:30:00".to_owned()),
        &TypeRef::DateTime,
        &[],
    )
    .unwrap_err();
    assert!(error.to_string().contains("aware RFC3339 datetime"));
}

#[test]
fn fixed_bytes_are_enforced_recursively_with_the_field_path() {
    let ty = TypeRef::FixedBytes { length: 4 };
    let exact = WireValue::Bytes(vec![1, 2, 3, 4]);
    assert_eq!(normalize_wire(&exact, &ty, &[]).unwrap(), exact);

    for invalid in [
        WireValue::Bytes(vec![1, 2, 3]),
        WireValue::Bytes(vec![1, 2, 3, 4, 5]),
    ] {
        let error = normalize_wire(&invalid, &ty, &[]).unwrap_err();
        assert!(error.to_string().contains("expected 4-byte array"));
    }

    let types = vec![TypeDef {
        owner: owner(),
        id: "Envelope".to_owned(),
        name: "Envelope".to_owned(),
        docs: None,
        shape: TypeShape::Struct {
            fields: vec![field("digest", ty, true)],
        },
    }];
    let invalid = WireValue::Object(BTreeMap::from([(
        "digest".to_owned(),
        WireValue::Bytes(vec![1, 2, 3]),
    )]));
    let error = normalize_wire(&invalid, &named("Envelope"), &types).unwrap_err();
    assert!(error.to_string().contains("$.digest"));
    assert!(error.to_string().contains("expected 4-byte array"));
}

#[test]
fn fixed_bytes_field_constraints_support_direct_and_named_aliases() {
    let mut direct = field("direct", TypeRef::FixedBytes { length: 4 }, true);
    direct.constraints.min_length = Some(3);
    direct.constraints.max_length = Some(4);
    let mut aliased = field("aliased", named("Digest"), true);
    aliased.constraints.min_length = Some(4);
    aliased.constraints.max_length = Some(5);
    let types = vec![
        TypeDef {
            owner: owner(),
            id: "Digest".to_owned(),
            name: "Digest".to_owned(),
            docs: None,
            shape: TypeShape::Alias {
                target: TypeRef::FixedBytes { length: 4 },
            },
        },
        TypeDef {
            owner: owner(),
            id: "Envelope".to_owned(),
            name: "Envelope".to_owned(),
            docs: None,
            shape: TypeShape::Struct {
                fields: vec![direct, aliased],
            },
        },
    ];
    let value = WireValue::Object(BTreeMap::from([
        ("direct".to_owned(), WireValue::Bytes(vec![1, 2, 3, 4])),
        ("aliased".to_owned(), WireValue::Bytes(vec![5, 6, 7, 8])),
    ]));

    assert_eq!(
        normalize_wire(&value, &named("Envelope"), &types).unwrap(),
        value
    );

    let mut direct_too_short = types.clone();
    let TypeShape::Struct { fields } = &mut direct_too_short[1].shape else {
        panic!("expected Envelope struct");
    };
    fields[0].constraints.min_length = Some(5);
    let error = normalize_wire(&value, &named("Envelope"), &direct_too_short).unwrap_err();
    assert!(error.to_string().contains("$.direct"));
    assert!(error.to_string().contains("length >= 5"));

    let mut alias_too_long = types;
    let TypeShape::Struct { fields } = &mut alias_too_long[1].shape else {
        panic!("expected Envelope struct");
    };
    fields[1].constraints.max_length = Some(3);
    let error = normalize_wire(&value, &named("Envelope"), &alias_too_long).unwrap_err();
    assert!(error.to_string().contains("$.aliased"));
    assert!(error.to_string().contains("length <= 3"));
}
