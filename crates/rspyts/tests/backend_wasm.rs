#![cfg(target_arch = "wasm32")]

use std::collections::BTreeMap;

use js_sys::{
    BigInt64Array, BigUint64Array, Date, Error, Float32Array, Float64Array, Int8Array, Int16Array,
    Int32Array, Object, Reflect, Uint8Array, Uint16Array, Uint32Array,
};
use rspyts::__private::{BufferValue, WireValue};
use rspyts::backend::typescript::{
    decode, decode_json, decode_typed, encode, encode_json, encode_typed,
};
use rspyts::ir::{
    BufferElement, CargoPackageId, DefinitionId, FieldConstraints, FieldDef, ScalarValue, TypeDef,
    TypeRef, TypeShape,
};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_test::*;

#[wasm_bindgen_test]
fn typed_contract_recurses_through_numbers_bytes_buffers_and_json() {
    let ty = TypeRef::Tuple {
        items: vec![
            TypeRef::Int {
                signed: true,
                bits: 32,
            },
            TypeRef::Int {
                signed: false,
                bits: 64,
            },
            TypeRef::Bytes,
            TypeRef::Buffer {
                element: BufferElement::F64,
            },
            TypeRef::Json,
        ],
    };
    let value = WireValue::Sequence(vec![
        WireValue::I64(-42),
        WireValue::U64(u64::MAX),
        WireValue::Bytes(vec![1, 2, 3]),
        WireValue::Buffer(BufferValue::F64(vec![1.5, 2.5])),
        WireValue::Object(BTreeMap::from([(
            "safeInteger".to_owned(),
            WireValue::U64(9_007_199_254_740_991),
        )])),
    ]);

    let host = encode_typed(&value, &ty, &[]).unwrap();
    let decoded = decode_typed(&host, &ty, &[]).unwrap();
    assert_eq!(decoded, value);
}

#[wasm_bindgen_test]
fn integer_values_are_accepted_for_float_contracts() {
    let host = encode_typed(&WireValue::I64(-42), &TypeRef::Float { bits: 64 }, &[]).unwrap();
    assert_eq!(host.as_f64(), Some(-42.0));
    assert_eq!(
        decode_typed(&JsValue::from_f64(42.0), &TypeRef::Float { bits: 32 }, &[],).unwrap(),
        WireValue::F64(42.0),
    );
    assert!(
        decode_typed(
            &JsValue::from_f64(f64::INFINITY),
            &TypeRef::Float { bits: 64 },
            &[],
        )
        .is_err()
    );
}

#[wasm_bindgen_test]
fn bytes_and_u8_buffers_share_a_host_view_but_not_contract_semantics() {
    let bytes: JsValue = Uint8Array::from(&[1, 2, 3][..]).into();
    assert_eq!(
        decode_typed(&bytes, &TypeRef::Bytes, &[]).unwrap(),
        WireValue::Bytes(vec![1, 2, 3]),
    );
    assert_eq!(
        decode_typed(
            &bytes,
            &TypeRef::Buffer {
                element: BufferElement::U8,
            },
            &[],
        )
        .unwrap(),
        WireValue::Buffer(BufferValue::U8(vec![1, 2, 3])),
    );
}

#[wasm_bindgen_test]
fn typed_arrays_obey_fixed_byte_contract_lengths() {
    let ty = TypeRef::FixedBytes { length: 4 };
    let exact: JsValue = Uint8Array::from(&[1, 2, 3, 4][..]).into();
    assert_eq!(
        decode_typed(&exact, &ty, &[]).unwrap(),
        WireValue::Bytes(vec![1, 2, 3, 4]),
    );

    for invalid in [
        Uint8Array::from(&[1, 2, 3][..]),
        Uint8Array::from(&[1, 2, 3, 4, 5][..]),
    ] {
        let error = decode_typed(invalid.as_ref(), &ty, &[]).unwrap_err();
        assert!(error.to_string().contains("expected 4-byte array"));
    }
}

#[wasm_bindgen_test]
fn every_numeric_buffer_uses_the_matching_owned_typed_array() {
    macro_rules! check {
        ($buffer:expr, $array:ty, $element:ident) => {{
            let value = WireValue::Buffer($buffer);
            let host = encode(&value).unwrap();
            assert!(host.is_instance_of::<$array>());
            assert_eq!(
                decode_typed(
                    &host,
                    &TypeRef::Buffer {
                        element: BufferElement::$element,
                    },
                    &[],
                )
                .unwrap(),
                value
            );
        }};
    }

    check!(BufferValue::U8(vec![0, u8::MAX]), Uint8Array, U8);
    check!(BufferValue::I8(vec![i8::MIN, i8::MAX]), Int8Array, I8);
    check!(BufferValue::U16(vec![0, u16::MAX]), Uint16Array, U16);
    check!(BufferValue::I16(vec![i16::MIN, i16::MAX]), Int16Array, I16);
    check!(BufferValue::U32(vec![0, u32::MAX]), Uint32Array, U32);
    check!(BufferValue::I32(vec![i32::MIN, i32::MAX]), Int32Array, I32);
    check!(BufferValue::U64(vec![0, u64::MAX]), BigUint64Array, U64);
    check!(
        BufferValue::I64(vec![i64::MIN, i64::MAX]),
        BigInt64Array,
        I64
    );
    check!(BufferValue::F32(vec![-1.5, 2.5]), Float32Array, F32);
    check!(BufferValue::F64(vec![-1.5, 2.5]), Float64Array, F64);

    let original = WireValue::Buffer(BufferValue::F64(vec![1.0]));
    let host = encode(&original).unwrap().unchecked_into::<Float64Array>();
    host.set_index(0, 99.0);
    assert_eq!(original, WireValue::Buffer(BufferValue::F64(vec![1.0])));
}

#[wasm_bindgen_test]
fn json_is_stringifiable_and_rejects_unsafe_integer_precision() {
    let json = WireValue::Object(BTreeMap::from([(
        "count".to_owned(),
        WireValue::U64(9_007_199_254_740_991),
    )]));
    let host = encode_json(&json).unwrap();
    assert!(js_sys::JSON::stringify(&host).is_ok());
    assert_eq!(decode_json(&host).unwrap(), json);

    for value in [
        WireValue::I64(-9_007_199_254_740_992),
        WireValue::U64(9_007_199_254_740_992),
        WireValue::F64(9_007_199_254_740_992.0),
        WireValue::F64(f64::INFINITY),
    ] {
        let nested = WireValue::Object(BTreeMap::from([(
            "items".to_owned(),
            WireValue::Sequence(vec![WireValue::Null, value]),
        )]));
        assert!(encode_json(&nested).is_err());
    }
    assert!(decode_json(&js_sys::BigInt::from(1_u32).into()).is_err());

    let nested = js_sys::Object::new();
    let items = js_sys::Array::of2(&JsValue::NULL, &JsValue::from_f64(9_007_199_254_740_992.0));
    Reflect::set(&nested, &JsValue::from_str("items"), &items).unwrap();
    assert!(decode_json(&nested.into()).is_err());
}

#[wasm_bindgen_test]
fn proto_is_an_own_key_and_class_instances_are_rejected() {
    let encoded = encode(&WireValue::Object(BTreeMap::from([(
        "__proto__".to_owned(),
        WireValue::String("kept".to_owned()),
    )])))
    .unwrap();
    assert_eq!(
        Reflect::get(&encoded, &JsValue::from_str("__proto__"))
            .unwrap()
            .as_string()
            .as_deref(),
        Some("kept"),
    );
    assert!(decode(&Date::new_0().into()).is_err());
}

#[wasm_bindgen_test]
fn boundary_errors_are_javascript_error_objects() {
    let error: JsValue = rspyts::backend::BackendError::NonFiniteStructuredFloat.into();
    assert!(error.is_instance_of::<Error>());
}

#[wasm_bindgen_test]
fn typed_contract_applies_defaults_and_constraints() {
    let types = [TypeDef {
        owner: CargoPackageId::new("wasm-tests"),
        id: "Request".to_owned(),
        name: "Request".to_owned(),
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
    let ty = TypeRef::Named {
        identity: DefinitionId::new("wasm-tests", "Request"),
    };
    let empty = Object::new();
    let decoded = decode_typed(empty.as_ref(), &ty, &types).unwrap();
    assert_eq!(
        decoded,
        WireValue::Object(BTreeMap::from([
            ("quantity".to_owned(), WireValue::U64(1),)
        ]))
    );

    let invalid = Object::new();
    Reflect::set(
        &invalid,
        &JsValue::from_str("quantity"),
        &js_sys::BigInt::from(0_u32),
    )
    .unwrap();
    let error = decode_typed(invalid.as_ref(), &ty, &types).unwrap_err();
    assert!(error.to_string().contains("$.quantity"));
}

#[wasm_bindgen_test]
fn optional_undefined_members_are_absent_but_required_members_still_reject() {
    let types = [TypeDef {
        owner: CargoPackageId::new("wasm-tests"),
        id: "Selection".to_owned(),
        name: "Selection".to_owned(),
        docs: None,
        shape: TypeShape::Struct {
            fields: vec![
                FieldDef {
                    rust_name: "label".to_owned(),
                    wire_name: "label".to_owned(),
                    docs: None,
                    ty: TypeRef::String,
                    required: true,
                    default: None,
                    constraints: FieldConstraints::default(),
                },
                FieldDef {
                    rust_name: "weight".to_owned(),
                    wire_name: "weight".to_owned(),
                    docs: None,
                    ty: TypeRef::Option {
                        item: Box::new(TypeRef::Float { bits: 64 }),
                    },
                    required: false,
                    default: None,
                    constraints: FieldConstraints::default(),
                },
            ],
        },
    }];
    let ty = TypeRef::Named {
        identity: DefinitionId::new("wasm-tests", "Selection"),
    };

    let optional_undefined = Object::new();
    Reflect::set(
        &optional_undefined,
        &JsValue::from_str("label"),
        &JsValue::from_str("primary"),
    )
    .unwrap();
    Reflect::set(
        &optional_undefined,
        &JsValue::from_str("weight"),
        &JsValue::UNDEFINED,
    )
    .unwrap();
    assert_eq!(
        decode_typed(optional_undefined.as_ref(), &ty, &types).unwrap(),
        WireValue::Object(BTreeMap::from([(
            "label".to_owned(),
            WireValue::String("primary".to_owned()),
        )])),
    );

    let required_undefined = Object::new();
    Reflect::set(
        &required_undefined,
        &JsValue::from_str("label"),
        &JsValue::UNDEFINED,
    )
    .unwrap();
    assert!(decode_typed(required_undefined.as_ref(), &ty, &types).is_err());
}
