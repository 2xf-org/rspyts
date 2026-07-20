//! Shared application fixture for the integration test suites.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rspyts::Model;
use serde::{Deserialize, Serialize};

/// A stable identifier supplied by another system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Model)]
#[serde(transparent)]
#[rspyts(host = String)]
pub struct JobId(pub String);

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
    /// Binary input that must remain a byte boundary.
    #[rspyts(bytes)]
    pub payload: Vec<u8>,
    /// Numeric input that must remain a contiguous buffer boundary.
    #[rspyts(buffer)]
    pub samples: Vec<f64>,
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
    pub id: JobId,
    pub accepted_count: u32,
    pub sample_total: f64,
    pub event: JobEvent,
}

/// Optional context attached to a job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Model)]
#[serde(rename_all = "camelCase")]
pub struct JobContext {
    pub created_at: DateTime<Utc>,
    pub metadata: serde_json::Value,
    pub labels: BTreeMap<String, String>,
    pub range: (i32, i32),
    pub note: Option<String>,
    #[rspyts(bytes)]
    pub fingerprint: [u8; 16],
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
    Completed { accepted_count: u32 },
    /// The job was rejected.
    Rejected { reason: String },
}

/// Failures that callers can handle by stable code.
#[derive(Debug, thiserror::Error, rspyts::Error)]
pub enum JobError {
    #[error("count must be between 1 and 100")]
    InvalidCount,
    #[error("the initial counter value cannot be negative")]
    #[serde(rename = "negative_start")]
    NegativeStart,
}

/// Validate and execute one job.
#[rspyts::export]
pub fn execute_job(request: &JobRequest) -> Result<JobResult, JobError> {
    if !(1..=100).contains(&request.count) {
        return Err(JobError::InvalidCount);
    }
    Ok(JobResult {
        id: JobId(format!("job-{}", request.display_name)),
        accepted_count: request.count,
        sample_total: request.samples.iter().sum(),
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
