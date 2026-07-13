use rspyts::bridge;

pub struct Widget {
    count: u32,
}

// No #[bridge(constructor)] and no #[bridge(static)] factory returning
// `Self`: nothing can ever produce a handle.
#[bridge]
impl Widget {
    pub fn poke(&mut self) -> u32 {
        self.count += 1;
        self.count
    }

    #[bridge(static)]
    pub fn limit() -> u32 {
        99
    }
}

fn main() {}
