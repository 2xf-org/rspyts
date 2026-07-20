use rspyts::Model;
use serde::{Deserialize, Serialize};

/// A request to roll one type of die.
#[derive(Debug, Clone, Serialize, Deserialize, Model)]
#[serde(rename_all = "camelCase")]
pub struct RollRequest {
    /// The number of sides on each die.
    #[rspyts(ge = 2, le = 100)]
    pub sides: u32,
    /// The number of dice to roll.
    #[rspyts(ge = 1, le = 100)]
    pub count: u32,
}

/// The result of a dice roll.
#[derive(Debug, Clone, Serialize, Deserialize, Model)]
#[serde(rename_all = "camelCase")]
pub struct RollResult {
    pub values: Vec<u32>,
    pub total: u64,
}

/// Errors from the example API.
#[derive(Debug, thiserror::Error, rspyts::Error)]
pub enum RollError {
    #[error("the request is outside the supported range")]
    InvalidRequest,
}

/// A repeatable source of dice rolls.
pub struct DiceCup {
    sides: u32,
    state: u64,
}

#[rspyts::export]
impl DiceCup {
    /// Create a dice cup.
    #[rspyts(constructor)]
    pub fn new(sides: u32, seed: u64) -> Result<Self, RollError> {
        if !(2..=100).contains(&sides) {
            return Err(RollError::InvalidRequest);
        }
        Ok(Self { sides, state: seed })
    }

    /// Roll the dice in this cup.
    pub fn roll(&mut self, count: u32) -> Result<RollResult, RollError> {
        if !(1..=100).contains(&count) {
            return Err(RollError::InvalidRequest);
        }
        let values = (0..count)
            .map(|_| next_roll(&mut self.state, self.sides))
            .collect::<Vec<_>>();
        let total = values.iter().map(|value| u64::from(*value)).sum();
        Ok(RollResult { values, total })
    }
}

/// Roll dice from a seed.
///
/// The seed makes the example repeatable in Rust, Python, and TypeScript.
#[rspyts::export]
pub fn roll_dice(request: RollRequest, seed: u64) -> Result<RollResult, RollError> {
    if !(2..=100).contains(&request.sides) || !(1..=100).contains(&request.count) {
        return Err(RollError::InvalidRequest);
    }
    let mut state = seed;
    let values = (0..request.count)
        .map(|_| next_roll(&mut state, request.sides))
        .collect::<Vec<_>>();
    let total = values.iter().map(|value| u64::from(*value)).sum();
    Ok(RollResult { values, total })
}

/// Roll dice and return a compact numeric buffer.
#[rspyts::export]
#[rspyts(returns(buffer))]
pub fn roll_values(request: RollRequest, seed: u64) -> Result<Vec<u32>, RollError> {
    roll_dice(request, seed).map(|result| result.values)
}

/// Convert bytes to a repeatable seed.
#[rspyts::export]
pub fn seed_from_bytes(#[rspyts(bytes)] bytes: &[u8]) -> u64 {
    bytes.iter().fold(0_u64, |seed, byte| {
        seed.wrapping_mul(31).wrapping_add(u64::from(*byte))
    })
}

#[rspyts::export]
pub const DEFAULT_SEED: u64 = 42;

fn next_roll(state: &mut u64, sides: u32) -> u32 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1);
    ((*state >> 32) as u32 % sides) + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_rolls_are_repeatable() {
        let request = RollRequest { sides: 6, count: 3 };
        let first = roll_dice(request.clone(), DEFAULT_SEED).unwrap();
        let second = roll_dice(request, DEFAULT_SEED).unwrap();
        assert_eq!(first.values, second.values);
    }
}
