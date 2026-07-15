use rspyts::bridge;

pub struct Thing;

#[bridge]
impl Thing {
    #[bridge(constructor)]
    pub fn create() -> Self {
        Thing
    }

    pub fn read_value(&self) {}

    pub fn read__value(&self) {}
}

fn main() {}
