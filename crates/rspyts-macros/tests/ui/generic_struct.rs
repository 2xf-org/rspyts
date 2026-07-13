use rspyts::bridge;

#[bridge]
pub struct Wrapper<T> {
    pub value: T,
}

fn main() {}
