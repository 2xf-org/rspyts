//! A small sensor-reading toolkit exposed to Python and TypeScript.

use rspyts::Model;
use serde::{Deserialize, Serialize};

const ENCODED_HEADER: &[u8; 8] = b"RSPYTS01";
const ENCODED_PREFIX_LENGTH: usize = ENCODED_HEADER.len() + size_of::<u32>();

/// Version of the portable reading format produced by [`encode_readings`].
#[rspyts::export]
pub const READINGS_FORMAT_VERSION: u32 = 1;

/// The direction from the first reading to the last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Model)]
#[serde(rename_all = "snake_case")]
pub enum Trend {
    /// The final reading is greater than the first.
    Rising,
    /// The final reading is less than the first.
    Falling,
    /// The first and final readings are equal.
    Steady,
}

/// Descriptive statistics for one series of sensor readings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Model)]
#[serde(rename_all = "camelCase")]
pub struct ReadingSummary {
    /// Number of readings in the series.
    pub count: u32,
    /// Smallest reading in the series.
    pub minimum: f64,
    /// Largest reading in the series.
    pub maximum: f64,
    /// Arithmetic mean of the readings.
    pub mean: f64,
    /// Direction from the first reading to the last.
    pub trend: Trend,
}

/// Errors returned for invalid reading data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, rspyts::Error)]
pub enum ReadingError {
    /// No readings were supplied.
    #[error("at least one reading is required")]
    EmptyReadings,
    /// A reading was NaN or infinite.
    #[error("readings must contain only finite numbers")]
    NonFiniteReading,
    /// The series cannot be represented by the portable format.
    #[error("the series contains too many readings")]
    TooManyReadings,
    /// The byte payload was not produced by [`encode_readings`].
    #[error("invalid encoded readings")]
    InvalidEncoding,
}

/// Summarize a series without copying its numeric buffer.
///
/// # Errors
///
/// Returns [`ReadingError::EmptyReadings`] for an empty series and
/// [`ReadingError::NonFiniteReading`] for NaN or infinite values.
#[rspyts::export]
pub fn summarize_readings(
    #[rspyts(buffer)] readings: &[f64],
) -> Result<ReadingSummary, ReadingError> {
    validate_readings(readings)?;
    let count = u32::try_from(readings.len()).map_err(|_| ReadingError::TooManyReadings)?;
    let first = readings[0];
    let last = readings[readings.len() - 1];
    let (minimum, maximum) = readings
        .iter()
        .copied()
        .fold((first, first), |(minimum, maximum), reading| {
            (minimum.min(reading), maximum.max(reading))
        });
    let scale = maximum.abs().max(minimum.abs());
    let mean = if scale == 0.0 {
        0.0
    } else {
        readings.iter().map(|reading| reading / scale).sum::<f64>() / f64::from(count) * scale
    };
    let trend = if last > first {
        Trend::Rising
    } else if last < first {
        Trend::Falling
    } else {
        Trend::Steady
    };

    Ok(ReadingSummary {
        count,
        minimum,
        maximum,
        mean,
        trend,
    })
}

/// Scale a series into the inclusive range from zero to one.
///
/// A constant series becomes all zeros.
///
/// # Errors
///
/// Returns [`ReadingError::EmptyReadings`] for an empty series and
/// [`ReadingError::NonFiniteReading`] for NaN or infinite values.
#[rspyts::export]
#[rspyts(returns(buffer))]
pub fn normalize_readings(#[rspyts(buffer)] readings: &[f64]) -> Result<Vec<f64>, ReadingError> {
    let summary = summarize_readings(readings)?;
    let range = summary.maximum - summary.minimum;
    if range == 0.0 {
        return Ok(vec![0.0; readings.len()]);
    }
    Ok(readings
        .iter()
        .map(|reading| (reading - summary.minimum) / range)
        .collect())
}

/// Encode readings as a versioned, little-endian byte payload.
///
/// # Errors
///
/// Returns [`ReadingError::EmptyReadings`] for an empty series,
/// [`ReadingError::NonFiniteReading`] for NaN or infinite values, and
/// [`ReadingError::TooManyReadings`] when the series length exceeds `u32`.
#[rspyts::export]
#[rspyts(returns(bytes))]
pub fn encode_readings(#[rspyts(buffer)] readings: &[f64]) -> Result<Vec<u8>, ReadingError> {
    validate_readings(readings)?;
    let count = u32::try_from(readings.len()).map_err(|_| ReadingError::TooManyReadings)?;
    let mut encoded = Vec::with_capacity(ENCODED_PREFIX_LENGTH + size_of_val(readings));
    encoded.extend_from_slice(ENCODED_HEADER);
    encoded.extend_from_slice(&count.to_le_bytes());
    for reading in readings {
        encoded.extend_from_slice(&reading.to_le_bytes());
    }
    Ok(encoded)
}

/// Decode a payload created by [`encode_readings`].
///
/// # Errors
///
/// Returns [`ReadingError::InvalidEncoding`] when the header, length, or
/// contents are invalid.
#[rspyts::export]
#[rspyts(returns(buffer))]
pub fn decode_readings(#[rspyts(bytes)] encoded: &[u8]) -> Result<Vec<f64>, ReadingError> {
    let (header, body) = encoded
        .split_at_checked(ENCODED_PREFIX_LENGTH)
        .ok_or(ReadingError::InvalidEncoding)?;
    if &header[..ENCODED_HEADER.len()] != ENCODED_HEADER {
        return Err(ReadingError::InvalidEncoding);
    }
    let count = u32::from_le_bytes(
        header[ENCODED_HEADER.len()..]
            .try_into()
            .map_err(|_| ReadingError::InvalidEncoding)?,
    ) as usize;
    if body.len()
        != count
            .checked_mul(size_of::<f64>())
            .ok_or(ReadingError::InvalidEncoding)?
    {
        return Err(ReadingError::InvalidEncoding);
    }
    let readings = body
        .chunks_exact(size_of::<f64>())
        .map(|chunk| {
            f64::from_le_bytes(
                chunk
                    .try_into()
                    .expect("chunks_exact always yields one complete f64"),
            )
        })
        .collect::<Vec<_>>();
    validate_readings(&readings).map_err(|_| ReadingError::InvalidEncoding)?;
    Ok(readings)
}

fn validate_readings(readings: &[f64]) -> Result<(), ReadingError> {
    if readings.is_empty() {
        return Err(ReadingError::EmptyReadings);
    }
    if readings.iter().any(|reading| !reading.is_finite()) {
        return Err(ReadingError::NonFiniteReading);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_reports_statistics_and_direction() {
        let summary = summarize_readings(&[12.0, 18.0, 15.0, 21.0]).unwrap();

        assert_eq!(summary.count, 4);
        assert_eq!(summary.minimum, 12.0);
        assert_eq!(summary.maximum, 21.0);
        assert_eq!(summary.mean, 16.5);
        assert_eq!(summary.trend, Trend::Rising);
        assert_eq!(
            summarize_readings(&[3.0, 2.0, 1.0]).unwrap().trend,
            Trend::Falling
        );
        assert_eq!(
            summarize_readings(&[3.0, 2.0, 3.0]).unwrap().trend,
            Trend::Steady
        );
    }

    #[test]
    fn normalization_preserves_shape_and_handles_constant_series() {
        assert_eq!(
            normalize_readings(&[10.0, 15.0, 20.0]).unwrap(),
            [0.0, 0.5, 1.0]
        );
        assert_eq!(normalize_readings(&[7.0, 7.0]).unwrap(), [0.0, 0.0]);
    }

    #[test]
    fn binary_format_round_trips_exact_values() {
        let readings = [-1.25, 0.0, 42.5];
        let encoded = encode_readings(&readings).unwrap();

        assert_eq!(&encoded[..ENCODED_HEADER.len()], ENCODED_HEADER);
        assert_eq!(decode_readings(&encoded).unwrap(), readings);
    }

    #[test]
    fn invalid_inputs_return_specific_errors() {
        assert_eq!(summarize_readings(&[]), Err(ReadingError::EmptyReadings));
        assert_eq!(
            normalize_readings(&[f64::NAN]),
            Err(ReadingError::NonFiniteReading)
        );
        assert_eq!(
            decode_readings(b"not readings"),
            Err(ReadingError::InvalidEncoding)
        );

        let mut encoded = encode_readings(&[1.0]).unwrap();
        encoded.pop();
        assert_eq!(
            decode_readings(&encoded),
            Err(ReadingError::InvalidEncoding)
        );
    }
}
