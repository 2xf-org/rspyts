use rspyts::bridge;

pub struct Gauge {
    value: f64,
}

#[bridge]
impl Gauge {
    #[bridge(constructor)]
    pub fn create() -> Self {
        Self { value: 0.0 }
    }

    pub fn finish(self) -> f64 {
        self.value
    }
}

fn main() {}
