use std::collections::BTreeMap;

use rspyts::__private::{BoundaryError, BufferDtype, BufferValue, WireValue};
use serde_json::{Value, json};

#[test]
fn serde_json_values_map_to_the_exact_wire_vocabulary() {
    let value = json!({
        "null": null,
        "bool": true,
        "negative": -4,
        "positive": 4,
        "float": 1.25,
        "string": "signal",
        "sequence": [1, "two"]
    });

    let wire = WireValue::try_from(value).unwrap();

    let WireValue::Object(fields) = wire else {
        panic!("expected an object");
    };
    assert_eq!(fields["null"], WireValue::Null);
    assert_eq!(fields["bool"], WireValue::Bool(true));
    assert_eq!(fields["negative"], WireValue::I64(-4));
    assert_eq!(fields["positive"], WireValue::U64(4));
    assert_eq!(fields["float"], WireValue::F64(1.25));
    assert_eq!(fields["string"], WireValue::String("signal".into()));
    assert_eq!(
        fields["sequence"],
        WireValue::Sequence(vec![WireValue::U64(1), WireValue::String("two".into())])
    );
}

#[test]
fn json_conversion_preserves_integer_extremes() {
    for wire in [WireValue::I64(i64::MIN), WireValue::U64(u64::MAX)] {
        let json = Value::try_from(&wire).unwrap();
        let round_trip = WireValue::try_from(json).unwrap();
        assert_eq!(round_trip, wire);
    }
}

#[test]
fn positive_json_integers_have_a_canonical_unsigned_representation() {
    let json = Value::try_from(WireValue::I64(42)).unwrap();
    assert_eq!(WireValue::try_from(json).unwrap(), WireValue::U64(42));
}

#[test]
fn json_conversion_preserves_nested_structured_values() {
    let wire = WireValue::Object(BTreeMap::from([
        ("enabled".into(), WireValue::Bool(true)),
        (
            "items".into(),
            WireValue::Sequence(vec![WireValue::Null, WireValue::F64(2.5)]),
        ),
    ]));

    let json = Value::try_from(&wire).unwrap();
    assert_eq!(json, json!({"enabled": true, "items": [null, 2.5]}));
    assert_eq!(WireValue::try_from(json).unwrap(), wire);
}

#[test]
fn non_finite_structured_floats_are_boundary_errors() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let wire = WireValue::Sequence(vec![WireValue::F64(value)]);
        assert_eq!(
            wire.validate(),
            Err(BoundaryError::NonFiniteStructuredFloat)
        );
        assert_eq!(
            Value::try_from(wire),
            Err(BoundaryError::NonFiniteStructuredFloat)
        );
    }
}

#[test]
fn bytes_and_numeric_buffers_never_degrade_into_json_arrays() {
    assert_eq!(
        Value::try_from(WireValue::Bytes(vec![1, 2, 3])),
        Err(BoundaryError::NotJsonRepresentable { kind: "bytes" })
    );
    assert_eq!(
        Value::try_from(WireValue::Buffer(BufferValue::U8(vec![1, 2, 3]))),
        Err(BoundaryError::NotJsonRepresentable {
            kind: "numeric buffer"
        })
    );

    let nested = WireValue::Sequence(vec![WireValue::Bytes(vec![1])]);
    assert_eq!(
        Value::try_from(nested),
        Err(BoundaryError::NotJsonRepresentable { kind: "bytes" })
    );
}

#[test]
fn every_numeric_buffer_reports_backend_neutral_metadata() {
    let buffers = [
        BufferValue::U8(vec![1, 2]),
        BufferValue::I8(vec![1, 2]),
        BufferValue::U16(vec![1, 2]),
        BufferValue::I16(vec![1, 2]),
        BufferValue::U32(vec![1, 2]),
        BufferValue::I32(vec![1, 2]),
        BufferValue::U64(vec![1, 2]),
        BufferValue::I64(vec![1, 2]),
        BufferValue::F32(vec![1.0, 2.0]),
        BufferValue::F64(vec![1.0, 2.0]),
    ];
    let expected = [
        (BufferDtype::U8, "u8", 8, Some(false)),
        (BufferDtype::I8, "i8", 8, Some(true)),
        (BufferDtype::U16, "u16", 16, Some(false)),
        (BufferDtype::I16, "i16", 16, Some(true)),
        (BufferDtype::U32, "u32", 32, Some(false)),
        (BufferDtype::I32, "i32", 32, Some(true)),
        (BufferDtype::U64, "u64", 64, Some(false)),
        (BufferDtype::I64, "i64", 64, Some(true)),
        (BufferDtype::F32, "f32", 32, None),
        (BufferDtype::F64, "f64", 64, None),
    ];

    for (buffer, (dtype, name, bits, signed)) in buffers.iter().zip(expected) {
        assert_eq!(buffer.dtype(), dtype);
        assert_eq!(buffer.len(), 2);
        assert!(!buffer.is_empty());
        assert_eq!(buffer.byte_len(), 2 * usize::from(bits) / 8);
        assert_eq!(dtype.name(), name);
        assert_eq!(dtype.bits(), bits);
        assert_eq!(dtype.is_signed(), signed);
        assert_eq!(dtype.is_integer(), signed.is_some());
        assert_eq!(dtype.is_float(), signed.is_none());
    }
}

#[test]
fn numeric_float_buffers_preserve_all_ieee_values() {
    let buffer = BufferValue::F64(vec![f64::NAN, f64::INFINITY, f64::NEG_INFINITY]);
    let wire = WireValue::Buffer(buffer.clone());

    assert!(wire.validate().is_ok());
    let BufferValue::F64(values) = buffer else {
        unreachable!();
    };
    assert!(values[0].is_nan());
    assert_eq!(values[1], f64::INFINITY);
    assert_eq!(values[2], f64::NEG_INFINITY);
}

#[test]
fn vector_conversions_retain_dtype_instead_of_guessing() {
    assert_eq!(
        BufferValue::from(vec![1_u8, 2]),
        BufferValue::U8(vec![1, 2])
    );
    assert_eq!(
        BufferValue::from(vec![1_i64, 2]),
        BufferValue::I64(vec![1, 2])
    );
    assert_eq!(
        BufferValue::from(vec![1.0_f32, 2.0]),
        BufferValue::F32(vec![1.0, 2.0])
    );
}
