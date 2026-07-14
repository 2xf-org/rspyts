use rspyts::bridge;
use serde::{Deserialize, Serialize};

#[bridge(serde)]
#[derive(Serialize, Deserialize)]
pub struct Skipped {
    #[serde(skip)]
    pub hidden: u32,
}

fn main() {}
