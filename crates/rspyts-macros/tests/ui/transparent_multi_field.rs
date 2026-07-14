use rspyts::bridge;
use serde::{Deserialize, Serialize};

#[bridge(serde)]
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub struct InvalidTransparent {
    pub left: u32,
    pub right: u32,
}

fn main() {}
