use rspyts::bridge;

#[bridge(target = "python")]
pub struct Config {
    pub value: u32,
}

fn main() {}
