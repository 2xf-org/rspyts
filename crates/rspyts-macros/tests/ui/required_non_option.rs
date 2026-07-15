use rspyts::bridge;

#[bridge]
pub struct RequiredNonOption {
    #[bridge(required)]
    pub value: String,
}

fn main() {}
