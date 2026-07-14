use rspyts::bridge;
use serde::{Deserialize, Serialize};

#[bridge(serde)]
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum Untagged {
    Number { value: u32 },
    Text { value: String },
}

fn main() {}
