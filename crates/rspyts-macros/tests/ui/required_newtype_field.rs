use rspyts::bridge;

#[bridge]
pub struct RequiredNewtype(#[bridge(required)] pub Option<String>);

fn main() {}
