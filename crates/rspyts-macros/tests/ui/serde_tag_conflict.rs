use rspyts::bridge;
use serde::{Deserialize, Serialize};

#[bridge(serde, tag = "kind")]
#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ConflictingTag {
    Ready { value: u32 },
}

fn main() {}
