//! The owner defines canonical models for every generated host package.

use rspyts::{Error, Type};
use serde::{Deserialize, Serialize};

/// Stable item kind serialized across every host boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Standard,
    Premium,
}

/// A JSON-safe quantity represented as a fraction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Quantity {
    pub numerator: u32,
    pub denominator: u32,
}

/// Canonical item shared with downstream packages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Item {
    pub id: String,
    pub quantity: Quantity,
    pub kind: Option<ItemKind>,
    #[rspyts(bytes)]
    pub tag: [u8; 4],
}

/// Native-only counter that deliberately has no JSON-safe wire projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeCounter {
    pub value: u64,
}

/// The vector metadata attached to a calculation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VectorSpec {
    pub name: String,
    pub dimensions: u32,
}

/// Constrained batch options proving defaults and aware datetimes in both hosts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchOptions {
    #[rspyts(literal = 1)]
    pub schema_version: u32,
    #[rspyts(min_length = 1, max_length = 32)]
    pub label: String,
    #[rspyts(default = 1, ge = 1, le = 3)]
    pub attempts: u32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[rspyts(min_length = 1, max_length = 3)]
    pub groups: Vec<String>,
}

/// A portable magnitude classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Magnitude {
    Regular,
    Large,
}

/// The nested result returned by the real Rust implementation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Calculation {
    pub vector: VectorSpec,
    pub magnitude: Magnitude,
    pub count: u32,
    pub mean: f64,
    #[rspyts(buffer)]
    pub scaled: Vec<f64>,
    #[rspyts(bytes)]
    pub checksum: [u8; 4],
}

/// Intentional API failures, exposed as typed host exceptions.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Error)]
pub enum CalculationError {
    #[error("values cannot be empty")]
    Empty,
    #[error("factor must be finite and greater than zero")]
    InvalidFactor,
}

/// Largest wide integer that remains exact on the static JavaScript wire.
#[rspyts::export]
pub const SAFE_WIDE: u64 = 9_007_199_254_740_991;

/// Wide integer retained by native hosts but omitted from the static wire.
#[rspyts::export]
pub const UNSAFE_WIDE: u64 = 9_007_199_254_740_992;

/// Construct a canonical item.
#[rspyts::export]
pub fn create_item(id: String, quantity: u32) -> Item {
    Item {
        id,
        quantity: Quantity {
            numerator: quantity,
            denominator: 1,
        },
        kind: Some(ItemKind::Standard),
        tag: [1, 2, 3, 4],
    }
}

/// Construct a native/Python counter without exposing it on the static wire surface.
#[rspyts::export(python)]
pub fn native_counter(value: u64) -> NativeCounter {
    NativeCounter { value }
}

/// Calculate one vector without introducing a transport DTO.
#[rspyts::export]
pub fn calculate(
    vector: VectorSpec,
    #[rspyts(buffer)] values: &[f64],
    #[rspyts(bytes)] checksum: &[u8; 4],
    factor: f64,
) -> Result<Calculation, CalculationError> {
    if values.is_empty() {
        return Err(CalculationError::Empty);
    }
    if !factor.is_finite() || factor <= 0.0 {
        return Err(CalculationError::InvalidFactor);
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let scaled = values.iter().map(|value| value * factor).collect();
    let magnitude = if values.iter().any(|value| value.abs() > 100.0) {
        Magnitude::Large
    } else {
        Magnitude::Regular
    };

    Ok(Calculation {
        vector,
        magnitude,
        count: u32::try_from(values.len()).expect("accepted buffers fit in u32"),
        mean,
        scaled,
        checksum: *checksum,
    })
}

/// Round-trip constrained batch options through the real Rust boundary.
#[rspyts::export]
pub fn validate_batch_options(options: BatchOptions) -> BatchOptions {
    options
}

/// Stateful calculator behavior exported through the existing implementation.
pub struct Calculator {
    vector: VectorSpec,
    factor: f64,
    calls: u64,
}

#[rspyts::export]
impl Calculator {
    #[rspyts(constructor)]
    pub fn new(vector: VectorSpec, factor: f64) -> Result<Self, CalculationError> {
        if !factor.is_finite() || factor <= 0.0 {
            return Err(CalculationError::InvalidFactor);
        }
        Ok(Self {
            vector,
            factor,
            calls: 0,
        })
    }

    pub fn calculate(
        &mut self,
        #[rspyts(buffer)] values: &[f64],
        #[rspyts(bytes)] checksum: &[u8; 4],
    ) -> Result<Calculation, CalculationError> {
        let result = calculate(self.vector.clone(), values, checksum, self.factor)?;
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

    fn vector() -> VectorSpec {
        VectorSpec {
            name: "example".into(),
            dimensions: 3,
        }
    }

    #[test]
    fn direct_function_is_the_reference_behavior() {
        let result = calculate(vector(), &[1.0, 2.0, 3.0], &[1, 2, 3, 4], 2.0).unwrap();
        assert_eq!(result.count, 3);
        assert_eq!(result.mean, 2.0);
        assert_eq!(result.scaled, vec![2.0, 4.0, 6.0]);
        assert_eq!(result.checksum, [1, 2, 3, 4]);
        assert_eq!(result.magnitude, Magnitude::Regular);
    }

    #[test]
    fn constrained_options_accept_the_host_default_and_aware_datetime() {
        let options: BatchOptions = serde_json::from_value(serde_json::json!({
            "schemaVersion": 1,
            "label": "example",
            "attempts": 1,
            "createdAt": "2030-01-02T03:04:05Z",
            "groups": ["primary"]
        }))
        .unwrap();

        assert_eq!(options.attempts, 1);
        assert_eq!(options.created_at.to_rfc3339(), "2030-01-02T03:04:05+00:00");
        assert_eq!(validate_batch_options(options.clone()), options);
    }

    #[test]
    fn typed_errors_do_not_need_a_bridge_error() {
        assert_eq!(
            calculate(vector(), &[], &[1, 2, 3, 4], 1.0),
            Err(CalculationError::Empty)
        );
    }

    #[test]
    fn resource_uses_the_same_domain_function() {
        let mut calculator = Calculator::new(vector(), 0.5).unwrap();
        let result = calculator.calculate(&[2.0, 4.0], &[9, 8, 7, 6]).unwrap();
        assert_eq!(result.scaled, vec![1.0, 2.0]);
        assert_eq!(calculator.calls(), 1);
    }
}
