//! multi-crate-app — the app side of the rspyts multi-crate example.
//!
//! This crate bridges functions over types it does not define: [`Point`]
//! and [`Axis`] live in `shared-types`. Linking that crate pulls its type
//! registrations into this module's manifest with their true origin, and
//! the `[python.imports]` / `[typescript.imports]` tables in `rspyts.toml`
//! tell the emitters to import them from shared-types' own generated
//! packages instead of re-emitting them. Python therefore reuses one
//! runtime `Point` class, while TypeScript declarations retain their shared
//! package provenance and one source of truth.

use rspyts::bridge;
use shared_types::{Axis, Point, SharedExact, SharedId};

/// Move `p` by `(dx, dy)`.
#[bridge]
pub fn translate(p: Point, dx: f64, dy: f64) -> Point {
    Point {
        x: p.x + dx,
        y: p.y + dy,
    }
}

/// Mirror `p` across `axis`.
#[bridge]
pub fn mirror(p: Point, axis: Axis) -> Point {
    match axis {
        Axis::Horizontal => Point { x: p.x, y: -p.y },
        Axis::Vertical => Point { x: -p.x, y: p.y },
    }
}

/// Round-trip nested exact integers while preserving the dependency origin.
#[bridge]
pub fn echo_shared_exact(value: SharedExact) -> SharedExact {
    value
}

/// Round-trip an exact-integer newtype defined by the dependency crate.
#[bridge]
pub fn echo_shared_id(value: SharedId) -> SharedId {
    value
}

rspyts::export!();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_moves_the_point() {
        let moved = translate(Point { x: 1.0, y: 2.0 }, 3.0, 4.0);
        assert_eq!((moved.x, moved.y), (4.0, 6.0));
    }

    #[test]
    fn mirror_flips_one_coordinate() {
        let flipped = mirror(Point { x: 1.0, y: 2.0 }, Axis::Vertical);
        assert_eq!((flipped.x, flipped.y), (-1.0, 2.0));
        let flipped = mirror(Point { x: 1.0, y: 2.0 }, Axis::Horizontal);
        assert_eq!((flipped.x, flipped.y), (1.0, -2.0));
    }

    #[test]
    fn shared_exact_integers_use_native_rust_values() {
        let value = echo_shared_exact(SharedExact {
            id: u64::MAX,
            signed: i64::MIN,
            history: vec![0, 9_007_199_254_740_993, u64::MAX],
            note: None,
        });
        assert_eq!(value.id, u64::MAX);
        assert_eq!(value.signed, i64::MIN);
        assert_eq!(value.history[1], 9_007_199_254_740_993);
        assert_eq!(value.note, None);
        assert_eq!(echo_shared_id(SharedId(u64::MAX)).0, u64::MAX);
    }
}
