//! shared-types — the dependency crate of the rspyts multi-crate example.
//!
//! Types bridged here carry `origin: "shared-types"` in every manifest
//! they appear in, including the manifests of crates that merely depend
//! on this one. That origin is what lets downstream configs map them to
//! this crate's own generated packages instead of re-emitting them.
//!
//! A compiled module must have exactly one exporter. The
//! `standalone-module` feature adds this crate's exporter for its own codegen
//! build; downstream crates link the default rlib without that feature and
//! provide their own exporter.

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

/// Exact integer data defined in one crate and bridged by another.
#[bridge]
pub struct SharedExact {
    pub id: u64,
    pub signed: i64,
    pub history: Vec<u64>,
    #[bridge(required)]
    pub note: Option<String>,
}

/// A named exact identifier whose origin remains the shared crate.
#[bridge]
pub struct SharedId(pub u64);

#[cfg(feature = "standalone-module")]
rspyts::export!();
