use rspyts::bridge;

#[bridge]
pub struct PresenceRecord {
    pub omittable: Option<String>,
    #[bridge(required)]
    pub required_nullable: Option<String>,
    pub unavailable: (),
}

#[bridge]
#[allow(unused_variables)]
pub fn echo_presence(value: Option<String>, unavailable: ()) -> Option<String> {
    value
}

#[bridge]
pub fn echo_presence_record(value: PresenceRecord) -> PresenceRecord {
    value
}

#[test]
fn macro_keeps_required_option_distinct_from_omittable_option() {
    let manifest = rspyts::__private::registry::build_manifest("rspyts", "0.0.0");
    let declaration = manifest
        .types
        .iter()
        .find(|declaration| declaration.name() == "PresenceRecord")
        .expect("bridged type is registered");
    let rspyts::__private::ir::TypeDecl::Struct { fields, .. } = declaration else {
        panic!("PresenceRecord should be a struct");
    };

    assert!(!fields[0].required);
    assert!(matches!(
        fields[0].ty,
        rspyts::__private::ir::Ty::Option { .. }
    ));
    assert!(fields[1].required);
    assert!(matches!(
        fields[1].ty,
        rspyts::__private::ir::Ty::Option { .. }
    ));
    assert!(fields[2].required);
    assert_eq!(fields[2].ty, rspyts::__private::ir::Ty::Null);
}

#[test]
fn ordinary_serde_keeps_native_option_semantics() {
    let explicit: PresenceRecord =
        serde_json::from_str(r#"{"requiredNullable":null,"unavailable":null}"#).unwrap();
    assert_eq!(explicit.omittable, None);
    assert_eq!(explicit.required_nullable, None);
    assert_eq!(explicit.unavailable, ());

    let value: PresenceRecord =
        serde_json::from_str(r#"{"requiredNullable":"set","unavailable":null}"#).unwrap();
    assert_eq!(value.required_nullable, Some("set".to_string()));

    let omitted: PresenceRecord = serde_json::from_str(r#"{"unavailable":null}"#).unwrap();
    assert_eq!(omitted.required_nullable, None);
    assert!(serde_json::from_str::<PresenceRecord>(r#"{"requiredNullable":null}"#).is_err());
    assert!(
        serde_json::from_str::<PresenceRecord>(r#"{"requiredNullable":null,"unavailable":false}"#,)
            .is_err()
    );
}

#[test]
fn generated_struct_shim_distinguishes_missing_from_explicit_null() {
    fn call(value: serde_json::Value) -> (u8, serde_json::Value) {
        let json = serde_json::to_vec(&value).unwrap();
        let request = rspyts::__private::envelope::encode_request(&json, &[]).unwrap();
        let pointer = unsafe { rspyts_fn__echo_presence_record(request.as_ptr(), request.len()) };
        let decoded = unsafe { rspyts::__private::envelope::decode(pointer) };
        let status = decoded.status;
        let value = serde_json::from_slice(decoded.json).unwrap();
        assert!(decoded.tail.is_empty());
        let total = unsafe { rspyts::__private::envelope::total_len(pointer) };
        unsafe { rspyts::__private::envelope::dealloc(pointer, total) };
        (status, value)
    }

    let (status, value) = call(serde_json::json!({
        "value": {"requiredNullable": null, "unavailable": null},
    }));
    assert_eq!(status, rspyts::__private::envelope::STATUS_OK);
    assert_eq!(
        value,
        serde_json::json!({
            "omittable": null,
            "requiredNullable": null,
            "unavailable": null,
        })
    );

    let (status, value) = call(serde_json::json!({
        "value": {"unavailable": null},
    }));
    assert_eq!(status, rspyts::__private::envelope::STATUS_ERROR);
    assert_eq!(value["code"], "invalidArgs");
}

#[test]
fn generated_function_shim_rejects_omission_and_non_null_null_values() {
    fn call(value: serde_json::Value) -> (u8, serde_json::Value) {
        let json = serde_json::to_vec(&value).unwrap();
        let request = rspyts::__private::envelope::encode_request(&json, &[]).unwrap();
        let pointer = unsafe { rspyts_fn__echo_presence(request.as_ptr(), request.len()) };
        let decoded = unsafe { rspyts::__private::envelope::decode(pointer) };
        let status = decoded.status;
        let value = serde_json::from_slice(decoded.json).unwrap();
        assert!(decoded.tail.is_empty());
        let total = unsafe { rspyts::__private::envelope::total_len(pointer) };
        unsafe { rspyts::__private::envelope::dealloc(pointer, total) };
        (status, value)
    }

    let (status, value) = call(serde_json::json!({"value": null, "unavailable": null}));
    assert_eq!(status, rspyts::__private::envelope::STATUS_OK);
    assert_eq!(value, serde_json::Value::Null);

    for invalid in [
        serde_json::json!({"unavailable": null}),
        serde_json::json!({"value": null}),
        serde_json::json!({"value": null, "unavailable": false}),
    ] {
        let (status, value) = call(invalid);
        assert_eq!(status, rspyts::__private::envelope::STATUS_ERROR);
        assert_eq!(value["code"], "invalidArgs");
    }
}

#[test]
fn ordinary_option_serde_is_unchanged_outside_the_bridge() {
    assert_eq!(
        serde_json::to_value(PresenceRecord {
            omittable: None,
            required_nullable: None,
            unavailable: (),
        })
        .unwrap(),
        serde_json::json!({
            "omittable": null,
            "requiredNullable": null,
            "unavailable": null,
        })
    );
}
