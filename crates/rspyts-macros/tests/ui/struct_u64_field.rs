//! Bare `u64` is deliberately outside the portable type system
//! (docs/design/type-system.md §1); the diagnostic points at the explicit
//! exact `rspyts::U64` wrapper.
use rspyts::bridge;

/// A struct with a non-bridgeable scalar.
#[bridge]
pub struct HasU64 {
    pub big: u64,
}

fn main() {}
