use rspyts::bridge;

pub struct Thing;

#[bridge]
impl Thing {
    #[bridge(constructor)]
    pub fn create() -> Self {
        Thing
    }

    #[bridge(static)]
    pub fn prototype() -> u32 {
        0
    }
}

fn main() {}
