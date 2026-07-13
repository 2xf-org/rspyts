use rspyts::{Buf, bridge};

#[bridge]
pub fn consume(values: Buf<f64>) -> f64 {
    values.into_inner().iter().sum()
}

fn main() {}
