use rspyts::bridge;

#[bridge]
pub async fn fetch_answer() -> u32 {
    42
}

fn main() {}
