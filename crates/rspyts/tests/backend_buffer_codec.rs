use rspyts::__private::{BufferDtype, BufferValue};
use rspyts::backend::{decode_buffer_bytes, encode_buffer_bytes};

#[test]
fn every_numeric_buffer_round_trips_through_canonical_bytes() {
    let cases = [
        BufferValue::U8(vec![0, 1, u8::MAX]),
        BufferValue::I8(vec![i8::MIN, -1, i8::MAX]),
        BufferValue::U16(vec![0, 1, u16::MAX]),
        BufferValue::I16(vec![i16::MIN, -1, i16::MAX]),
        BufferValue::U32(vec![0, 1, u32::MAX]),
        BufferValue::I32(vec![i32::MIN, -1, i32::MAX]),
        BufferValue::U64(vec![0, 1, u64::MAX]),
        BufferValue::I64(vec![i64::MIN, -1, i64::MAX]),
        BufferValue::F32(vec![f32::NEG_INFINITY, -0.0, f32::NAN, f32::INFINITY]),
        BufferValue::F64(vec![f64::NEG_INFINITY, -0.0, f64::NAN, f64::INFINITY]),
    ];

    for case in cases {
        let bytes = encode_buffer_bytes(&case);
        let decoded = decode_buffer_bytes(case.dtype(), case.len(), &bytes).unwrap();
        match (&case, &decoded) {
            (BufferValue::F32(left), BufferValue::F32(right)) => assert_eq!(
                left.iter().map(|value| value.to_bits()).collect::<Vec<_>>(),
                right
                    .iter()
                    .map(|value| value.to_bits())
                    .collect::<Vec<_>>()
            ),
            (BufferValue::F64(left), BufferValue::F64(right)) => assert_eq!(
                left.iter().map(|value| value.to_bits()).collect::<Vec<_>>(),
                right
                    .iter()
                    .map(|value| value.to_bits())
                    .collect::<Vec<_>>()
            ),
            _ => assert_eq!(case, decoded),
        }
    }
}

#[test]
fn malformed_buffer_lengths_are_rejected() {
    let error = decode_buffer_bytes(BufferDtype::F64, 2, &[0; 8]).unwrap_err();
    assert!(error.to_string().contains("requires 16 bytes"));
}
