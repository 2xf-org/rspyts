#![cfg(all(feature = "python", not(target_arch = "wasm32")))]

use std::collections::BTreeMap;

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyInt, PyModule};
use rspyts::__private::{BufferValue, WireValue};
use rspyts::backend::python::{decode, decode_typed, encode, encode_typed, register};
use rspyts::ir::{BufferElement, TypeRef};

fn with_python(test: impl for<'py> FnOnce(Python<'py>)) {
    Python::initialize();
    Python::attach(test);
}

#[test]
fn exact_integer_extremes_remain_python_ints() {
    with_python(|py| {
        let signed = encode_typed(
            py,
            &WireValue::I64(i64::MIN),
            &TypeRef::Int {
                signed: true,
                bits: 64,
            },
            &[],
        )
        .unwrap();
        let unsigned = encode_typed(
            py,
            &WireValue::U64(u64::MAX),
            &TypeRef::Int {
                signed: false,
                bits: 64,
            },
            &[],
        )
        .unwrap();
        assert_eq!(signed.bind(py).extract::<i64>().unwrap(), i64::MIN);
        assert_eq!(unsigned.bind(py).extract::<u64>().unwrap(), u64::MAX);
    });
}

#[test]
fn every_numeric_buffer_round_trips_through_the_private_native_class() {
    with_python(|py| {
        let cases = [
            (BufferValue::U8(vec![0, u8::MAX]), BufferElement::U8),
            (BufferValue::I8(vec![i8::MIN, i8::MAX]), BufferElement::I8),
            (BufferValue::U16(vec![0, u16::MAX]), BufferElement::U16),
            (
                BufferValue::I16(vec![i16::MIN, i16::MAX]),
                BufferElement::I16,
            ),
            (BufferValue::U32(vec![0, u32::MAX]), BufferElement::U32),
            (
                BufferValue::I32(vec![i32::MIN, i32::MAX]),
                BufferElement::I32,
            ),
            (BufferValue::U64(vec![0, u64::MAX]), BufferElement::U64),
            (
                BufferValue::I64(vec![i64::MIN, i64::MAX]),
                BufferElement::I64,
            ),
            (BufferValue::F32(vec![-1.5, 2.5]), BufferElement::F32),
            (BufferValue::F64(vec![-1.5, 2.5]), BufferElement::F64),
        ];

        for (buffer, element) in cases {
            let value = WireValue::Buffer(buffer);
            let host = encode_typed(py, &value, &TypeRef::Buffer { element }, &[]).unwrap();
            assert_eq!(
                decode_typed(host.bind(py), &TypeRef::Buffer { element }, &[]).unwrap(),
                value,
            );
            assert!(
                host.bind(py)
                    .getattr("data")
                    .unwrap()
                    .cast::<PyBytes>()
                    .is_ok()
            );
        }
    });
}

#[test]
fn ordinary_dicts_cannot_be_confused_with_numeric_buffers() {
    with_python(|py| {
        let dict = PyDict::new(py);
        dict.set_item("rspytsBuffer", true).unwrap();
        assert_eq!(
            decode(dict.as_any()).unwrap(),
            WireValue::Object(BTreeMap::from([(
                "rspytsBuffer".to_owned(),
                WireValue::Bool(true),
            )])),
        );
    });
}

#[test]
fn registered_payload_constructor_rejects_malformed_widths() {
    with_python(|py| {
        let module = PyModule::new(py, "rspyts_native").unwrap();
        register(&module).unwrap();
        let class = module.getattr("BufferPayload").unwrap();
        let data = PyBytes::new(py, &[1, 2, 3]);
        assert!(class.call1(("f64", data)).is_err());
    });
}

#[test]
fn recursive_schema_canonicalizes_nested_integer_signedness() {
    with_python(|py| {
        let host = encode(py, &WireValue::Sequence(vec![WireValue::I64(7)])).unwrap();
        let decoded = decode_typed(
            host.bind(py),
            &TypeRef::List {
                item: Box::new(TypeRef::Int {
                    signed: false,
                    bits: 64,
                }),
            },
            &[],
        )
        .unwrap();
        assert_eq!(decoded, WireValue::Sequence(vec![WireValue::U64(7)]));
    });
}

#[test]
fn python_ints_are_accepted_for_float_contracts() {
    with_python(|py| {
        let value = PyInt::new(py, 42);
        assert_eq!(
            decode_typed(value.as_any(), &TypeRef::Float { bits: 64 }, &[]).unwrap(),
            WireValue::F64(42.0),
        );
        assert_eq!(
            decode_typed(value.as_any(), &TypeRef::Float { bits: 32 }, &[]).unwrap(),
            WireValue::F64(42.0),
        );
    });
}
