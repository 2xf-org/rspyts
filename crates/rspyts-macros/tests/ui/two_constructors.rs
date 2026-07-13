use rspyts::bridge;

pub struct Twice;

#[bridge]
impl Twice {
    #[bridge(constructor)]
    pub fn first() -> Self {
        Twice
    }

    #[bridge(constructor)]
    pub fn second() -> Self {
        Twice
    }
}

fn main() {}
