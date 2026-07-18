use std::collections::BTreeMap;

use rspyts::__private::{BufferValue, WireValue};
use rspyts::codec::{decode, encode};
use rspyts::ir::{
    BufferElement, CargoPackageId, DefinitionId, EnumVariantDef, FieldConstraints, FieldDef,
    TypeDef, TypeRef, TypeShape,
};
use serde::{Deserialize, Serialize};

fn field(name: &str, ty: TypeRef) -> FieldDef {
    FieldDef {
        rust_name: name.to_owned(),
        wire_name: name.to_owned(),
        docs: None,
        ty,
        required: true,
        default: None,
        constraints: Default::default(),
    }
}

fn owner() -> CargoPackageId {
    CargoPackageId::new("codec-tests")
}

fn named(id: &str) -> TypeRef {
    TypeRef::Named {
        identity: DefinitionId::new("codec-tests", id),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Nested {
    sequence: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Payload {
    minimum: i64,
    maximum: u64,
    bytes: Vec<u8>,
    samples: Vec<f64>,
    narrow_samples: Vec<f32>,
    nested: Option<Nested>,
    metadata: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct ConstrainedOutput {
    quantity: u64,
}

fn payload_types() -> Vec<TypeDef> {
    vec![
        TypeDef {
            owner: owner(),
            id: "Nested".to_owned(),
            name: "Nested".to_owned(),
            docs: None,
            shape: TypeShape::Struct {
                fields: vec![field(
                    "sequence",
                    TypeRef::Int {
                        signed: false,
                        bits: 32,
                    },
                )],
            },
        },
        TypeDef {
            owner: owner(),
            id: "Payload".to_owned(),
            name: "Payload".to_owned(),
            docs: None,
            shape: TypeShape::Struct {
                fields: vec![
                    field(
                        "minimum",
                        TypeRef::Int {
                            signed: true,
                            bits: 64,
                        },
                    ),
                    field(
                        "maximum",
                        TypeRef::Int {
                            signed: false,
                            bits: 64,
                        },
                    ),
                    field("bytes", TypeRef::Bytes),
                    field(
                        "samples",
                        TypeRef::Buffer {
                            element: BufferElement::F64,
                        },
                    ),
                    field(
                        "narrowSamples",
                        TypeRef::Buffer {
                            element: BufferElement::F32,
                        },
                    ),
                    field(
                        "nested",
                        TypeRef::Option {
                            item: Box::new(named("Nested")),
                        },
                    ),
                    field("metadata", TypeRef::Json),
                ],
            },
        },
    ]
}

#[test]
fn nested_rust_values_round_trip_without_json_loss() {
    let nan = f64::from_bits(0x7ff8_0000_0000_0042);
    let narrow_nan = f32::from_bits(0x7fc0_0042);
    let value = Payload {
        minimum: i64::MIN,
        maximum: u64::MAX,
        bytes: vec![0, 1, u8::MAX],
        samples: vec![nan, f64::INFINITY, f64::NEG_INFINITY, -0.0],
        narrow_samples: vec![narrow_nan, f32::INFINITY, -0.0],
        nested: Some(Nested { sequence: u32::MAX }),
        metadata: serde_json::json!({"safe": 42, "items": [true, null]}),
    };
    let types = payload_types();
    let ty = named("Payload");

    let wire = encode(&value, &ty, &types).unwrap();
    let WireValue::Object(fields) = &wire else {
        panic!("expected payload object");
    };
    assert_eq!(fields["minimum"], WireValue::I64(i64::MIN));
    assert_eq!(fields["maximum"], WireValue::U64(u64::MAX));
    assert_eq!(fields["bytes"], WireValue::Bytes(vec![0, 1, u8::MAX]));
    let WireValue::Buffer(BufferValue::F64(samples)) = &fields["samples"] else {
        panic!("expected f64 buffer");
    };
    assert_eq!(samples[0].to_bits(), nan.to_bits());
    assert!(samples[1].is_infinite() && samples[1].is_sign_positive());
    assert!(samples[2].is_infinite() && samples[2].is_sign_negative());

    let decoded: Payload = decode(wire, &ty, &types).unwrap();
    assert_eq!(decoded.minimum, value.minimum);
    assert_eq!(decoded.maximum, value.maximum);
    assert_eq!(decoded.bytes, value.bytes);
    assert_eq!(decoded.samples[0].to_bits(), nan.to_bits());
    assert!(decoded.samples[1].is_infinite());
    assert_eq!(decoded.narrow_samples[0].to_bits(), narrow_nan.to_bits());
    assert_eq!(decoded.nested, value.nested);
    assert_eq!(decoded.metadata, value.metadata);
}

#[test]
fn json_codec_rejects_nested_unsafe_and_nonfinite_numbers() {
    let unsafe_json = serde_json::json!({"items": [{"count": 9_007_199_254_740_992_u64}]});
    let error = encode(&unsafe_json, &TypeRef::Json, &[]).unwrap_err();
    assert!(
        error.to_string().contains("$.items[0].count"),
        "unexpected error: {error}"
    );

    let unsafe_wire = WireValue::Object(BTreeMap::from([(
        "items".to_owned(),
        WireValue::Sequence(vec![WireValue::Object(BTreeMap::from([(
            "count".to_owned(),
            WireValue::F64(f64::INFINITY),
        )]))]),
    )]));
    let error = decode::<serde_json::Value>(unsafe_wire, &TypeRef::Json, &[]).unwrap_err();
    assert!(
        error.to_string().contains("finite"),
        "unexpected error: {error}"
    );

    let safe_json = serde_json::json!({
        "minimum": -9_007_199_254_740_991_i64,
        "maximum": 9_007_199_254_740_991_u64,
        "fraction": 1.25,
    });
    assert_eq!(
        decode::<serde_json::Value>(
            encode(&safe_json, &TypeRef::Json, &[]).unwrap(),
            &TypeRef::Json,
            &[],
        )
        .unwrap(),
        safe_json,
    );
}

#[test]
fn every_rust_numeric_vector_uses_the_declared_buffer_dtype() {
    macro_rules! check {
        ($values:expr, $element:ident, $variant:ident, $type:ty) => {{
            let values: Vec<$type> = $values;
            let ty = TypeRef::Buffer {
                element: BufferElement::$element,
            };
            let wire = encode(&values, &ty, &[]).unwrap();
            assert!(matches!(wire, WireValue::Buffer(BufferValue::$variant(_))));
            let decoded: Vec<$type> = decode(wire, &ty, &[]).unwrap();
            assert_eq!(decoded, values);
        }};
    }

    check!(vec![0, u8::MAX], U8, U8, u8);
    check!(vec![i8::MIN, i8::MAX], I8, I8, i8);
    check!(vec![0, u16::MAX], U16, U16, u16);
    check!(vec![i16::MIN, i16::MAX], I16, I16, i16);
    check!(vec![0, u32::MAX], U32, U32, u32);
    check!(vec![i32::MIN, i32::MAX], I32, I32, i32);
    check!(vec![0, u64::MAX], U64, U64, u64);
    check!(vec![i64::MIN, i64::MAX], I64, I64, i64);
    check!(vec![-1.5, 2.5], F32, F32, f32);
    check!(vec![-1.5, 2.5], F64, F64, f64);
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Selection {
    Included { sequence: u64 },
    Excluded,
}

#[test]
fn internally_tagged_enums_round_trip_through_their_real_serde_shape() {
    let types = vec![TypeDef {
        owner: owner(),
        id: "Selection".to_owned(),
        name: "Selection".to_owned(),
        docs: None,
        shape: TypeShape::TaggedEnum {
            tag: "kind".to_owned(),
            variants: vec![
                EnumVariantDef {
                    rust_name: "Included".to_owned(),
                    wire_name: "included".to_owned(),
                    docs: None,
                    fields: vec![field(
                        "sequence",
                        TypeRef::Int {
                            signed: false,
                            bits: 64,
                        },
                    )],
                },
                EnumVariantDef {
                    rust_name: "Excluded".to_owned(),
                    wire_name: "excluded".to_owned(),
                    docs: None,
                    fields: vec![],
                },
            ],
        },
    }];
    let ty = named("Selection");
    let value = Selection::Included { sequence: u64::MAX };
    let wire = encode(&value, &ty, &types).unwrap();
    assert_eq!(
        wire,
        WireValue::Object(BTreeMap::from([
            ("kind".to_owned(), WireValue::String("included".to_owned()),),
            ("sequence".to_owned(), WireValue::U64(u64::MAX)),
        ])),
    );
    assert_eq!(decode::<Selection>(wire, &ty, &types).unwrap(), value);
}

#[test]
fn structured_nonfinite_floats_are_rejected_but_buffer_nonfinite_is_allowed() {
    let scalar = encode(&f64::NAN, &TypeRef::Float { bits: 64 }, &[]).unwrap_err();
    assert!(scalar.to_string().contains("finite float"));

    let buffer = encode(
        &vec![f64::NAN, f64::INFINITY],
        &TypeRef::Buffer {
            element: BufferElement::F64,
        },
        &[],
    )
    .unwrap();
    let WireValue::Buffer(BufferValue::F64(values)) = buffer else {
        panic!("expected f64 buffer");
    };
    assert!(values[0].is_nan());
    assert!(values[1].is_infinite());
}

#[test]
fn integer_rust_values_widen_to_float_contracts() {
    assert_eq!(
        encode(&-42_i64, &TypeRef::Float { bits: 64 }, &[]).unwrap(),
        WireValue::F64(-42.0),
    );
    assert_eq!(
        encode(&42_u64, &TypeRef::Float { bits: 32 }, &[]).unwrap(),
        WireValue::F64(42.0),
    );
    assert_eq!(
        decode::<f64>(WireValue::I64(7), &TypeRef::Float { bits: 64 }, &[],).unwrap(),
        7.0,
    );
}

#[test]
fn fixed_byte_arrays_round_trip_and_enforce_the_rust_length() {
    let value = [1_u8, 2, 3, 4];
    let ty = TypeRef::FixedBytes { length: 4 };
    let wire = encode(&value, &ty, &[]).unwrap();
    assert_eq!(wire, WireValue::Bytes(value.to_vec()));
    assert_eq!(decode::<[u8; 4]>(wire, &ty, &[]).unwrap(), value);

    let error = encode(&[1_u8, 2, 3], &ty, &[]).unwrap_err();
    assert!(error.to_string().contains("expected 4-byte array"));
    assert!(error.to_string().contains("received 3 bytes"));

    let error = decode::<Vec<u8>>(WireValue::Bytes(vec![1, 2, 3, 4, 5]), &ty, &[]).unwrap_err();
    assert!(error.to_string().contains("expected 4-byte array"));
    assert!(error.to_string().contains("received 5 bytes"));
}

#[test]
fn large_byte_inputs_decode_without_materializing_one_raw_value_per_byte() {
    let value = vec![0xa5; 8 * 1024 * 1024];
    let decoded = decode::<Vec<u8>>(WireValue::Bytes(value.clone()), &TypeRef::Bytes, &[]).unwrap();
    assert_eq!(decoded, value);
}

#[test]
fn aware_datetimes_round_trip_through_the_datetime_contract() {
    let value = chrono::DateTime::parse_from_rfc3339("2026-07-16T12:30:00-04:00").unwrap();
    let wire = encode(&value, &TypeRef::DateTime, &[]).unwrap();
    assert_eq!(
        wire,
        rspyts::__private::WireValue::String("2026-07-16T12:30:00-04:00".to_owned())
    );
    assert_eq!(
        decode::<chrono::DateTime<chrono::FixedOffset>>(wire, &TypeRef::DateTime, &[]).unwrap(),
        value
    );
}

#[test]
fn rust_outputs_are_checked_against_field_constraints() {
    let types = [TypeDef {
        owner: owner(),
        id: "ConstrainedOutput".to_owned(),
        name: "ConstrainedOutput".to_owned(),
        docs: None,
        shape: TypeShape::Struct {
            fields: vec![FieldDef {
                constraints: FieldConstraints {
                    ge: Some(1),
                    ..Default::default()
                },
                ..field(
                    "quantity",
                    TypeRef::Int {
                        signed: false,
                        bits: 64,
                    },
                )
            }],
        },
    }];
    let error = encode(
        &ConstrainedOutput { quantity: 0 },
        &named("ConstrainedOutput"),
        &types,
    )
    .unwrap_err();
    assert!(error.to_string().contains("$.quantity"));
    assert!(error.to_string().contains(">= 1"));
}
