//! Shared application fixture for the integration test suites.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rspyts::Model;
use serde::{Deserialize, Serialize};

mod inline {
    use rspyts::Model;
    use serde::{Deserialize, Serialize};

    /// A model declared in an inline Rust module.
    #[allow(
        dead_code,
        reason = "the contract suite inspects this declaration without constructing it"
    )]
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Model)]
    pub struct InlineValue {
        /// Payload retained under the declaration's original module.
        pub value: String,
    }
}

#[allow(
    unused_imports,
    reason = "the contract suite verifies that a re-export does not change the declaration path"
)]
pub use inline::InlineValue;

/// A stable identifier supplied by another system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Model)]
#[serde(transparent)]
#[rspyts(host = String)]
pub struct JobId(
    /// Identifier text as received from or sent to the host.
    pub String,
);

/// The supported ways to run a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Model)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    /// Favor low latency.
    Fast,
    /// Favor repeatable output.
    Deterministic,
}

/// A request that crosses both generated host boundaries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Model)]
#[serde(rename_all = "camelCase")]
pub struct JobRequest {
    /// A user-visible job label.
    #[rspyts(min_length = 2, max_length = 64)]
    pub display_name: String,
    /// The number of values to process.
    #[rspyts(ge = 1, le = 100)]
    pub count: u32,
    /// Select the execution strategy.
    pub mode: RunMode,
    /// Disable side effects when true.
    #[serde(default)]
    #[rspyts(default = false)]
    pub dry_run: bool,
    /// Optional context supplied by the caller.
    pub context: Option<JobContext>,
}

/// The result returned for a valid job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Model)]
#[serde(rename_all = "camelCase")]
pub struct JobResult {
    /// Stable identifier allocated for the accepted job.
    pub id: JobId,
    /// Number of values accepted for processing.
    pub accepted_count: u32,
    /// Sum of the supplied numeric samples.
    pub sample_total: f64,
    /// Terminal state produced by execution.
    pub event: JobEvent,
}

/// Full-width unsigned values used to verify the Python boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Model)]
#[serde(rename_all = "camelCase")]
pub struct WideNumbers {
    /// Scalar unsigned value at the full Rust width.
    pub value: u64,
    /// Nested unsigned values at the full Rust width.
    pub values: Vec<u64>,
    /// Optional unsigned value at the full Rust width.
    pub optional: Option<u64>,
}

/// Optional context attached to a job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Model)]
#[serde(rename_all = "camelCase")]
pub struct JobContext {
    /// UTC creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Arbitrary JSON metadata supplied by the caller.
    pub metadata: serde_json::Value,
    /// Deterministically ordered labels.
    pub labels: BTreeMap<String, String>,
    /// Inclusive integer range associated with the job.
    pub range: (i32, i32),
    /// Optional free-form note.
    pub note: Option<String>,
    /// Binary fingerprint serialized as an ordinary model field.
    pub fingerprint: Vec<u8>,
}

/// A state update emitted by a job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Model)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum JobEvent {
    /// The job completed.
    Completed {
        /// Number of values accepted by the completed job.
        accepted_count: u32,
    },
    /// The job was rejected.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
}

/// Failures that callers can handle by stable code.
#[derive(Debug, thiserror::Error, rspyts::Error)]
pub enum JobError {
    /// A requested count fell outside the supported interval.
    #[error("count must be between 1 and 100")]
    InvalidCount,
    /// A counter constructor received a negative initial value.
    #[error("the initial counter value cannot be negative")]
    #[serde(rename = "negative_start")]
    NegativeStart,
}

/// Validate and execute one job.
#[rspyts::export]
pub fn execute_job(
    request: &JobRequest,
    #[rspyts(bytes)] payload: &[u8],
    #[rspyts(buffer)] samples: &[f64],
) -> Result<JobResult, JobError> {
    if !(1..=100).contains(&request.count) {
        return Err(JobError::InvalidCount);
    }
    let _payload_size = payload.len();
    Ok(JobResult {
        id: JobId(format!("job-{}", request.display_name)),
        accepted_count: request.count,
        sample_total: samples.iter().sum(),
        event: JobEvent::Completed {
            accepted_count: request.count,
        },
    })
}

/// Reverse a byte sequence without changing its host boundary type.
#[rspyts::export]
#[rspyts(returns(bytes))]
pub fn reverse_bytes(#[rspyts(bytes)] input: &[u8]) -> Vec<u8> {
    input.iter().rev().copied().collect()
}

/// Scale a numeric buffer without turning it into a model.
#[rspyts::export]
#[rspyts(returns(buffer))]
pub fn scale_samples(#[rspyts(buffer)] samples: &[f64], factor: f64) -> Vec<f64> {
    samples.iter().map(|value| value * factor).collect()
}

/// Return full-width unsigned values unchanged.
#[rspyts::export]
#[must_use]
pub fn echo_u64(value: u64) -> u64 {
    value
}

/// Return nested full-width unsigned values unchanged.
#[rspyts::export]
#[must_use]
pub fn echo_wide_numbers(value: WideNumbers) -> WideNumbers {
    value
}

/// A stateful counter exposed as one native resource.
pub struct Counter {
    value: i64,
}

#[rspyts::export]
impl Counter {
    /// Create a counter with an explicit value.
    #[rspyts(constructor)]
    pub fn new(initial_value: i64) -> Result<Self, JobError> {
        if initial_value < 0 {
            return Err(JobError::NegativeStart);
        }
        Ok(Self {
            value: initial_value,
        })
    }

    /// Create a counter at zero.
    #[rspyts(constructor)]
    pub fn from_zero() -> Self {
        Self { value: 0 }
    }

    /// Add a value and return the new total.
    pub fn add(&mut self, delta: i64) -> i64 {
        self.value += delta;
        self.value
    }

    /// Reset the counter for Rust-only callers.
    #[rspyts(skip)]
    #[allow(
        dead_code,
        reason = "the integration suite verifies that this method stays hidden"
    )]
    pub fn reset(&mut self) {
        self.value = 0;
    }
}

/// The contract revision used by this integration fixture.
#[rspyts::export]
pub const CONTRACT_REVISION: u32 = 7;

/// The default label used by this integration fixture.
#[rspyts::export]
pub static DEFAULT_LABEL: &str = "integration";

rspyts::application!();
