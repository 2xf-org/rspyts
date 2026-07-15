use rspyts::bridge;

pub struct Thing;

#[bridge]
impl Thing {
    #[bridge(constructor)]
    pub fn create() -> Self {
        Thing
    }

    pub fn dispose(&self) {}
}

fn main() {}
