use rspyts::bridge;

pub struct Thing;

#[bridge]
impl Thing {
    #[bridge(constructor)]
    pub fn create() -> Self {
        Thing
    }

    #[bridge(target = "typescript")]
    pub fn close(&self) {}

    #[bridge(target = "python")]
    pub fn free(&self) {}

    #[bridge(target = "python")]
    pub fn constructor(&self) {}

    #[bridge(target = "typescript")]
    pub fn __init__(&self) {}
}

fn main() {}
