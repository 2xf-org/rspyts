//! `u64` is deliberately outside the portable type system
//! (docs/design/type-system.md §1); the rejection surfaces as a missing
//! `Bridged` bound on the field type.
use rspyts::bridge;

/// A struct with a non-bridgeable scalar.
#[bridge]
pub struct HasU64 {
    pub big: u64,
}

fn main() {}
