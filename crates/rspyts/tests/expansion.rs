//! End-to-end macro expansion tests.
//!
//! This file defines bridged items covering the whole `#[bridge]` surface
//! and exercises the generated `unsafe extern "C"` shims directly from
//! Rust: the shims are `pub` functions in this test crate, so calls go
//! through the exact code paths a foreign runtime would use — args JSON
//! in, response envelope out (ABI §3.1/§4) — without loading a cdylib.
//!
//! `rspyts::export!()` is invoked here (and only here) so the module-level
//! symbols are covered too. Keep this the only integration-test binary in
//! this crate that invokes it.

use rspyts::{Buf, Json, bridge};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

rspyts::export!();

// ---------------------------------------------------------------------------
// Fixture: data types
// ---------------------------------------------------------------------------

/// A nested payload.
#[bridge]
pub struct NestedInfo {
    /// Free-form note.
    pub note: String,
}

/// Every scalar and container in one shape.
///
/// Second doc paragraph.
#[bridge]
pub struct Everything {
    pub flag: bool,
    pub tiny_u: u8,
    pub small_u: u16,
    pub medium_u: u32,
    pub tiny_i: i8,
    pub small_i: i16,
    pub medium_i: i32,
    pub single: f32,
    pub double: f64,
    pub text: String,
    /// Present only sometimes.
    pub maybe_text: Option<String>,
    pub numbers: Vec<i32>,
    pub lookup: HashMap<String, f64>,
    pub ordered: BTreeMap<String, u32>,
    pub nested: NestedInfo,
}

/// Snake-cased wire shape.
#[bridge(rename_all = "snake_case")]
pub struct SnakeWire {
    pub item_count: u32,
    /// Optional maximum.
    pub max_value: Option<f64>,
}

/// Severity levels.
#[bridge]
pub enum Level {
    /// Lowest severity.
    Low,
    MidRange,
    High,
}

/// A geometric shape.
#[bridge]
pub enum Shape {
    /// A circle.
    Circle {
        radius_len: f64,
    },
    Rect {
        width: f64,
        height: f64,
    },
}

/// Something that happened.
#[bridge(tag = "kind")]
pub enum Event {
    Started { at_ms: u32 },
    Stopped { at_ms: u32, reason: Option<String> },
}

/// Failure modes of the fixture class.
#[bridge(error)]
#[derive(Debug)]
pub enum FixtureError {
    /// Nothing to add.
    Empty,
    /// The counter would exceed its limit.
    OverLimit { max_allowed: i32, attempted: i32 },
}

impl std::fmt::Display for FixtureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FixtureError::Empty => write!(f, "nothing to add"),
            FixtureError::OverLimit {
                max_allowed,
                attempted,
            } => {
                write!(f, "adding {attempted} exceeds the limit {max_allowed}")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Fixture: free functions
// ---------------------------------------------------------------------------

/// The answer, no questions asked.
#[bridge]
pub fn zero_params() -> u32 {
    42
}

/// Consume a value, produce nothing.
#[bridge]
pub fn no_return(count: u32) {
    let _ = count;
}

/// Combine three buffers of different dtypes with two plain params.
#[bridge]
pub fn mixed_slices(
    bytes: &[u8],
    scale_factor: f64,
    floats: &[f64],
    offset: i32,
    ints: &[i32],
) -> f64 {
    let byte_sum: f64 = bytes.iter().map(|b| f64::from(*b)).sum();
    let float_sum: f64 = floats.iter().sum();
    let int_sum: f64 = ints.iter().map(|i| f64::from(*i)).sum();
    byte_sum * scale_factor + float_sum + int_sum + f64::from(offset)
}

/// A labeled chunk of samples.
#[bridge]
pub struct Chunk {
    pub label: String,
    /// Raw samples for this chunk.
    pub samples: Buf<f32>,
}

/// Split `values` into chunks of `chunk_len` samples.
#[bridge]
pub fn make_chunks(values: &[f32], chunk_len: u32) -> Vec<Chunk> {
    values
        .chunks(chunk_len as usize)
        .enumerate()
        .map(|(index, chunk)| Chunk {
            label: format!("chunk-{index}"),
            samples: Buf::new(chunk.to_vec()),
        })
        .collect()
}

/// Wrap `text` in guillemets with a check mark.
#[bridge]
pub fn decorate(text: &str) -> String {
    format!("«{text}» ✓")
}

/// Render a short summary of `all`.
#[bridge]
pub fn describe(all: &Everything) -> String {
    format!(
        "flag={} medium_u={} text={} maybe={:?} numbers={:?} nested={}",
        all.flag, all.medium_u, all.text, all.maybe_text, all.numbers, all.nested.note
    )
}

/// Round-trip a snake-cased struct.
#[bridge]
pub fn echo_snake(wire: SnakeWire) -> SnakeWire {
    wire
}

/// Echo `label`, or a placeholder when absent.
#[bridge]
pub fn maybe_label(label: Option<String>) -> String {
    label.unwrap_or_else(|| String::from("<none>"))
}

// ---------------------------------------------------------------------------
// Fixture: constants, targets, Json passthrough
// ---------------------------------------------------------------------------

/// EMG channels.
#[bridge]
pub enum Channel {
    /// Chin electromyography.
    ChinEmg,
    LegMovement,
}

/// Echo a channel.
#[bridge]
pub fn echo_channel(channel: Channel) -> Channel {
    channel
}

/// Library banner shown by clients.
#[bridge]
pub const BANNER: &str = "rspyts expansion fixture";

/// Channel labels, in montage order.
#[bridge]
pub const CHANNEL_LABELS: &[&str] = &["chin_emg", "leg_movement"];

/// Gain applied when none is configured.
#[bridge]
pub const DEFAULT_GAIN: f64 = 1.25;

/// Echo arbitrary JSON.
#[bridge]
pub fn echo_json(value: Json) -> Json {
    value
}

/// Double a value; Python-only surface.
#[bridge(target = "python")]
pub fn python_only(x: u32) -> u32 {
    x * 2
}

// ---------------------------------------------------------------------------
// Fixture: class
// ---------------------------------------------------------------------------

/// A stateful counter with private state.
pub struct Counter {
    total: i32,
    step: i32,
}

/// Counter methods exposed to foreign callers.
#[bridge]
impl Counter {
    /// Create a counter advancing by `step`.
    #[bridge(constructor)]
    pub fn create(step: i32) -> Self {
        Self { total: 0, step }
    }

    /// Current total.
    pub fn total(&self) -> i32 {
        self.total
    }

    /// Advance `times` steps and return the new total.
    pub fn advance(&mut self, times: i32) -> i32 {
        self.total += self.step * times;
        self.total
    }

    /// Add `amount` directly, failing on zero or past `max_allowed`.
    pub fn checked_add(&mut self, amount: i32, max_allowed: i32) -> Result<i32, FixtureError> {
        if amount == 0 {
            return Err(FixtureError::Empty);
        }
        let next = self.total + amount;
        if next > max_allowed {
            return Err(FixtureError::OverLimit {
                max_allowed,
                attempted: amount,
            });
        }
        self.total = next;
        Ok(next)
    }

    /// The step new counters default to.
    #[bridge(static)]
    pub fn suggested_step() -> u32 {
        4
    }
}

/// A recording session opened through a factory.
pub struct Session {
    id: u32,
}

/// Factory-only class: constructed exclusively through `open`.
#[bridge]
impl Session {
    /// Open the session `id`; zero is invalid.
    #[bridge(static)]
    pub fn open(id: u32) -> Result<Self, FixtureError> {
        if id == 0 {
            return Err(FixtureError::Empty);
        }
        Ok(Self { id })
    }

    /// Probe the backend; TypeScript-only surface.
    #[bridge(static, target = "typescript")]
    pub fn probe() -> u32 {
        7
    }

    /// This session's id.
    pub fn id(&self) -> u32 {
        self.id
    }
}

/// Hit counters used by the impl-level scoping tests.
pub struct Telemetry {
    hits: u32,
}

/// Python-scoped impl: members inherit the target unless they override.
#[bridge(target = "python")]
impl Telemetry {
    /// Start with no hits.
    #[bridge(constructor)]
    pub fn start() -> Self {
        Self { hits: 0 }
    }

    /// Count one hit and return the total.
    pub fn record(&mut self) -> u32 {
        self.hits += 1;
        self.hits
    }

    /// Total hits so far.
    pub fn hits(&self) -> u32 {
        self.hits
    }

    /// Override: TypeScript-only despite the impl-level Python default.
    #[bridge(target = "typescript")]
    pub fn probe(&self) -> u32 {
        9
    }

    /// Statics inherit the impl-level default too.
    #[bridge(static)]
    pub fn flush_interval_ms() -> u32 {
        250
    }
}

// ---------------------------------------------------------------------------
// Envelope helpers
// ---------------------------------------------------------------------------

/// A decoded, owned copy of a response envelope.
struct Env {
    status: u8,
    json: Value,
    tail: Vec<u8>,
}

/// Decode and free an envelope returned by a shim.
fn grab(ptr: *mut u8) -> Env {
    let decoded = unsafe { rspyts_core::envelope::decode(ptr) };
    let env = Env {
        status: decoded.status,
        json: serde_json::from_slice(decoded.json).expect("envelope JSON parses"),
        tail: decoded.tail.to_vec(),
    };
    unsafe { rspyts_core::envelope::dealloc(ptr, rspyts_core::envelope::total_len(ptr)) };
    env
}

/// Encode an args JSON object for a shim call.
fn args(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("args encode")
}

fn expect_ok(env: &Env) -> &Value {
    assert_eq!(
        env.status,
        rspyts_core::envelope::STATUS_OK,
        "{:?}",
        env.json
    );
    &env.json
}

fn expect_err<'a>(env: &'a Env, code: &str) -> &'a Value {
    assert_eq!(
        env.status,
        rspyts_core::envelope::STATUS_ERROR,
        "{:?}",
        env.json
    );
    assert_eq!(env.json["code"], code, "{:?}", env.json);
    &env.json
}

// ---------------------------------------------------------------------------
// Module symbols (export!)
// ---------------------------------------------------------------------------

#[test]
fn abi_version_is_one() {
    assert_eq!(rspyts_abi_version(), 1);
}

#[test]
fn alloc_and_free_round_trip() {
    let ptr = rspyts_alloc(16);
    assert!(!ptr.is_null());
    unsafe { rspyts_free(ptr, 16) };
    let empty = rspyts_alloc(0);
    assert!(!empty.is_null());
    unsafe { rspyts_free(empty, 0) };
}

#[test]
fn manifest_export_matches_registry_build() {
    let env = grab(rspyts_manifest());
    let expected = serde_json::to_value(rspyts_core::registry::build_manifest(
        "rspyts",
        env!("CARGO_PKG_VERSION"),
    ))
    .unwrap();
    assert_eq!(*expect_ok(&env), expected);
    assert!(env.tail.is_empty());
}

// ---------------------------------------------------------------------------
// Free-function shims
// ---------------------------------------------------------------------------

#[test]
fn zero_param_fn_accepts_empty_object_and_empty_payload() {
    let body = args(&json!({}));
    let env = grab(unsafe { rspyts_fn__zero_params(body.as_ptr(), body.len()) });
    assert_eq!(*expect_ok(&env), json!(42));

    // len == 0 is the documented "no args" spelling (ABI §3.1).
    let env = grab(unsafe { rspyts_fn__zero_params(std::ptr::null(), 0) });
    assert_eq!(*expect_ok(&env), json!(42));
}

#[test]
fn unit_return_serializes_as_null() {
    let body = args(&json!({"count": 3}));
    let env = grab(unsafe { rspyts_fn__no_return(body.as_ptr(), body.len()) });
    assert_eq!(*expect_ok(&env), Value::Null);
    assert!(env.tail.is_empty());
}

#[test]
fn slice_params_cross_in_declaration_order_after_the_args_pair() {
    let bytes: [u8; 2] = [1, 2];
    let floats: [f64; 1] = [0.5];
    let ints: [i32; 1] = [100];
    // Plain params travel in the JSON object; slices are (ptr, len) pairs
    // appended in declaration order (ABI §3.1): args, s0=bytes, s1=floats,
    // s2=ints.
    let body = args(&json!({"scaleFactor": 10.0, "offset": 7}));
    let env = grab(unsafe {
        rspyts_fn__mixed_slices(
            body.as_ptr(),
            body.len(),
            bytes.as_ptr(),
            bytes.len(),
            floats.as_ptr(),
            floats.len(),
            ints.as_ptr(),
            ints.len(),
        )
    });
    // (1 + 2) * 10 + 0.5 + 100 + 7
    assert_eq!(*expect_ok(&env), json!(137.5));
}

#[test]
fn empty_slices_may_pass_null_pointers() {
    let body = args(&json!({"scaleFactor": 2.0, "offset": 1}));
    let env = grab(unsafe {
        rspyts_fn__mixed_slices(
            body.as_ptr(),
            body.len(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
        )
    });
    assert_eq!(*expect_ok(&env), json!(1.0));
}

#[test]
fn buf_nested_in_struct_in_vec_returns_via_the_tail() {
    let values: [f32; 5] = [1.0, 2.0, 3.0, 4.0, 5.0];
    let body = args(&json!({"chunkLen": 2}));
    let env = grab(unsafe {
        rspyts_fn__make_chunks(body.as_ptr(), body.len(), values.as_ptr(), values.len())
    });
    let out = expect_ok(&env);

    // Three chunks: [1,2], [3,4], [5]; each Buf<f32> serializes as a
    // placeholder object pointing into the shared tail.
    assert_eq!(
        *out,
        json!([
            {"label": "chunk-0", "samples": {"__rspyts_buf__": {"off": 0, "len": 2, "dt": "f32"}}},
            {"label": "chunk-1", "samples": {"__rspyts_buf__": {"off": 8, "len": 2, "dt": "f32"}}},
            {"label": "chunk-2", "samples": {"__rspyts_buf__": {"off": 16, "len": 1, "dt": "f32"}}},
        ])
    );
    assert_eq!(env.tail.len(), 20);
    let decoded: Vec<f32> = env
        .tail
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    assert_eq!(decoded, values);
}

#[test]
fn unicode_survives_args_and_returns() {
    let body = args(&json!({"text": "héllo 🌍 — ναι"}));
    let env = grab(unsafe { rspyts_fn__decorate(body.as_ptr(), body.len()) });
    assert_eq!(*expect_ok(&env), json!("«héllo 🌍 — ναι» ✓"));
}

#[test]
fn nested_struct_param_round_trips_with_camel_case_wire_names() {
    let body = args(&json!({
        "all": {
            "flag": true,
            "tinyU": 1,
            "smallU": 2,
            "mediumU": 3,
            "tinyI": -1,
            "smallI": -2,
            "mediumI": -3,
            "single": 1.5,
            "double": 2.5,
            "text": "hi",
            "maybeText": "there",
            "numbers": [7, 8],
            "lookup": {"a": 0.5},
            "ordered": {"z": 9},
            "nested": {"note": "deep"},
        }
    }));
    let env = grab(unsafe { rspyts_fn__describe(body.as_ptr(), body.len()) });
    assert_eq!(
        *expect_ok(&env),
        json!("flag=true medium_u=3 text=hi maybe=Some(\"there\") numbers=[7, 8] nested=deep")
    );
}

#[test]
fn deny_unknown_fields_rejects_extras_as_invalid_args() {
    // Unknown key in the args object itself.
    let body = args(&json!({"count": 3, "extra": 1}));
    let env = grab(unsafe { rspyts_fn__no_return(body.as_ptr(), body.len()) });
    let err = expect_err(&env, "invalidArgs");
    assert!(
        err["message"]
            .as_str()
            .unwrap()
            .contains("invalid arguments"),
        "{err:?}"
    );

    // Unknown key inside a bridged struct param.
    let body = args(&json!({
        "all": {
            "flag": true,
            "tinyU": 1,
            "smallU": 2,
            "mediumU": 3,
            "tinyI": -1,
            "smallI": -2,
            "mediumI": -3,
            "single": 1.5,
            "double": 2.5,
            "text": "hi",
            "maybeText": null,
            "numbers": [],
            "lookup": {},
            "ordered": {},
            "nested": {"note": "deep", "unexpected": 1},
        }
    }));
    let env = grab(unsafe { rspyts_fn__describe(body.as_ptr(), body.len()) });
    expect_err(&env, "invalidArgs");
}

#[test]
fn optional_string_param_present_null_and_omitted() {
    let body = args(&json!({"label": "tagged"}));
    let env = grab(unsafe { rspyts_fn__maybe_label(body.as_ptr(), body.len()) });
    assert_eq!(*expect_ok(&env), json!("tagged"));

    let body = args(&json!({"label": null}));
    let env = grab(unsafe { rspyts_fn__maybe_label(body.as_ptr(), body.len()) });
    assert_eq!(*expect_ok(&env), json!("<none>"));

    let body = args(&json!({}));
    let env = grab(unsafe { rspyts_fn__maybe_label(body.as_ptr(), body.len()) });
    assert_eq!(*expect_ok(&env), json!("<none>"));
}

#[test]
fn snake_case_override_controls_the_wire_names() {
    let body = args(&json!({"wire": {"item_count": 3, "max_value": 9.5}}));
    let env = grab(unsafe { rspyts_fn__echo_snake(body.as_ptr(), body.len()) });
    assert_eq!(*expect_ok(&env), json!({"item_count": 3, "max_value": 9.5}));

    // The optional field may be omitted on input; it serializes as null.
    let body = args(&json!({"wire": {"item_count": 1}}));
    let env = grab(unsafe { rspyts_fn__echo_snake(body.as_ptr(), body.len()) });
    assert_eq!(
        *expect_ok(&env),
        json!({"item_count": 1, "max_value": null})
    );

    // camelCase keys must be rejected: the override moved the wire names.
    let body = args(&json!({"wire": {"itemCount": 3}}));
    let env = grab(unsafe { rspyts_fn__echo_snake(body.as_ptr(), body.len()) });
    expect_err(&env, "invalidArgs");
}

// ---------------------------------------------------------------------------
// Enum wire shapes (emitted serde attributes)
// ---------------------------------------------------------------------------

#[test]
fn string_enum_serializes_as_camel_case_strings() {
    assert_eq!(serde_json::to_value(Level::Low).unwrap(), json!("low"));
    assert_eq!(
        serde_json::to_value(Level::MidRange).unwrap(),
        json!("midRange")
    );
    let level: Level = serde_json::from_value(json!("high")).unwrap();
    assert!(matches!(level, Level::High));
    assert!(serde_json::from_value::<Level>(json!("High")).is_err());
}

#[test]
fn data_enum_uses_the_default_type_tag() {
    assert_eq!(
        serde_json::to_value(Shape::Circle { radius_len: 2.5 }).unwrap(),
        json!({"type": "circle", "radiusLen": 2.5})
    );
    let shape: Shape = serde_json::from_value(json!({
        "type": "rect", "width": 3.0, "height": 4.0,
    }))
    .unwrap();
    assert!(matches!(shape, Shape::Rect { width, height } if width == 3.0 && height == 4.0));
}

#[test]
fn data_enum_tag_override_is_honored() {
    assert_eq!(
        serde_json::to_value(Event::Stopped {
            at_ms: 9,
            reason: None,
        })
        .unwrap(),
        json!({"kind": "stopped", "atMs": 9, "reason": null})
    );
    // Optional variant fields may be omitted on input.
    let event: Event = serde_json::from_value(json!({"kind": "stopped", "atMs": 9})).unwrap();
    assert!(matches!(
        event,
        Event::Stopped {
            at_ms: 9,
            reason: None
        }
    ));
    let event: Event = serde_json::from_value(json!({"kind": "started", "atMs": 1})).unwrap();
    assert!(matches!(event, Event::Started { at_ms: 1 }));
}

#[test]
fn struct_serde_round_trips_and_rejects_unknown_fields() {
    let everything = Everything {
        flag: false,
        tiny_u: 8,
        small_u: 16,
        medium_u: 32,
        tiny_i: -8,
        small_i: -16,
        medium_i: -32,
        single: 0.5,
        double: 0.25,
        text: String::from("τ"),
        maybe_text: None,
        numbers: vec![1, 2, 3],
        lookup: HashMap::from([(String::from("k"), 1.5)]),
        ordered: BTreeMap::from([(String::from("a"), 1), (String::from("b"), 2)]),
        nested: NestedInfo {
            note: String::from("n"),
        },
    };
    let value = serde_json::to_value(&everything).unwrap();
    assert_eq!(
        value,
        json!({
            "flag": false,
            "tinyU": 8,
            "smallU": 16,
            "mediumU": 32,
            "tinyI": -8,
            "smallI": -16,
            "mediumI": -32,
            "single": 0.5,
            "double": 0.25,
            "text": "τ",
            "maybeText": null,
            "numbers": [1, 2, 3],
            "lookup": {"k": 1.5},
            "ordered": {"a": 1, "b": 2},
            "nested": {"note": "n"},
        })
    );
    let back: Everything = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(back.ordered.len(), 2);

    let mut with_extra = value;
    with_extra["surprise"] = json!(1);
    assert!(serde_json::from_value::<Everything>(with_extra).is_err());
}

// ---------------------------------------------------------------------------
// Error enums
// ---------------------------------------------------------------------------

#[test]
fn error_enum_maps_variants_to_code_message_and_data() {
    let err = rspyts::BridgeErr::into_bridge_error(FixtureError::Empty);
    assert_eq!(
        serde_json::to_string(&err).unwrap(),
        r#"{"code":"empty","message":"nothing to add"}"#
    );

    let err = rspyts::BridgeErr::into_bridge_error(FixtureError::OverLimit {
        max_allowed: 5,
        attempted: 9,
    });
    // Compared as `Value` because the `data` key order depends on whether
    // serde_json's `preserve_order` feature is unified into the build.
    assert_eq!(
        serde_json::to_value(&err).unwrap(),
        json!({
            "code": "overLimit",
            "message": "adding 9 exceeds the limit 5",
            "data": {"maxAllowed": 5, "attempted": 9},
        })
    );
}

// ---------------------------------------------------------------------------
// Class shims
// ---------------------------------------------------------------------------

fn new_counter(step: i32) -> u64 {
    let body = args(&json!({"step": step}));
    let env = grab(unsafe { rspyts_cls__Counter__new(body.as_ptr(), body.len()) });
    expect_ok(&env).as_u64().expect("handle is a number")
}

fn counter_total(handle: u64) -> Env {
    let body = args(&json!({}));
    grab(unsafe { rspyts_cls__Counter__total(handle, body.as_ptr(), body.len()) })
}

#[test]
fn class_lifecycle_methods_result_paths_and_drop_semantics() {
    let first = new_counter(2);
    let second = new_counter(10);
    assert_ne!(first, second, "handles are unique");

    // &mut self through the shim, per instance.
    let body = args(&json!({"times": 3}));
    let env = grab(unsafe { rspyts_cls__Counter__advance(first, body.as_ptr(), body.len()) });
    assert_eq!(*expect_ok(&env), json!(6));
    let body = args(&json!({"times": 1}));
    let env = grab(unsafe { rspyts_cls__Counter__advance(second, body.as_ptr(), body.len()) });
    assert_eq!(*expect_ok(&env), json!(10));

    // &self sees each instance's own state.
    assert_eq!(*expect_ok(&counter_total(first)), json!(6));
    assert_eq!(*expect_ok(&counter_total(second)), json!(10));

    // Result method: Ok path.
    let body = args(&json!({"amount": 4, "maxAllowed": 100}));
    let env = grab(unsafe { rspyts_cls__Counter__checked_add(first, body.as_ptr(), body.len()) });
    assert_eq!(*expect_ok(&env), json!(10));

    // Result method: fieldless error variant.
    let body = args(&json!({"amount": 0, "maxAllowed": 100}));
    let env = grab(unsafe { rspyts_cls__Counter__checked_add(first, body.as_ptr(), body.len()) });
    let err = expect_err(&env, "empty");
    assert_eq!(err["message"], "nothing to add");
    assert!(err.get("data").is_none());

    // Result method: fielded error variant with a data object.
    let body = args(&json!({"amount": 9, "maxAllowed": 5}));
    let env = grab(unsafe { rspyts_cls__Counter__checked_add(first, body.as_ptr(), body.len()) });
    let err = expect_err(&env, "overLimit");
    assert_eq!(err["message"], "adding 9 exceeds the limit 5");
    assert_eq!(err["data"], json!({"attempted": 9, "maxAllowed": 5}));

    // Drop the first instance; the second keeps working.
    rspyts_cls__Counter__drop(first);
    assert_eq!(*expect_ok(&counter_total(second)), json!(10));

    // Stale handle after drop.
    expect_err(&counter_total(first), "staleHandle");

    // Drop is idempotent — including on never-issued handles.
    rspyts_cls__Counter__drop(first);
    rspyts_cls__Counter__drop(u64::MAX);

    rspyts_cls__Counter__drop(second);
    expect_err(&counter_total(second), "staleHandle");
}

// ---------------------------------------------------------------------------
// String enums through the shim
// ---------------------------------------------------------------------------

#[test]
fn string_enum_round_trips_through_the_shim_as_camel_case() {
    assert_eq!(
        serde_json::to_value(Channel::ChinEmg).unwrap(),
        json!("chinEmg")
    );
    assert_eq!(
        serde_json::to_value(Channel::LegMovement).unwrap(),
        json!("legMovement")
    );

    let body = args(&json!({"channel": "chinEmg"}));
    let env = grab(unsafe { rspyts_fn__echo_channel(body.as_ptr(), body.len()) });
    assert_eq!(*expect_ok(&env), json!("chinEmg"));

    // camelCase IS the contract — no other spelling exists on the wire.
    let body = args(&json!({"channel": "chin_emg"}));
    let env = grab(unsafe { rspyts_fn__echo_channel(body.as_ptr(), body.len()) });
    expect_err(&env, "invalidArgs");
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

#[test]
fn const_decls_capture_name_ty_and_serialized_value() {
    use rspyts_core::ir::Ty;
    let manifest = rspyts_core::registry::build_manifest("expansion-fixture", "0.0.0");
    let find = |name: &str| {
        manifest
            .constants
            .iter()
            .find(|c| c.name == name)
            .expect("const registered")
    };

    let banner = find("BANNER");
    assert_eq!(banner.ty, Ty::String);
    assert_eq!(banner.value, json!("rspyts expansion fixture"));
    assert_eq!(banner.origin, "rspyts");

    let labels = find("CHANNEL_LABELS");
    assert_eq!(
        labels.ty,
        Ty::List {
            inner: Box::new(Ty::String),
        }
    );
    assert_eq!(labels.value, json!(["chin_emg", "leg_movement"]));

    let gain = find("DEFAULT_GAIN");
    assert_eq!(gain.ty, Ty::F64);
    assert_eq!(gain.value, json!(1.25));
}

// ---------------------------------------------------------------------------
// Target scoping and Json passthrough
// ---------------------------------------------------------------------------

#[test]
fn target_scoping_lands_in_the_manifest() {
    use rspyts_core::ir::Target;
    let manifest = rspyts_core::registry::build_manifest("expansion-fixture", "0.0.0");

    let scoped = manifest
        .functions
        .iter()
        .find(|f| f.name == "python_only")
        .unwrap();
    assert_eq!(scoped.targets, vec![Target::Python]);
    let unscoped = manifest
        .functions
        .iter()
        .find(|f| f.name == "decorate")
        .unwrap();
    assert_eq!(unscoped.targets, Target::all());

    let session = manifest
        .classes
        .iter()
        .find(|c| c.name == "Session")
        .unwrap();
    let probe = session.statics.iter().find(|s| s.name == "probe").unwrap();
    assert_eq!(probe.targets, vec![Target::Typescript]);
}

#[test]
fn impl_level_target_is_inherited_and_overridable() {
    use rspyts_core::ir::Target;
    let manifest = rspyts_core::registry::build_manifest("expansion-fixture", "0.0.0");
    let telemetry = manifest
        .classes
        .iter()
        .find(|c| c.name == "Telemetry")
        .unwrap();

    // The constructor is never scoped: impl-level targets must not
    // silently affect construction.
    assert!(telemetry.constructor.is_some());

    let method = |name: &str| telemetry.methods.iter().find(|m| m.name == name).unwrap();
    // Unmarked members inherit the impl-level default...
    assert_eq!(method("record").targets, vec![Target::Python]);
    assert_eq!(method("hits").targets, vec![Target::Python]);
    // ...a member's own target wins...
    assert_eq!(method("probe").targets, vec![Target::Typescript]);
    // ...and statics inherit exactly like methods.
    let flush = telemetry
        .statics
        .iter()
        .find(|s| s.name == "flush_interval_ms")
        .unwrap();
    assert_eq!(flush.targets, vec![Target::Python]);
}

#[test]
fn json_passthrough_round_trips_arbitrary_shapes() {
    let payload = json!({"nested": [1, "two", {"three": null}], "flag": true});
    let body = args(&json!({"value": payload}));
    let env = grab(unsafe { rspyts_fn__echo_json(body.as_ptr(), body.len()) });
    assert_eq!(*expect_ok(&env), payload);

    // The shim (target-scoped functions always keep their shim) still works.
    let body = args(&json!({"x": 21}));
    let env = grab(unsafe { rspyts_fn__python_only(body.as_ptr(), body.len()) });
    assert_eq!(*expect_ok(&env), json!(42));
}

// ---------------------------------------------------------------------------
// Statics and factories
// ---------------------------------------------------------------------------

#[test]
fn static_shims_take_no_handle_and_encode_ordinarily() {
    let body = args(&json!({}));
    let env = grab(unsafe { rspyts_cls__Counter__suggested_step(body.as_ptr(), body.len()) });
    assert_eq!(*expect_ok(&env), json!(4));

    let env = grab(unsafe { rspyts_cls__Session__probe(body.as_ptr(), body.len()) });
    assert_eq!(*expect_ok(&env), json!(7));
}

#[test]
fn factory_only_class_lifecycle_and_stale_handles() {
    // Error path: the factory's Result surfaces like any fallible shim.
    let body = args(&json!({"id": 0}));
    let env = grab(unsafe { rspyts_cls__Session__open(body.as_ptr(), body.len()) });
    expect_err(&env, "empty");

    let body = args(&json!({"id": 5}));
    let env = grab(unsafe { rspyts_cls__Session__open(body.as_ptr(), body.len()) });
    let first = expect_ok(&env).as_u64().expect("handle is a number");
    let body = args(&json!({"id": 6}));
    let env = grab(unsafe { rspyts_cls__Session__open(body.as_ptr(), body.len()) });
    let second = expect_ok(&env).as_u64().expect("handle is a number");
    assert_ne!(first, second, "factory handles are unique");

    let empty = args(&json!({}));
    let env = grab(unsafe { rspyts_cls__Session__id(first, empty.as_ptr(), empty.len()) });
    assert_eq!(*expect_ok(&env), json!(5));

    // Factory-made handles drop exactly like constructor-made ones.
    rspyts_cls__Session__drop(first);
    let env = grab(unsafe { rspyts_cls__Session__id(first, empty.as_ptr(), empty.len()) });
    expect_err(&env, "staleHandle");
    // Idempotent, including on never-issued handles.
    rspyts_cls__Session__drop(first);
    rspyts_cls__Session__drop(u64::MAX);

    let env = grab(unsafe { rspyts_cls__Session__id(second, empty.as_ptr(), empty.len()) });
    assert_eq!(*expect_ok(&env), json!(6));
    rspyts_cls__Session__drop(second);
}

// ---------------------------------------------------------------------------
// Manifest snapshot
// ---------------------------------------------------------------------------

#[test]
fn manifest_snapshot_covers_every_registered_item() {
    let manifest = rspyts_core::registry::build_manifest("expansion-fixture", "0.0.0");
    let actual = serde_json::to_value(&manifest).unwrap();
    let expected = json!({
        "abi": "0.1",
        "crateName": "expansion-fixture",
        "crateVersion": "0.0.0",
        "types": [
            {
                "kind": "stringEnum",
                "name": "Channel",
                "docs": "EMG channels.",
                "origin": "rspyts",
                "variants": [
                    {"name": "ChinEmg", "wireName": "chinEmg", "docs": "Chin electromyography."},
                    {"name": "LegMovement", "wireName": "legMovement", "docs": ""},
                ],
            },
            {
                "kind": "struct",
                "name": "Chunk",
                "docs": "A labeled chunk of samples.",
                "origin": "rspyts",
                "fields": [
                    {"name": "label", "wireName": "label", "docs": "", "ty": {"kind": "string"}, "optional": false},
                    {"name": "samples", "wireName": "samples", "docs": "Raw samples for this chunk.", "ty": {"kind": "buf", "dt": "f32"}, "optional": false},
                ],
            },
            {
                "kind": "enum",
                "name": "Event",
                "docs": "Something that happened.",
                "origin": "rspyts",
                "tag": "kind",
                "variants": [
                    {
                        "name": "Started",
                        "wireName": "started",
                        "docs": "",
                        "fields": [
                            {"name": "at_ms", "wireName": "atMs", "docs": "", "ty": {"kind": "u32"}, "optional": false},
                        ],
                    },
                    {
                        "name": "Stopped",
                        "wireName": "stopped",
                        "docs": "",
                        "fields": [
                            {"name": "at_ms", "wireName": "atMs", "docs": "", "ty": {"kind": "u32"}, "optional": false},
                            {"name": "reason", "wireName": "reason", "docs": "", "ty": {"kind": "option", "inner": {"kind": "string"}}, "optional": true},
                        ],
                    },
                ],
            },
            {
                "kind": "struct",
                "name": "Everything",
                "docs": "Every scalar and container in one shape.\n\nSecond doc paragraph.",
                "origin": "rspyts",
                "fields": [
                    {"name": "flag", "wireName": "flag", "docs": "", "ty": {"kind": "bool"}, "optional": false},
                    {"name": "tiny_u", "wireName": "tinyU", "docs": "", "ty": {"kind": "u8"}, "optional": false},
                    {"name": "small_u", "wireName": "smallU", "docs": "", "ty": {"kind": "u16"}, "optional": false},
                    {"name": "medium_u", "wireName": "mediumU", "docs": "", "ty": {"kind": "u32"}, "optional": false},
                    {"name": "tiny_i", "wireName": "tinyI", "docs": "", "ty": {"kind": "i8"}, "optional": false},
                    {"name": "small_i", "wireName": "smallI", "docs": "", "ty": {"kind": "i16"}, "optional": false},
                    {"name": "medium_i", "wireName": "mediumI", "docs": "", "ty": {"kind": "i32"}, "optional": false},
                    {"name": "single", "wireName": "single", "docs": "", "ty": {"kind": "f32"}, "optional": false},
                    {"name": "double", "wireName": "double", "docs": "", "ty": {"kind": "f64"}, "optional": false},
                    {"name": "text", "wireName": "text", "docs": "", "ty": {"kind": "string"}, "optional": false},
                    {"name": "maybe_text", "wireName": "maybeText", "docs": "Present only sometimes.", "ty": {"kind": "option", "inner": {"kind": "string"}}, "optional": true},
                    {"name": "numbers", "wireName": "numbers", "docs": "", "ty": {"kind": "list", "inner": {"kind": "i32"}}, "optional": false},
                    {"name": "lookup", "wireName": "lookup", "docs": "", "ty": {"kind": "map", "value": {"kind": "f64"}}, "optional": false},
                    {"name": "ordered", "wireName": "ordered", "docs": "", "ty": {"kind": "map", "value": {"kind": "u32"}}, "optional": false},
                    {"name": "nested", "wireName": "nested", "docs": "", "ty": {"kind": "ref", "name": "NestedInfo"}, "optional": false},
                ],
            },
            {
                "kind": "errorEnum",
                "name": "FixtureError",
                "docs": "Failure modes of the fixture class.",
                "origin": "rspyts",
                "variants": [
                    {"name": "Empty", "wireCode": "empty", "docs": "Nothing to add.", "fields": []},
                    {
                        "name": "OverLimit",
                        "wireCode": "overLimit",
                        "docs": "The counter would exceed its limit.",
                        "fields": [
                            {"name": "max_allowed", "wireName": "maxAllowed", "docs": "", "ty": {"kind": "i32"}, "optional": false},
                            {"name": "attempted", "wireName": "attempted", "docs": "", "ty": {"kind": "i32"}, "optional": false},
                        ],
                    },
                ],
            },
            {
                "kind": "stringEnum",
                "name": "Level",
                "docs": "Severity levels.",
                "origin": "rspyts",
                "variants": [
                    {"name": "Low", "wireName": "low", "docs": "Lowest severity."},
                    {"name": "MidRange", "wireName": "midRange", "docs": ""},
                    {"name": "High", "wireName": "high", "docs": ""},
                ],
            },
            {
                "kind": "struct",
                "name": "NestedInfo",
                "docs": "A nested payload.",
                "origin": "rspyts",
                "fields": [
                    {"name": "note", "wireName": "note", "docs": "Free-form note.", "ty": {"kind": "string"}, "optional": false},
                ],
            },
            {
                "kind": "enum",
                "name": "Shape",
                "docs": "A geometric shape.",
                "origin": "rspyts",
                "tag": "type",
                "variants": [
                    {
                        "name": "Circle",
                        "wireName": "circle",
                        "docs": "A circle.",
                        "fields": [
                            {"name": "radius_len", "wireName": "radiusLen", "docs": "", "ty": {"kind": "f64"}, "optional": false},
                        ],
                    },
                    {
                        "name": "Rect",
                        "wireName": "rect",
                        "docs": "",
                        "fields": [
                            {"name": "width", "wireName": "width", "docs": "", "ty": {"kind": "f64"}, "optional": false},
                            {"name": "height", "wireName": "height", "docs": "", "ty": {"kind": "f64"}, "optional": false},
                        ],
                    },
                ],
            },
            {
                "kind": "struct",
                "name": "SnakeWire",
                "docs": "Snake-cased wire shape.",
                "origin": "rspyts",
                "fields": [
                    {"name": "item_count", "wireName": "item_count", "docs": "", "ty": {"kind": "u32"}, "optional": false},
                    {"name": "max_value", "wireName": "max_value", "docs": "Optional maximum.", "ty": {"kind": "option", "inner": {"kind": "f64"}}, "optional": true},
                ],
            },
        ],
        "constants": [
            {
                "name": "BANNER",
                "docs": "Library banner shown by clients.",
                "origin": "rspyts",
                "ty": {"kind": "string"},
                "value": "rspyts expansion fixture",
            },
            {
                "name": "CHANNEL_LABELS",
                "docs": "Channel labels, in montage order.",
                "origin": "rspyts",
                "ty": {"kind": "list", "inner": {"kind": "string"}},
                "value": ["chin_emg", "leg_movement"],
            },
            {
                "name": "DEFAULT_GAIN",
                "docs": "Gain applied when none is configured.",
                "origin": "rspyts",
                "ty": {"kind": "f64"},
                "value": 1.25,
            },
        ],
        "functions": [
            {
                "name": "decorate",
                "docs": "Wrap `text` in guillemets with a check mark.",
                "params": [
                    {"name": "text", "wireName": "text", "ty": {"kind": "string"}},
                ],
                "ret": {"kind": "string"},
                "err": null,
                "targets": ["python", "typescript"],
            },
            {
                "name": "describe",
                "docs": "Render a short summary of `all`.",
                "params": [
                    {"name": "all", "wireName": "all", "ty": {"kind": "ref", "name": "Everything"}},
                ],
                "ret": {"kind": "string"},
                "err": null,
                "targets": ["python", "typescript"],
            },
            {
                "name": "echo_channel",
                "docs": "Echo a channel.",
                "params": [
                    {"name": "channel", "wireName": "channel", "ty": {"kind": "ref", "name": "Channel"}},
                ],
                "ret": {"kind": "ref", "name": "Channel"},
                "err": null,
                "targets": ["python", "typescript"],
            },
            {
                "name": "echo_json",
                "docs": "Echo arbitrary JSON.",
                "params": [
                    {"name": "value", "wireName": "value", "ty": {"kind": "json"}},
                ],
                "ret": {"kind": "json"},
                "err": null,
                "targets": ["python", "typescript"],
            },
            {
                "name": "echo_snake",
                "docs": "Round-trip a snake-cased struct.",
                "params": [
                    {"name": "wire", "wireName": "wire", "ty": {"kind": "ref", "name": "SnakeWire"}},
                ],
                "ret": {"kind": "ref", "name": "SnakeWire"},
                "err": null,
                "targets": ["python", "typescript"],
            },
            {
                "name": "make_chunks",
                "docs": "Split `values` into chunks of `chunk_len` samples.",
                "params": [
                    {"name": "values", "wireName": "values", "ty": {"kind": "slice", "dt": "f32"}},
                    {"name": "chunk_len", "wireName": "chunkLen", "ty": {"kind": "u32"}},
                ],
                "ret": {"kind": "list", "inner": {"kind": "ref", "name": "Chunk"}},
                "err": null,
                "targets": ["python", "typescript"],
            },
            {
                "name": "maybe_label",
                "docs": "Echo `label`, or a placeholder when absent.",
                "params": [
                    {"name": "label", "wireName": "label", "ty": {"kind": "option", "inner": {"kind": "string"}}},
                ],
                "ret": {"kind": "string"},
                "err": null,
                "targets": ["python", "typescript"],
            },
            {
                "name": "mixed_slices",
                "docs": "Combine three buffers of different dtypes with two plain params.",
                "params": [
                    {"name": "bytes", "wireName": "bytes", "ty": {"kind": "slice", "dt": "u8"}},
                    {"name": "scale_factor", "wireName": "scaleFactor", "ty": {"kind": "f64"}},
                    {"name": "floats", "wireName": "floats", "ty": {"kind": "slice", "dt": "f64"}},
                    {"name": "offset", "wireName": "offset", "ty": {"kind": "i32"}},
                    {"name": "ints", "wireName": "ints", "ty": {"kind": "slice", "dt": "i32"}},
                ],
                "ret": {"kind": "f64"},
                "err": null,
                "targets": ["python", "typescript"],
            },
            {
                "name": "no_return",
                "docs": "Consume a value, produce nothing.",
                "params": [
                    {"name": "count", "wireName": "count", "ty": {"kind": "u32"}},
                ],
                "ret": {"kind": "unit"},
                "err": null,
                "targets": ["python", "typescript"],
            },
            {
                "name": "python_only",
                "docs": "Double a value; Python-only surface.",
                "params": [
                    {"name": "x", "wireName": "x", "ty": {"kind": "u32"}},
                ],
                "ret": {"kind": "u32"},
                "err": null,
                "targets": ["python"],
            },
            {
                "name": "zero_params",
                "docs": "The answer, no questions asked.",
                "params": [],
                "ret": {"kind": "u32"},
                "err": null,
                "targets": ["python", "typescript"],
            },
        ],
        "classes": [
            {
                "name": "Counter",
                "docs": "Counter methods exposed to foreign callers.",
                "constructor": {
                    "docs": "Create a counter advancing by `step`.",
                    "params": [
                        {"name": "step", "wireName": "step", "ty": {"kind": "i32"}},
                    ],
                    "err": null,
                },
                "methods": [
                    {
                        "name": "total",
                        "docs": "Current total.",
                        "mutable": false,
                        "params": [],
                        "ret": {"kind": "i32"},
                        "err": null,
                        "targets": ["python", "typescript"],
                    },
                    {
                        "name": "advance",
                        "docs": "Advance `times` steps and return the new total.",
                        "mutable": true,
                        "params": [
                            {"name": "times", "wireName": "times", "ty": {"kind": "i32"}},
                        ],
                        "ret": {"kind": "i32"},
                        "err": null,
                        "targets": ["python", "typescript"],
                    },
                    {
                        "name": "checked_add",
                        "docs": "Add `amount` directly, failing on zero or past `max_allowed`.",
                        "mutable": true,
                        "params": [
                            {"name": "amount", "wireName": "amount", "ty": {"kind": "i32"}},
                            {"name": "max_allowed", "wireName": "maxAllowed", "ty": {"kind": "i32"}},
                        ],
                        "ret": {"kind": "i32"},
                        "err": "FixtureError",
                        "targets": ["python", "typescript"],
                    },
                ],
                "statics": [
                    {
                        "name": "suggested_step",
                        "docs": "The step new counters default to.",
                        "params": [],
                        "ret": {"kind": "u32"},
                        "err": null,
                        "returnsSelf": false,
                        "targets": ["python", "typescript"],
                    },
                ],
            },
            {
                "name": "Session",
                "docs": "Factory-only class: constructed exclusively through `open`.",
                "constructor": null,
                "methods": [
                    {
                        "name": "id",
                        "docs": "This session's id.",
                        "mutable": false,
                        "params": [],
                        "ret": {"kind": "u32"},
                        "err": null,
                        "targets": ["python", "typescript"],
                    },
                ],
                "statics": [
                    {
                        "name": "open",
                        "docs": "Open the session `id`; zero is invalid.",
                        "params": [
                            {"name": "id", "wireName": "id", "ty": {"kind": "u32"}},
                        ],
                        "ret": {"kind": "unit"},
                        "err": "FixtureError",
                        "returnsSelf": true,
                        "targets": ["python", "typescript"],
                    },
                    {
                        "name": "probe",
                        "docs": "Probe the backend; TypeScript-only surface.",
                        "params": [],
                        "ret": {"kind": "u32"},
                        "err": null,
                        "returnsSelf": false,
                        "targets": ["typescript"],
                    },
                ],
            },
            {
                "name": "Telemetry",
                "docs": "Python-scoped impl: members inherit the target unless they override.",
                "constructor": {
                    "docs": "Start with no hits.",
                    "params": [],
                    "err": null,
                },
                "methods": [
                    {
                        "name": "record",
                        "docs": "Count one hit and return the total.",
                        "mutable": true,
                        "params": [],
                        "ret": {"kind": "u32"},
                        "err": null,
                        "targets": ["python"],
                    },
                    {
                        "name": "hits",
                        "docs": "Total hits so far.",
                        "mutable": false,
                        "params": [],
                        "ret": {"kind": "u32"},
                        "err": null,
                        "targets": ["python"],
                    },
                    {
                        "name": "probe",
                        "docs": "Override: TypeScript-only despite the impl-level Python default.",
                        "mutable": false,
                        "params": [],
                        "ret": {"kind": "u32"},
                        "err": null,
                        "targets": ["typescript"],
                    },
                ],
                "statics": [
                    {
                        "name": "flush_interval_ms",
                        "docs": "Statics inherit the impl-level default too.",
                        "params": [],
                        "ret": {"kind": "u32"},
                        "err": null,
                        "returnsSelf": false,
                        "targets": ["python"],
                    },
                ],
            },
        ],
    });
    assert_eq!(
        actual,
        expected,
        "manifest snapshot drifted:\n{}",
        serde_json::to_string_pretty(&actual).unwrap()
    );
}
