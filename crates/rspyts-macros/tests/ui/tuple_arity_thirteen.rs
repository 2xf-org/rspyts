use rspyts::bridge;

#[bridge]
pub struct TooManyTupleItems {
    pub values: (u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8),
}

fn main() {}
