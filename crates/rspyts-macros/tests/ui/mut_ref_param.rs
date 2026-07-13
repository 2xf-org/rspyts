use rspyts::bridge;

#[bridge]
pub fn bump(value: &mut u32) {
    *value += 1;
}

fn main() {}
