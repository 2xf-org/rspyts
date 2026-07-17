#[derive(rspyts::Type, serde::Serialize)]
pub struct Settings {
    pub ratio: Option<f64>,
}

#[rspyts::export]
pub const SETTINGS: Settings = Settings {
    ratio: Some(f64::NAN),
};

#[test]
fn option_some_nan_is_rejected_before_json_can_erase_it_to_null() {
    let error = rspyts::registry::manifest("rspyts", "0.4.2", "native").unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("constant `SETTINGS` does not match its declared type")
            && message.contains("finite float"),
        "unexpected error: {message}"
    );
}
