use rspyts::bridge;
use serde::Serialize;

#[bridge(serde)]
#[derive(Serialize)]
pub struct MissingDeserialize {
    pub value: u32,
}

fn main() {}
