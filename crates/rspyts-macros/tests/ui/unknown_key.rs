use rspyts::bridge;

#[bridge(frobnicate)]
pub struct Config {
    pub value: u32,
}

fn main() {}
