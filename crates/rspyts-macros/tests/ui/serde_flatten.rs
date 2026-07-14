use rspyts::bridge;
use serde::{Deserialize, Serialize};

#[bridge(serde)]
#[derive(Serialize, Deserialize)]
pub struct Flattened {
    #[serde(flatten)]
    pub value: std::collections::BTreeMap<String, u32>,
}

fn main() {}
