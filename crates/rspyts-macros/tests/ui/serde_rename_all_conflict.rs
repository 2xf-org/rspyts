use rspyts::bridge;
use serde::{Deserialize, Serialize};

#[bridge(serde, rename_all = "camelCase")]
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ConflictingCase {
    pub item_count: u32,
}

fn main() {}
