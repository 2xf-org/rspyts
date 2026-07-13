use rspyts::bridge;

#[bridge]
pub enum Mixed {
    Data { value: u32 },
    Fieldless,
}

fn main() {}
