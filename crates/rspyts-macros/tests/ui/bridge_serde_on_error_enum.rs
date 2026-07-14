use rspyts::bridge;
use serde::{Deserialize, Serialize};

#[bridge(error, serde)]
#[derive(Debug, Serialize, Deserialize)]
pub enum AdoptedError {
    Failed,
}

fn main() {}
