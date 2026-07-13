use rspyts::bridge;

#[bridge]
pub fn first<T: Clone>(items: Vec<T>) -> T {
    items[0].clone()
}

fn main() {}
