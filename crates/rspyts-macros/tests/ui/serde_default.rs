use rspyts::bridge;
use serde::{Deserialize, Serialize};

#[bridge(serde)]
#[derive(Serialize, Deserialize)]
pub struct Defaulted {
    #[serde(default)]
    pub value: u32,
}

fn main() {}
