//! Proc macros for the [rspyts](https://github.com/2xf-org/rspyts) bridge.
//!
//! This crate defines the [`bridge`] attribute macro — the single
//! user-facing entry point of rspyts. It is an implementation detail of
//! the `rspyts` facade crate: depend on `rspyts` and write
//! `use rspyts::bridge;`, never on `rspyts-macros` directly. Every path in
//! emitted code goes through `::rspyts::__private::…`, so expansions only
//! compile inside crates that depend on the facade.
//!
//! The normative contracts implemented here are `docs/design/abi.md`
//! (symbols, envelopes, argument passing) and `docs/design/type-system.md`
//! (what may cross the bridge). If this code and those documents disagree,
//! this code is wrong.

mod attrs;
mod casing;
mod classes;
mod consts;
mod docs;
mod emit;
mod functions;
mod serde_reflect;
mod sig;
mod types;

use proc_macro::TokenStream;

/// Bridge a Rust definition into Python and TypeScript.
///
/// `#[bridge]` covers six item forms. In every case the original item is
/// re-emitted (structurally untouched apart from added serde attributes),
/// followed by the plumbing the bridge needs: `extern "C"` shims and an
/// inventory registration that feeds the `rspyts_manifest()` export
/// emitted by `rspyts::export!()`.
///
/// # Data structs
///
/// ```ignore
/// #[bridge]
/// /// Options controlling value processing.
/// pub struct QueryOptions {
///     /// Minimum value to include.
///     pub minimum_value: f64,
///     pub tolerance: Option<f64>,
/// }
/// ```
///
/// Adds `#[derive(Serialize, Deserialize)]` with
/// `#[serde(rename_all = "camelCase", deny_unknown_fields)]` — wire field
/// names are camelCase (`minimumValue`) and unknown fields are rejected on
/// deserialize. Override the casing per struct with
/// `#[bridge(rename_all = "snake_case")]`. Doc comments propagate to
/// Python docstrings, TypeScript doc comments, and JSON Schema
/// descriptions. `Option<T>` fields are marked optional in the generated
/// surfaces (null default). Rejected: generics, lifetimes, tuple structs,
/// unit structs, non-`pub` fields.
///
/// A type that already derives Serde can instead use adoption mode:
///
/// ```ignore
/// #[bridge(serde)]
/// #[derive(serde::Serialize, serde::Deserialize)]
/// #[serde(rename_all = "kebab-case", deny_unknown_fields)]
/// pub struct ExistingOptions {
///     pub batch_size: u32,
/// }
/// ```
///
/// Adoption injects neither derives nor Serde attributes. rspyts reflects
/// Serde's `rename`, `rename_all`, `tag`, `transparent`, and
/// `deny_unknown_fields` subset into the manifest and rejects every
/// unmodeled shape-changing key at its definition site. Both derives are
/// required syntactically and by trait bound. Use `rspyts::Json` for an
/// intentionally schemaless custom codec.
///
/// # Enums
///
/// All variants fieldless → a **string enum**, serialized as the camelCase
/// variant name:
///
/// ```ignore
/// #[bridge]
/// pub enum Severity { Low, Medium, High }   // "low" | "medium" | "high"
/// ```
///
/// Any variant with named fields → an internally tagged **data enum**;
/// every variant must then use named fields. The discriminator key
/// defaults to `"type"`, overridable with `#[bridge(tag = "kind")]`:
///
/// ```ignore
/// #[bridge(tag = "kind")]
/// pub enum ValueEvent {
///     Accepted { index: u32, value: f64 },  // {"kind":"accepted","index":…}
///     Rejected { index: u32 },
/// }
/// ```
///
/// Rejected: tuple variants, and mixing fieldless with data variants.
///
/// # Error enums — `#[bridge(error)]`
///
/// ```ignore
/// #[bridge(error)]
/// pub enum QueryError {
///     InvalidBatchSize,
///     BatchTooLarge { max: u32 },
/// }
/// impl std::fmt::Display for QueryError { /* … */ }
/// ```
///
/// Derives [`BridgeErr`](../rspyts/trait.BridgeErr.html): the camelCase
/// variant name becomes the error `code`, the `Display` string the
/// `message`, and named fields the `data` object (camelCase keys). The
/// enum must implement `Display`; no serde derives are added — error
/// enums surface as exception/error classes, never as data shapes.
///
/// # Constants
///
/// ```ignore
/// #[bridge]
/// /// Default number of values processed at once.
/// pub const DEFAULT_BATCH_SIZE: u32 = 256;
///
/// #[bridge]
/// pub const SUPPORTED_FORMATS: &[&str] = &["csv", "json"];
/// ```
///
/// The const is re-emitted unchanged; its value is captured with
/// `serde_json::to_value` when the manifest is built and projected as a
/// real importable constant in both languages, keeping its
/// SCREAMING_SNAKE_CASE name.
///
/// Accepted const types — exactly these:
///
/// - **scalars**: `bool`, `u8`/`u16`/`u32`, `i8`/`i16`/`i32`, `f32`/`f64`;
/// - **`&'static str`** (a `String` on the wire — `const String` is
///   impossible in Rust, so the borrowed form is special-cased);
/// - **arrays and slices of supported types**: `[T; N]`,
///   `&'static [T]`, including `&'static [&'static str]`, nested
///   arbitrarily — each maps to a list;
/// - **any owned `Bridged` + `Serialize` type constructible in const
///   context** (e.g. a `#[bridge]` struct with a `const`-buildable shape).
///
/// Everything else — references other than the forms above, raw pointers,
/// function pointers, trait objects — is rejected at expansion time.
///
/// # Free functions
///
/// ```ignore
/// #[bridge]
/// /// Process a buffer of numeric values.
/// pub fn process_values(
///     values: &[f64],                         // borrowed slice param: (ptr, len)
///     batch_size: u32,                        // plain param: JSON args object
///     options: &QueryOptions,                 // plain param, deserialized owned
/// ) -> Result<ProcessingReport, QueryError> { /* … */ }
/// ```
///
/// Emits `extern "C" fn rspyts_fn__process_values(args_ptr, args_len,
/// s0_ptr, s0_len) -> *mut u8` (ABI §3.1). Parameters written as slices of
/// any supported numeric dtype cross as raw `(ptr, len)`
/// pairs; everything else travels in one ABI-2 request envelope keyed by the
/// camelCase parameter name. Owned `Buf<T>` and `Bytes` values use binary
/// attachments in that envelope. `&T` and `&str` parameters deserialize to
/// owned values and are re-borrowed for the call. Return `T`, `()`, or —
/// written literally — `Result<T, E>` where `E: BridgeErr`. Every shim
/// catches panics; nothing unwinds across the boundary. Rejected:
/// `async fn`, generics, non-identifier parameter patterns, and unsupported
/// borrowed types. Use `&[T]` for a borrowed top-level numeric input and
/// `Buf<T>` for an owned or nested numeric value.
///
/// # Classes — `#[bridge]` on an impl block
///
/// ```ignore
/// pub struct RunningStats { /* private state — NOT #[bridge]-annotated */ }
///
/// #[bridge]
/// impl RunningStats {
///     #[bridge(constructor)]
///     pub fn new(window: u32) -> Self { /* … */ }
///     pub fn push(&mut self, chunk: &[f64]) { /* … */ }
///     pub fn snapshot(&self) -> Statistics { /* … */ }
/// }
/// ```
///
/// The type becomes an **opaque class**: state lives in Rust, foreign
/// code holds a `u64` handle allocated from a per-type slab. The struct
/// itself must NOT also carry `#[bridge]` — a type is data or a class,
/// never both. At most one method carries `#[bridge(constructor)]`; it
/// takes no `self` and returns `Self` or `Result<Self, E>`. Every other
/// unmarked method takes `&self` or `&mut self` (`self` by value is
/// rejected) with parameters and returns as for free functions. Emitted
/// symbols: `rspyts_cls__{Type}__new`, `rspyts_cls__{Type}__{method}`
/// (handle first), and the idempotent `rspyts_cls__{Type}__drop`. Method
/// calls lock the object for their duration; a dropped handle yields a
/// `staleHandle` error.
///
/// ## Statics and factories — `#[bridge(static)]`
///
/// ```ignore
/// #[bridge]
/// impl Session {
///     /// Open a data file.
///     #[bridge(static)]
///     pub fn open(path: &str) -> Result<Self, IoError> { /* … */ }
///
///     /// The library's default window length.
///     #[bridge(static)]
///     pub fn default_window() -> u32 { 512 }
///
///     pub fn progress(&self) -> f64 { /* … */ }
/// }
/// ```
///
/// A `#[bridge(static)]` method takes no `self` and gets the shim
/// `rspyts_cls__{Type}__{name}` *without* a handle parameter. When it
/// returns `Self` or `Result<Self, E>` (written literally) it is a
/// **factory**: the instance is inserted into the slab and the fresh
/// handle returned, exactly like the constructor — so a class may be
/// factory-only, with no `#[bridge(constructor)]` at all. A class with
/// neither a constructor nor a `Self`-returning static is rejected.
/// Statics and methods share one name space; `new` and `drop` stay
/// reserved.
///
/// # Target scoping — `#[bridge(target = "…")]`
///
/// Free functions, methods, and statics (not types or constructors) can be
/// limited to a single projection:
///
/// ```ignore
/// #[bridge(target = "python")]
/// pub fn as_numpy_layout(samples: &[f64]) -> Buf<f64> { /* … */ }
/// ```
///
/// The shim always exists; the emitters simply skip the function when
/// generating the other language. Combinable:
/// `#[bridge(static, target = "python")]`.
///
/// On an impl block, `#[bridge(target = "…")]` sets the default for every
/// method and static in the block; a member carrying its own `target`
/// overrides the default:
///
/// ```ignore
/// #[bridge(target = "python")]
/// impl Telemetry {
///     #[bridge(constructor)]
///     pub fn new() -> Self { /* … */ }             // never scoped
///     pub fn record(&mut self) -> u32 { /* … */ }  // python (inherited)
///     #[bridge(target = "typescript")]
///     pub fn probe(&self) -> u32 { /* … */ }       // typescript (override)
/// }
/// ```
///
/// The impl-level target applies to methods and statics only: the class
/// itself — its existence and its constructor — remains in both
/// projections. There is no class-level hiding.
///
/// # Arguments
///
/// | Argument | Applies to | Effect |
/// |---|---|---|
/// | *(none)* | struct, enum, fn, const, impl | default bridging |
/// | `error` | enum | derive `BridgeErr` instead of data serde |
/// | `serde` | struct, data enum | adopt existing Serde derives and reflected naming metadata |
/// | `tag = "…"` | data enum | discriminator key (default `"type"`) |
/// | `rename_all = "…"` | struct | wire casing: `"camelCase"` (default) or `"snake_case"` |
/// | `constructor` | method in a bridged impl | marks the constructor |
/// | `static` | method in a bridged impl | handle-less static; factory when returning `Self` |
/// | `target = "…"` | fn, method, static, impl | emit only for `"python"` or `"typescript"`; on an impl it is the default for all members |
#[proc_macro_attribute]
pub fn bridge(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr: proc_macro2::TokenStream = attr.into();
    let item: proc_macro2::TokenStream = item.into();
    match expand(attr, item.clone()) {
        Ok(tokens) => tokens.into(),
        Err(error) => {
            // Emit the diagnostic *and* the untouched item, so code that
            // references the item reports one real error instead of a
            // cascade of "cannot find type" noise.
            let mut out = error.to_compile_error();
            out.extend(item);
            out.into()
        }
    }
}

fn expand(
    attr: proc_macro2::TokenStream,
    item: proc_macro2::TokenStream,
) -> syn::Result<proc_macro2::TokenStream> {
    let args = attrs::BridgeArgs::parse(attr)?;
    match syn::parse2::<syn::Item>(item)? {
        syn::Item::Struct(item) => types::expand_struct(args, item),
        syn::Item::Enum(item) => types::expand_enum(args, item),
        syn::Item::Fn(item) => functions::expand_fn(args, item),
        syn::Item::Const(item) => consts::expand_const(args, item),
        syn::Item::Impl(item) => classes::expand_impl(args, item),
        other => Err(syn::Error::new_spanned(
            other,
            "#[bridge] supports structs, enums, free functions, consts, and \
             inherent impl blocks",
        )),
    }
}
