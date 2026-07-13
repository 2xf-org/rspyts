use rspyts::bridge;

#[bridge]
pub enum Pair {
    Both(u32, u32),
}

fn main() {}
