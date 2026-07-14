use rspyts::bridge;
use serde::{Deserialize, Serialize};

mod codec {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &u32, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u32(*value)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u32, D::Error>
    where
        D: Deserializer<'de>,
    {
        u32::deserialize(deserializer)
    }
}

#[bridge(serde)]
#[derive(Serialize, Deserialize)]
pub struct CustomCodec {
    #[serde(with = "codec")]
    pub value: u32,
}

fn main() {}
