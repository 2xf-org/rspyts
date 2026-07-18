use rspyts::ir::{IR_VERSION, TypeRef};

#[test]
fn fixed_bytes_have_an_exact_versioned_ir_shape() {
    let ty = TypeRef::FixedBytes { length: 4 };
    let json = serde_json::to_value(&ty).unwrap();

    assert_eq!(
        json,
        serde_json::json!({
            "kind": "fixedBytes",
            "length": 4,
        })
    );
    assert_eq!(serde_json::from_value::<TypeRef>(json).unwrap(), ty);
    assert_eq!(IR_VERSION, 6);
}

#[test]
fn dynamic_and_fixed_bytes_remain_distinct_in_nested_ir() {
    let fixed = TypeRef::List {
        item: Box::new(TypeRef::FixedBytes { length: 8 }),
    };
    let dynamic = TypeRef::List {
        item: Box::new(TypeRef::Bytes),
    };

    assert_ne!(fixed, dynamic);
    assert_eq!(
        serde_json::from_value::<TypeRef>(serde_json::to_value(&fixed).unwrap()).unwrap(),
        TypeRef::List {
            item: Box::new(TypeRef::FixedBytes { length: 8 }),
        }
    );
}
