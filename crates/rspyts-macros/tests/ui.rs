//! Compile-fail tests for the `#[bridge]` diagnostics.
//!
//! Each fixture under `tests/ui/` triggers one rejection documented in
//! `docs/design/type-system.md` / `docs/design/abi.md`; the `.stderr`
//! snapshots assert that the macro's own `compile_error!` message (not a
//! rustc-internal cascade) reaches the user. Regenerate snapshots with
//! `TRYBUILD=overwrite cargo test -p rspyts-macros --test ui` and review
//! the diff.

#[test]
fn ui() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/*.rs");
}
