use rspyts::bridge;

#[bridge]
pub struct Hidden {
    pub shown: u32,
    secret: u32,
}

fn main() {}
