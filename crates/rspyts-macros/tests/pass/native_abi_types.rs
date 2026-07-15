use rspyts::bridge;

type JsonAlias = serde_json::Value;

#[bridge]
pub const MIN_SIGNED: i64 = i64::MIN;

#[bridge]
pub const MAX_UNSIGNED: u64 = u64::MAX;

#[bridge]
pub const NULL_JSON: serde_json::Value = serde_json::Value::Null;

#[bridge]
pub struct NativeSigned(pub i64);

#[bridge]
pub struct NativeValues {
    pub signed: i64,
    pub unsigned: u64,
    pub json: serde_json::Value,
    #[serde(rename = "__rspyts_buf__")]
    pub legacy_buffer_key: u32,
    #[serde(rename = "__rspyts_json__")]
    pub legacy_json_key: u32,
    pub maybe_signed: Option<i64>,
    #[bridge(required)]
    pub required_json: Option<serde_json::Value>,
}

#[bridge]
pub enum NativeEvent {
    Values {
        signed: i64,
        unsigned: u64,
        json: serde_json::Value,
        #[bridge(required)]
        required_signed: Option<i64>,
    },
}

#[derive(Debug)]
#[bridge(error)]
pub enum NativeError {
    Invalid {
        signed: i64,
        unsigned: u64,
        json: serde_json::Value,
        #[bridge(required)]
        context: Option<serde_json::Value>,
    },
}

impl std::fmt::Display for NativeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid native value")
    }
}

pub struct NativeCounter(i64);

#[bridge]
impl NativeCounter {
    #[bridge(constructor)]
    pub fn create(value: i64) -> Self {
        Self(value)
    }

    pub fn add(&mut self, value: u64) -> i64 {
        self.0 = self.0.saturating_add_unsigned(value);
        self.0
    }

    pub fn json(&self, value: serde_json::Value) -> serde_json::Value {
        value
    }
}

#[bridge]
pub fn reflect(
    value: serde_json::Value,
    signed: i64,
    unsigned: u64,
) -> Result<serde_json::Value, NativeError> {
    Ok(serde_json::json!({"value": value, "signed": signed, "unsigned": unsigned}))
}

#[bridge]
pub fn aliased_return() -> JsonAlias {
    serde_json::Value::Null
}

#[bridge]
pub fn unit_return() {}

fn main() {}
