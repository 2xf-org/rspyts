use rspyts::ir::{FieldConstraints, TypeRef, TypeShape};

#[test]
fn the_ir_is_small_and_host_neutral() {
    let field = rspyts::ir::FieldDef {
        rust_name: "count".into(),
        wire_name: "count".into(),
        docs: None,
        ty: TypeRef::Int {
            signed: false,
            bits: 32,
        },
        required: true,
        default: None,
        constraints: FieldConstraints {
            ge: Some(1),
            ..FieldConstraints::default()
        },
    };
    assert!(matches!(
        TypeShape::Struct {
            fields: vec![field]
        },
        TypeShape::Struct { .. }
    ));
}
