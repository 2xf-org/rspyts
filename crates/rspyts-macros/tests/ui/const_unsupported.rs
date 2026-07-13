use rspyts::bridge;

#[bridge]
pub const BAD_REF: &'static f64 = &1.0;

#[bridge]
pub const BAD_PTR: *const u8 = std::ptr::null();

fn main() {}
