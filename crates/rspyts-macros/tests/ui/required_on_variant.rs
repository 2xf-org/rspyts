use rspyts::bridge;

#[bridge]
pub enum RequiredVariant {
    #[bridge(required)]
    Value,
}

fn main() {}
