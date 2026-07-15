use rspyts::bridge;

#[bridge(serde)]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrototypeHazard {
    #[serde(rename = "__proto__")]
    pub value: String,
}

fn main() {}
