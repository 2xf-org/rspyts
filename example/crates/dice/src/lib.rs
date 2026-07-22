//! Random example API with Rust-shaped modules.

use rspyts::Model;
use serde::{Deserialize, Serialize};

pub mod fair;
pub mod loaded;
pub mod summary;

/// A root report that references a model from a nested namespace.
#[derive(Debug, Clone, Serialize, Deserialize, Model)]
#[serde(rename_all = "camelCase")]
pub struct DiceReport {
    pub summary: summary::RollSummary,
}
