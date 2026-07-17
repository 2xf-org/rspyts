//! A direct domain API used to prove the rspyts 0.4 host boundaries.

use rspyts::{Error, Type};
use serde::{Deserialize, Serialize};

fn default_quantity() -> u32 {
    1
}

/// The source channel attached to an analysis result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Channel {
    pub id: String,
    pub sample_rate_hz: u32,
}

/// A constrained request proving defaults and aware datetimes in both hosts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContractRequest {
    #[rspyts(literal = 2)]
    pub contract_version: u32,
    #[rspyts(min_length = 1, max_length = 32)]
    pub actor: String,
    #[serde(default = "default_quantity")]
    #[rspyts(default = 1, ge = 1)]
    pub quantity: u32,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
    #[rspyts(min_length = 1, max_length = 3)]
    pub tags: Vec<String>,
}

/// A portable quality classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Good,
    Noisy,
}

/// The nested result returned by the real Rust implementation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Summary {
    pub channel: Channel,
    pub quality: Quality,
    pub count: u64,
    pub average: f64,
    #[rspyts(buffer)]
    pub normalized: Vec<f64>,
    #[rspyts(bytes)]
    pub fingerprint: Vec<u8>,
}

/// Intentional API failures, exposed as typed host exceptions.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Error)]
pub enum AnalyzeError {
    #[error("values cannot be empty")]
    Empty,
    #[error("scale must be finite and greater than zero")]
    InvalidScale,
    #[error("fingerprint must contain exactly four bytes")]
    InvalidFingerprint,
}

/// Analyze one signal without introducing a transport DTO.
#[rspyts::export]
pub fn summarize(
    channel: Channel,
    #[rspyts(buffer)] values: &[f64],
    #[rspyts(bytes)] fingerprint: &[u8],
    scale: f64,
) -> Result<Summary, AnalyzeError> {
    if values.is_empty() {
        return Err(AnalyzeError::Empty);
    }
    if !scale.is_finite() || scale <= 0.0 {
        return Err(AnalyzeError::InvalidScale);
    }
    if fingerprint.len() != 4 {
        return Err(AnalyzeError::InvalidFingerprint);
    }

    let average = values.iter().sum::<f64>() / values.len() as f64;
    let normalized = values.iter().map(|value| value * scale).collect();
    let quality = if values.iter().any(|value| value.abs() > 100.0) {
        Quality::Noisy
    } else {
        Quality::Good
    };

    Ok(Summary {
        channel,
        quality,
        count: values.len() as u64,
        average,
        normalized,
        fingerprint: fingerprint.to_vec(),
    })
}

/// Round-trip a constrained request through the real Rust boundary.
#[rspyts::export]
pub fn validate_request(request: ContractRequest) -> ContractRequest {
    request
}

/// Stateful domain behavior exported through the existing implementation.
pub struct Analyzer {
    channel: Channel,
    scale: f64,
    calls: u64,
}

#[rspyts::export]
impl Analyzer {
    #[rspyts(constructor)]
    pub fn new(channel: Channel, scale: f64) -> Result<Self, AnalyzeError> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err(AnalyzeError::InvalidScale);
        }
        Ok(Self {
            channel,
            scale,
            calls: 0,
        })
    }

    pub fn summarize(
        &mut self,
        #[rspyts(buffer)] values: &[f64],
        #[rspyts(bytes)] fingerprint: &[u8],
    ) -> Result<Summary, AnalyzeError> {
        let result = summarize(self.channel.clone(), values, fingerprint, self.scale)?;
        self.calls += 1;
        Ok(result)
    }

    pub fn calls(&self) -> u64 {
        self.calls
    }
}

rspyts::module!(native);

#[cfg(test)]
mod tests {
    use super::*;

    fn channel() -> Channel {
        Channel {
            id: "c3".into(),
            sample_rate_hz: 256,
        }
    }

    #[test]
    fn direct_function_is_the_reference_behavior() {
        let result = summarize(channel(), &[1.0, 2.0, 3.0], &[1, 2, 3, 4], 2.0).unwrap();
        assert_eq!(result.count, 3);
        assert_eq!(result.average, 2.0);
        assert_eq!(result.normalized, vec![2.0, 4.0, 6.0]);
        assert_eq!(result.fingerprint, vec![1, 2, 3, 4]);
        assert_eq!(result.quality, Quality::Good);
    }

    #[test]
    fn constrained_request_uses_the_rust_default_and_aware_datetime() {
        let request: ContractRequest = serde_json::from_value(serde_json::json!({
            "contractVersion": 2,
            "actor": "viewer",
            "occurredAt": "2026-07-16T12:34:56Z",
            "tags": ["night"]
        }))
        .unwrap();

        assert_eq!(request.quantity, 1);
        assert_eq!(
            request.occurred_at.to_rfc3339(),
            "2026-07-16T12:34:56+00:00"
        );
        assert_eq!(validate_request(request.clone()), request);
    }

    #[test]
    fn typed_errors_do_not_need_a_bridge_error() {
        assert_eq!(
            summarize(channel(), &[], &[1, 2, 3, 4], 1.0),
            Err(AnalyzeError::Empty)
        );
        assert_eq!(
            summarize(channel(), &[1.0], &[1, 2], 1.0),
            Err(AnalyzeError::InvalidFingerprint)
        );
    }

    #[test]
    fn resource_uses_the_same_domain_function() {
        let mut analyzer = Analyzer::new(channel(), 0.5).unwrap();
        let result = analyzer.summarize(&[2.0, 4.0], &[9, 8, 7, 6]).unwrap();
        assert_eq!(result.normalized, vec![1.0, 2.0]);
        assert_eq!(analyzer.calls(), 1);
    }
}
