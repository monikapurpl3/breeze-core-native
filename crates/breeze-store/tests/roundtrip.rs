//! Byte-for-byte round-trip against Breeze Core's own output.
//!
//! The fixtures in `fixtures/` were produced by Breeze Core's real pydantic
//! models via `model_dump_json(indent=2)` — see `fixtures/generate-fixtures.py`.
//! Each test loads one, serialises it back, and compares **bytes**, not values.
//!
//! Values-only equality is not enough for a drop-in replacement. A server that
//! reads a file correctly and writes it back with keys reordered, `null`s
//! dropped, or a newline appended still works — right up until the user wants to
//! switch back and has nothing byte-identical to compare against. This is the
//! test that makes migration reversible.

use breeze_store::{to_json, AppConfig, DevicesDoc, ProgramsDoc, TimersDoc};

/// Load a fixture, serialise it back, and require the bytes to be identical.
fn assert_byte_identical<T>(name: &str)
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let original = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read fixture {}: {e}", path.display()));

    let parsed: T =
        serde_json::from_str(&original).unwrap_or_else(|e| panic!("{name} did not parse: {e}"));
    let rewritten = to_json(&parsed).unwrap_or_else(|e| panic!("{name} did not serialise: {e}"));

    if rewritten != original {
        // Point at the first divergence rather than dumping both documents,
        // which for programs.json is 1.5 KB of near-identical text.
        let at = original
            .bytes()
            .zip(rewritten.bytes())
            .position(|(a, b)| a != b)
            .unwrap_or_else(|| original.len().min(rewritten.len()));
        let from = at.saturating_sub(60);
        panic!(
            "{name} changed at byte {at}\n  python: ...{}\n  rust:   ...{}",
            &original[from..(at + 60).min(original.len())],
            &rewritten[from..(at + 60).min(rewritten.len())],
        );
    }
}

#[test]
fn config_json_round_trips_byte_for_byte() {
    assert_byte_identical::<AppConfig>("config.json");
}

#[test]
fn devices_json_round_trips_byte_for_byte() {
    // Covers both credential versions, a non-expiring record, and the epoch
    // floats -- the most likely place for Rust and Python to disagree on
    // formatting, since both print the shortest representation that round-trips.
    assert_byte_identical::<DevicesDoc>("devices.json");
}

#[test]
fn programs_json_round_trips_byte_for_byte() {
    // Covers all three program kinds, and is the fixture that pins `id` coming
    // last after pydantic's subclass field ordering.
    assert_byte_identical::<ProgramsDoc>("programs.json");
}

#[test]
fn timers_json_round_trips_byte_for_byte() {
    assert_byte_identical::<TimersDoc>("timers.json");
}

/// The formatting rules the round-trips depend on, asserted directly so a failure
/// says *which* rule broke rather than just "the bytes differ".
#[test]
fn the_formatting_contract_holds() {
    let json = to_json(&TimersDoc::default()).unwrap();
    assert!(!json.ends_with('\n'), "no trailing newline");

    let config: AppConfig = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let text = to_json(&config).unwrap();
    assert!(
        text.contains("\n  \"api_key\""),
        "2-space indent at depth 1"
    );
    assert!(text.contains("\n    {"), "4-space indent inside a list");
    assert!(
        text.contains("\"token\": null"),
        "None must be an explicit null"
    );
    assert!(text.contains(": "), "a space after every key");
}

/// Whole floats keep their `.0`, and fractional ones keep their digits. Python
/// writes `1787000000.0`; dropping the `.0` or rounding the fraction would change
/// devices.json on every save.
#[test]
fn float_formatting_matches_python() {
    let doc: DevicesDoc = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/devices.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let text = to_json(&doc).unwrap();
    assert!(
        text.contains("1787000000.0"),
        "whole float lost its .0: {text}"
    );
    assert!(
        text.contains("1787488496.123456"),
        "fraction changed: {text}"
    );
    assert!(text.contains("1790000000.5"), "half value changed: {text}");
}

/// Timestamps taken from a real `devices.json`, which exposed a bug my own
/// fixtures did not.
///
/// serde_json's default float parser is fast but may be off by one ULP. Reading
/// `2102237115.0631797` gave a double one bit away from the one Python read,
/// which then rendered as `2102237115.0631795` — so the file changed on every
/// save, quietly, for a real installation. Rust's own `str::parse` and Python's
/// `float()` agree exactly; only serde_json's fast path did not.
///
/// The fix is the `float_roundtrip` feature. These are the exact values that
/// caught it, kept as a regression test because nothing else in the suite would
/// notice if the feature were dropped from Cargo.toml.
#[test]
fn real_world_timestamps_survive_to_the_last_bit() {
    for literal in [
        "2102237115.0631797",
        "1787160402.5505733",
        "1787228970.6895123",
    ] {
        let parsed: f64 = serde_json::from_str(literal).unwrap();
        let native: f64 = literal.parse().unwrap();
        assert_eq!(
            parsed.to_bits(),
            native.to_bits(),
            "{literal} parsed to a different double than Rust's own parser              -- is the float_roundtrip feature still enabled?"
        );
        assert_eq!(
            serde_json::to_string(&parsed).unwrap(),
            literal,
            "{literal} did not render back to itself"
        );
    }
}

/// The same values inside a real document shape, so the check covers the actual
/// struct path and not just bare floats.
#[test]
fn a_devices_document_with_awkward_timestamps_round_trips() {
    let doc = r#"{
  "devices": [
    {
      "token_id": "synthetic",
      "label": "regression",
      "auth_version": 2,
      "token_hash": null,
      "public_key": "cHVibGljLWtleS1wbGFjZWhvbGRlcg",
      "created_at": 1787160402.5505733,
      "expires_at": 2102237115.0631797,
      "last_used": 1787228970.6895123
    }
  ]
}"#;
    let parsed: DevicesDoc = serde_json::from_str(doc).unwrap();
    assert_eq!(to_json(&parsed).unwrap(), doc);
}

/// An unknown key must not be silently dropped on load and then lost on save —
/// or rather, if it is dropped, we need to know, because a newer Breeze Core
/// could add a field this build does not understand.
///
/// This documents the current behaviour deliberately: unknown fields *are*
/// dropped, so a downgrade path exists but is lossy. It is asserted so the
/// decision is visible rather than accidental.
#[test]
fn unknown_fields_are_dropped_and_that_is_recorded() {
    let with_extra = r#"{
  "timers": [],
  "some_future_field": 1
}"#;
    let doc: TimersDoc = serde_json::from_str(with_extra).unwrap();
    let back = to_json(&doc).unwrap();
    assert!(
        !back.contains("some_future_field"),
        "if this now round-trips, the migration story improved -- update the docs"
    );
}
