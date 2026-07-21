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
pub fn roll_dice(value: u32) -> RollResult {
    RollResult {
        value,
        favored_value: DEFAULT_FAVORED_VALUE,
    }
}

/// A repeatable source of loaded-die rolls.
pub struct DiceCup {
    favored_value: u32,
}

#[rspyts::export]
impl DiceCup {
    /// Create a loaded dice cup.
    #[rspyts(constructor)]
    #[must_use]
    pub fn new(favored_value: u32) -> Self {
        Self { favored_value }
    }

    /// Return one loaded-die result.
    #[must_use]
    pub fn roll(&mut self, value: u32) -> RollResult {
        RollResult {
            value,
            favored_value: self.favored_value,
        }
    }
}

/// The default favored value.
#[rspyts::export]
pub const DEFAULT_FAVORED_VALUE: u32 = 6;
