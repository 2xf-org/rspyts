//! Catalog owns this domain type and its host identities.

use rspyts::Type;
use serde::{Deserialize, Serialize};

/// Immutable signal identity shared with downstream domain packages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignalDefinition {
    pub id: String,
    pub sample_rate_hz: u32,
}

/// The Catalog implementation that constructs the shared domain value.
#[rspyts::export]
pub fn define_signal(id: String, sample_rate_hz: u32) -> SignalDefinition {
    SignalDefinition { id, sample_rate_hz }
}

#[cfg(feature = "bindings")]
rspyts::module!(native);
