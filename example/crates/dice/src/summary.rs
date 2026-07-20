//! Summaries that use fair-roll models.

use rspyts::Model;
use serde::{Deserialize, Serialize};

use crate::fair::roll::{RollError, RollResult};

/// A named summary of one fair roll.
#[derive(Debug, Clone, Serialize, Deserialize, Model)]
#[serde(rename_all = "camelCase")]
pub struct RollSummary {
    pub label: String,
    pub result: RollResult,
}

/// Add a label to a fair-roll result.
///
/// # Errors
///
/// Returns [`RollError::EmptyLabel`] if `label` is empty.
#[rspyts::export]
pub fn summarize_roll(label: String, result: RollResult) -> Result<RollSummary, RollError> {
    if label.is_empty() {
        return Err(RollError::EmptyLabel);
    }
    Ok(RollSummary { label, result })
}
