//! Evaluation consumes Hardware's type directly; there is no bridge DTO.

use cross_package_hardware::SignalDefinition;
use rspyts::Type;
use serde::{Deserialize, Serialize};

/// Evaluation-owned result containing the canonical Hardware signal value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvaluationResult {
    pub signal: SignalDefinition,
    pub accepted: bool,
}
/// Evaluate the canonical Hardware domain value directly.
#[rspyts::export]
pub fn evaluate(signal: SignalDefinition, score: i32) -> EvaluationResult {
    EvaluationResult {
        signal,
        accepted: score >= 0,
    }
}

rspyts::module!(native);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumes_the_hardware_type_without_a_mirror() {
        let signal = SignalDefinition {
            id: "sig:c3".into(),
            sample_rate_hz: 256,
        };
        let result = evaluate(signal.clone(), 1);
        assert_eq!(result.signal, signal);
        assert!(result.accepted);
    }
}
