//! Reports consumes Catalog's type directly; there is no bridge DTO.

use cross_package_catalog::SignalDefinition;
use rspyts::Type;
use serde::{Deserialize, Serialize};

/// Reports-owned result containing the canonical Catalog signal value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportsResult {
    pub signal: SignalDefinition,
    pub accepted: bool,
}
/// Evaluate the canonical Catalog domain value directly.
#[rspyts::export]
pub fn evaluate(signal: SignalDefinition, score: i32) -> ReportsResult {
    ReportsResult {
        signal,
        accepted: score >= 0,
    }
}

rspyts::module!(native);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumes_the_catalog_type_without_a_mirror() {
        let signal = SignalDefinition {
            id: "sig:c3".into(),
            sample_rate_hz: 256,
        };
        let result = evaluate(signal.clone(), 1);
        assert_eq!(result.signal, signal);
        assert!(result.accepted);
    }
}
