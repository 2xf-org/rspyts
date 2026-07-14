//! basic-example — the rspyts end-to-end example.
//!
//! A deliberately plain little library that nonetheless exercises every
//! feature of the portable type system: structs, string enums, tagged data
//! enums, error enums with data, optional values, borrowed slice
//! parameters, owned `Buf` inputs and returns, plain and fallible functions, bridged
//! constants, schemaless [`Json`] passthrough, binary values, nested owned
//! buffers, transparent newtypes, target-scoped functions, and a stateful
//! handle class with statics and a factory.
//!
//! Build the cdylib (`cargo build -p basic-example`), run
//! `rspyts generate --config examples/basic/rspyts.toml`, and the Python
//! and TypeScript packages next door gain fully typed access to everything
//! below.

use std::collections::BTreeMap;

use rspyts::{Buf, Bytes, I64, Json, U64, bridge};

/// The largest number of values [`summarize`] accepts in one call.
#[bridge]
pub const MAX_WINDOW: u32 = 1024;

/// Wire names of every [`Rounding`] mode, in declaration order.
#[bridge]
pub const ROUNDING_MODES: &[&str] = &["up", "down", "nearest", "halfEven"];

/// Largest unsigned value that can cross the bridge exactly.
#[bridge]
pub const MAX_EXACT_U64: U64 = U64::new(u64::MAX);

/// Signed and unsigned exact-integer bounds.
#[bridge]
pub const EXACT_BOUNDS: (I64, U64) = (I64::new(i64::MIN), U64::new(u64::MAX));

/// Rounding strategy for [`round_value`].
#[bridge]
pub enum Rounding {
    Up,
    Down,
    Nearest,
    /// Banker's rounding: ties go to the nearest even value.
    HalfEven,
}

/// Summary statistics for a list of numbers.
#[bridge]
pub struct Summary {
    /// How many values were summarized.
    pub item_count: u32,
    pub total: f64,
    pub average: f64,
    /// Optional label passed through untouched.
    pub label: Option<String>,
}

/// Stable identifier for a binary packet.
///
/// Transparent newtypes remain named in generated APIs and schemas while
/// using the inner integer directly at runtime.
#[bridge]
pub struct PacketId(pub u32);

/// Binary values and owned numeric buffers nested in ordinary data.
#[bridge]
pub struct BinaryPacket {
    pub id: PacketId,
    /// Opaque bytes, distinct from a numeric `u8` buffer on the wire.
    pub payload: Bytes,
    /// An aligned floating-point buffer.
    pub samples: Buf<f64>,
    /// Buffers can appear inside lists.
    pub chunks: Vec<Buf<i16>>,
    /// Buffers can also appear inside string-keyed maps.
    pub channels: BTreeMap<String, Buf<u8>>,
}

/// Exact integers in plain, tuple, optional, and list positions.
#[bridge]
pub struct ExactNumbers {
    pub signed: I64,
    pub unsigned: U64,
    pub pair: (I64, U64),
    pub history: Option<Vec<U64>>,
}

/// A state that can be either tag-only or carry exact data.
#[bridge]
pub enum TransferState {
    Pending,
    Complete { sequence: U64 },
}

/// A number parsed from text.
#[bridge(tag = "type")]
pub enum ParsedNumber {
    /// The text was a whole number.
    Integer { value: i32 },
    /// The text had a fractional part.
    Decimal { value: f64 },
}

/// Failure modes of the basic functions.
#[bridge(error)]
#[derive(Debug)]
pub enum BasicError {
    /// The input contained no values.
    EmptyInput,
    /// The input contained more than [`MAX_WINDOW`] values.
    TooManyValues { count: u32 },
    /// The text could not be parsed as a number.
    NotANumber { text: String },
    /// The scale factor must be non-zero.
    ZeroFactor { factor: f64 },
    /// The file could not be read.
    UnreadableFile { path: String },
    /// An exact sequence exceeded the caller's exact limit.
    SequenceTooLarge { value: U64, maximum: U64 },
}

impl std::fmt::Display for BasicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BasicError::EmptyInput => write!(f, "input is empty"),
            BasicError::TooManyValues { count } => {
                write!(f, "expected at most {MAX_WINDOW} values, got {count}")
            }
            BasicError::NotANumber { text } => write!(f, "`{text}` is not a number"),
            BasicError::ZeroFactor { factor } => {
                write!(f, "factor must be non-zero, got {factor}")
            }
            BasicError::UnreadableFile { path } => write!(f, "cannot read `{path}`"),
            BasicError::SequenceTooLarge { value, maximum } => {
                write!(f, "sequence {value} exceeds maximum {maximum}")
            }
        }
    }
}

/// Summarize a list of numbers (at most [`MAX_WINDOW`] of them).
#[bridge]
pub fn summarize(values: &[f64], label: Option<String>) -> Result<Summary, BasicError> {
    if values.is_empty() {
        return Err(BasicError::EmptyInput);
    }
    if values.len() > MAX_WINDOW as usize {
        return Err(BasicError::TooManyValues {
            count: values.len() as u32,
        });
    }
    let total: f64 = values.iter().sum();
    Ok(Summary {
        item_count: values.len() as u32,
        total,
        average: total / values.len() as f64,
        label,
    })
}

/// Multiply every value by `factor`.
///
/// The result comes back as a raw `f64` buffer — element count equals the
/// input length, and the bytes never touch JSON.
#[bridge]
pub fn scale(values: &[f64], factor: f64) -> Result<Buf<f64>, BasicError> {
    if factor == 0.0 {
        return Err(BasicError::ZeroFactor { factor });
    }
    Ok(Buf::new(values.iter().map(|v| v * factor).collect()))
}

/// Return arbitrary bytes unchanged.
#[bridge]
pub fn echo_bytes(value: Bytes) -> Bytes {
    value
}

/// Return a nested binary packet unchanged.
///
/// Every attachment is decoded into owned Rust storage before this function
/// runs and encoded into a fresh response attachment afterwards.
#[bridge]
pub fn echo_binary_packet(value: BinaryPacket) -> BinaryPacket {
    value
}

/// Return exact integers unchanged.
#[bridge]
pub fn echo_exact_numbers(value: ExactNumbers) -> ExactNumbers {
    value
}

/// Return an exact pair unchanged.
#[bridge]
pub fn echo_exact_pair(value: (I64, U64)) -> (I64, U64) {
    value
}

/// Return a mixed unit-and-data enum unchanged.
#[bridge]
pub fn echo_transfer_state(value: TransferState) -> TransferState {
    value
}

/// Validate one exact sequence against another.
#[bridge]
pub fn validate_sequence(value: U64, maximum: U64) -> Result<U64, BasicError> {
    if value > maximum {
        return Err(BasicError::SequenceTooLarge { value, maximum });
    }
    Ok(value)
}

/// Round `value` according to `mode`.
#[bridge]
pub fn round_value(value: f64, mode: Rounding) -> f64 {
    match mode {
        Rounding::Up => value.ceil(),
        Rounding::Down => value.floor(),
        Rounding::Nearest => value.round(),
        Rounding::HalfEven => value.round_ties_even(),
    }
}

/// Attach `value` to caller-supplied metadata.
///
/// `metadata` is schemaless JSON: it crosses the bridge untouched (a dict
/// in Python, an object in TypeScript), gains a `"value"` key, and comes
/// back exactly as rich as it went in. Non-object metadata is wrapped
/// under an `"input"` key first.
#[bridge]
pub fn annotate(value: f64, metadata: Json) -> Json {
    let mut object = match metadata.into_inner() {
        serde_json::Value::Object(map) => map,
        other => {
            let mut map = serde_json::Map::new();
            map.insert("input".to_string(), other);
            map
        }
    };
    object.insert("value".to_string(), serde_json::Value::from(value));
    Json::new(serde_json::Value::Object(object))
}

/// Read one number per line from the text file at `path`, skipping blank
/// lines.
///
/// Python-only: the TypeScript projection runs inside a WASM sandbox with
/// no filesystem, so the emitters skip it there. The `extern "C"` shim
/// still exists on every target — only the generated surface is scoped.
#[bridge(target = "python")]
pub fn load_values(path: String) -> Result<Vec<f64>, BasicError> {
    let text = std::fs::read_to_string(&path)
        .map_err(|_| BasicError::UnreadableFile { path: path.clone() })?;
    let mut values = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        match line.parse::<f64>() {
            Ok(value) if value.is_finite() => values.push(value),
            _ => {
                return Err(BasicError::NotANumber {
                    text: line.to_string(),
                });
            }
        }
    }
    if values.is_empty() {
        return Err(BasicError::EmptyInput);
    }
    Ok(values)
}

/// Parse text into an integer or decimal number.
#[bridge]
pub fn parse_number(text: String) -> Result<ParsedNumber, BasicError> {
    let trimmed = text.trim();
    if let Ok(value) = trimmed.parse::<i32>() {
        return Ok(ParsedNumber::Integer { value });
    }
    match trimmed.parse::<f64>() {
        Ok(value) if value.is_finite() => Ok(ParsedNumber::Decimal { value }),
        _ => Err(BasicError::NotANumber { text }),
    }
}

/// Panics on purpose. Exists so the test suites can verify that panics
/// surface as `RspytsPanicError` instead of undefined behavior; never call
/// it from real code.
#[bridge]
pub fn simulate_panic() {
    panic!("intentional panic for testing");
}

// Class docs are read from the #[bridge] impl block below, so the struct
// itself carries only this plain comment.
pub struct Counter {
    label: String,
    start: i32,
    value: i32,
}

/// A counter that lives in Rust; Python and TypeScript hold an opaque
/// handle to it.
#[bridge]
impl Counter {
    /// Create a counter starting at `start`.
    #[bridge(constructor)]
    pub fn new(start: i32, label: String) -> Self {
        Self {
            label,
            start,
            value: start,
        }
    }

    /// Create a counter starting at zero — a factory: it returns `Self`,
    /// so foreign code gets a fresh handle exactly as from the
    /// constructor.
    #[bridge(static)]
    pub fn starting_at_zero(label: String) -> Self {
        Self::new(0, label)
    }

    /// The label given to counters when the caller has no better idea.
    #[bridge(static)]
    pub fn default_label() -> String {
        "unnamed".to_string()
    }

    /// Increase the counter and return the new value.
    pub fn increment(&mut self, by: i32) -> i32 {
        self.value += by;
        self.value
    }

    /// Parse `text` as an integer and add it. Returns the new value.
    pub fn add_parsed(&mut self, text: String) -> Result<i32, BasicError> {
        match parse_number(text)? {
            ParsedNumber::Integer { value } => Ok(self.increment(value)),
            ParsedNumber::Decimal { value } => Err(BasicError::NotANumber {
                text: format!("{value} is not a whole number"),
            }),
        }
    }

    /// Current value, without modifying it.
    pub fn current_value(&self) -> i32 {
        self.value
    }

    /// The label passed to the constructor.
    pub fn label(&self) -> String {
        self.label.clone()
    }

    /// Reset to the starting value.
    pub fn reset(&mut self) {
        self.value = self.start;
    }
}

rspyts::export!();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_computes_stats() {
        let summary = summarize(&[2.0, 4.0, 6.0], Some("demo".into())).unwrap();
        assert_eq!(summary.item_count, 3);
        assert_eq!(summary.total, 12.0);
        assert_eq!(summary.average, 4.0);
        assert_eq!(summary.label.as_deref(), Some("demo"));
    }

    #[test]
    fn summarize_rejects_empty_input() {
        assert!(matches!(summarize(&[], None), Err(BasicError::EmptyInput)));
    }

    #[test]
    fn scale_multiplies() {
        assert_eq!(
            scale(&[1.0, 2.0], 2.0).unwrap().into_inner(),
            vec![2.0, 4.0]
        );
    }

    #[test]
    fn bytes_preserve_empty_and_edge_values() {
        assert!(echo_bytes(Bytes::new(Vec::new())).is_empty());
        assert_eq!(
            echo_bytes(Bytes::new(vec![0x00, 0x7f, 0x80, 0xff])).into_inner(),
            vec![0x00, 0x7f, 0x80, 0xff]
        );
    }

    #[test]
    fn nested_owned_buffers_round_trip() {
        let mut channels = BTreeMap::new();
        channels.insert("red".to_string(), Buf::new(vec![0, 127, 255]));
        channels.insert("empty".to_string(), Buf::new(Vec::new()));
        let packet = echo_binary_packet(BinaryPacket {
            id: PacketId(7),
            payload: Bytes::new(vec![0, 255]),
            samples: Buf::new(vec![1.5, -2.25]),
            chunks: vec![Buf::new(vec![-32_768, 0, 32_767]), Buf::new(Vec::new())],
            channels,
        });

        assert_eq!(packet.id.0, 7);
        assert_eq!(packet.payload.as_slice(), &[0, 255]);
        assert_eq!(packet.samples.0, vec![1.5, -2.25]);
        assert_eq!(packet.chunks[0].0, vec![-32_768, 0, 32_767]);
        assert!(packet.chunks[1].0.is_empty());
        assert_eq!(packet.channels["red"].0, vec![0, 127, 255]);
        assert!(packet.channels["empty"].0.is_empty());
    }

    #[test]
    fn exact_numbers_preserve_all_boundaries_and_nesting() {
        let value = ExactNumbers {
            signed: I64::new(i64::MIN),
            unsigned: U64::new(u64::MAX),
            pair: (I64::new(9_007_199_254_740_993), U64::new(u64::MAX)),
            history: Some(vec![U64::new(0), U64::new(u64::MAX)]),
        };
        let out = echo_exact_numbers(value);
        assert_eq!(out.signed.get(), i64::MIN);
        assert_eq!(out.unsigned.get(), u64::MAX);
        assert_eq!(out.pair.0.get(), 9_007_199_254_740_993);
        assert_eq!(out.history.unwrap()[1].get(), u64::MAX);
        assert_eq!(MAX_EXACT_U64.get(), u64::MAX);
        assert_eq!(EXACT_BOUNDS.0.get(), i64::MIN);
    }

    #[test]
    fn exact_sequence_validation_keeps_typed_error_data() {
        assert_eq!(
            validate_sequence(U64::new(9_007_199_254_740_993), U64::new(u64::MAX))
                .unwrap()
                .get(),
            9_007_199_254_740_993
        );
        assert!(matches!(
            validate_sequence(U64::new(u64::MAX), U64::new(9_007_199_254_740_993)),
            Err(BasicError::SequenceTooLarge { value, maximum })
                if value.get() == u64::MAX && maximum.get() == 9_007_199_254_740_993
        ));
    }

    #[test]
    fn mixed_state_and_tuple_round_trip() {
        assert!(matches!(
            echo_transfer_state(TransferState::Pending),
            TransferState::Pending
        ));
        assert!(matches!(
            echo_transfer_state(TransferState::Complete {
                sequence: U64::new(u64::MAX),
            }),
            TransferState::Complete { sequence } if sequence.get() == u64::MAX
        ));
        assert_eq!(
            echo_exact_pair((I64::new(i64::MIN), U64::new(u64::MAX))),
            (I64::new(i64::MIN), U64::new(u64::MAX))
        );
    }

    #[test]
    fn parse_number_disambiguates() {
        assert!(matches!(
            parse_number("42".into()),
            Ok(ParsedNumber::Integer { value: 42 })
        ));
        assert!(matches!(
            parse_number("3.5".into()),
            Ok(ParsedNumber::Decimal { .. })
        ));
        assert!(matches!(
            parse_number("abc".into()),
            Err(BasicError::NotANumber { .. })
        ));
    }

    #[test]
    fn summarize_rejects_oversized_input() {
        let values = vec![1.0; MAX_WINDOW as usize + 1];
        assert!(matches!(
            summarize(&values, None),
            Err(BasicError::TooManyValues { count }) if count == MAX_WINDOW + 1
        ));
    }

    #[test]
    fn round_value_half_even_breaks_ties_toward_even() {
        assert_eq!(round_value(2.5, Rounding::HalfEven), 2.0);
        assert_eq!(round_value(3.5, Rounding::HalfEven), 4.0);
        assert_eq!(round_value(2.4, Rounding::HalfEven), 2.0);
    }

    #[test]
    fn annotate_merges_value_into_objects() {
        let out = annotate(2.5, Json::new(serde_json::json!({"source": "demo"}))).into_inner();
        assert_eq!(out, serde_json::json!({"source": "demo", "value": 2.5}));
    }

    #[test]
    fn annotate_wraps_non_objects() {
        let out = annotate(1.0, Json::new(serde_json::json!([1, 2]))).into_inner();
        assert_eq!(out, serde_json::json!({"input": [1, 2], "value": 1.0}));
    }

    #[test]
    fn load_values_reads_one_number_per_line() {
        let path = std::env::temp_dir().join(format!("rspyts-basic-{}.txt", std::process::id()));
        std::fs::write(&path, "1.5\n\n  2\n-3.25\n").unwrap();
        let values = load_values(path.to_string_lossy().into_owned()).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(values, vec![1.5, 2.0, -3.25]);
        assert!(matches!(
            load_values("/definitely/not/a/file".into()),
            Err(BasicError::UnreadableFile { .. })
        ));
    }

    #[test]
    fn counter_statics() {
        let mut counter = Counter::starting_at_zero("fresh".into());
        assert_eq!(counter.current_value(), 0);
        assert_eq!(counter.increment(3), 3);
        assert_eq!(counter.label(), "fresh");
        assert_eq!(Counter::default_label(), "unnamed");
    }

    #[test]
    fn counter_lifecycle() {
        let mut counter = Counter::new(10, "test".into());
        assert_eq!(counter.increment(5), 15);
        assert_eq!(counter.add_parsed("7".into()).unwrap(), 22);
        assert!(counter.add_parsed("1.5".into()).is_err());
        assert_eq!(counter.current_value(), 22);
        counter.reset();
        assert_eq!(counter.current_value(), 10);
    }
}
