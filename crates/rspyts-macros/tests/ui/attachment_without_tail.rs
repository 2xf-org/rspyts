use rspyts::{bridge, Buf, Bytes};

#[bridge]
pub const BAD_BUFFER: Option<Buf<u8>> = None;

#[bridge(error)]
pub enum BadError {
    Payload { bytes: Option<Bytes> },
}

fn main() {}
