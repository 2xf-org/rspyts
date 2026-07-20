//! Results for a loaded die.

use rspyts::Model;
use serde::{Deserialize, Serialize};

/// The result of one loaded-die roll.
#[derive(Debug, Clone, Serialize, Deserialize, Model)]
#[serde(rename_all = "camelCase")]
pub struct RollResult {
    pub value: u32,
    pub favored_value: u32,
}

/// Return one loaded-die result.
#[rspyts::export]
#[must_use]
pub fn loaded_roll(value: u32) -> RollResult {
    RollResult {
        value,
        favored_value: DEFAULT_FAVORED_VALUE,
    }
}

/// The default favored value.
#[rspyts::export]
pub const DEFAULT_FAVORED_VALUE: u32 = 6;
