//! shared-types — the dependency crate of the rspyts multi-crate example.
//!
//! Types bridged here carry `origin: "shared-types"` in every manifest
//! they appear in, including the manifests of crates that merely depend
//! on this one. That origin is what lets downstream configs map them to
//! this crate's own generated packages instead of re-emitting them.
//!
//! Note what is *not* here: `rspyts::export!()`. A compiled module has
//! exactly one exporter, and this crate is linked into other bridged
//! cdylibs (see `examples/multi-crate/app`). The tiny `module/` crate
//! next door re-exports everything and adds the export so this crate can
//! also be generated standalone.

use rspyts::bridge;

/// A point on the plane.
#[bridge]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// An axis to mirror across.
#[bridge]
pub enum Axis {
    Horizontal,
    Vertical,
}
