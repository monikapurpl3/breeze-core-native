//! Conformance vectors ported from msmart-ng's own test suite.
//!
//! These are the closest thing the Midea LAN protocol has to a specification:
//! real captured messages with the values a known-good implementation decodes
//! from them. Reimplementing the protocol without them would be guesswork, and
//! the two bugs found while writing this crate — a transposed swing axis and a
//! mistaken AES key size — were both the kind that only a vector catches.
//!
//! Provenance: `msmart/device/AC/test_command.py`, msmart-ng 2026.8.0.

use breeze_proto::ac::response::State;
use breeze_proto::ac::types::{Mode, SwingMode, TemperatureType};
use breeze_proto::ac::{command, response};
use breeze_proto::frame::{self, DeviceType};

/// Decode hex, panicking on malformed input — test-only convenience.
fn hex(s: &str) -> Vec<u8> {
    assert_eq!(s.len() % 2, 0, "hex string must have an even length");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
        .collect()
}

/// Unwrap a full 0xAA message down to its command payload.
fn payload_of(message: &str) -> Vec<u8> {
    let raw = hex(message);
    frame::parse(&raw, DeviceType::AirConditioner)
        .expect("vector must be a valid frame")
        .to_vec()
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// msmart `TestCommand::test_frame`: a GetState query with message id 0x11.
#[test]
fn get_state_command_is_byte_identical() {
    let expected = hex("418100ff03ff00020000000000000000000000000311f4");
    let built = command::get_state(0x11, TemperatureType::Indoor);
    let payload = frame::parse(&built, DeviceType::AirConditioner).unwrap();
    assert_eq!(payload, &expected[..]);
}

// ---------------------------------------------------------------------------
// State responses: whole messages
// ---------------------------------------------------------------------------

/// msmart `test_message_checksum`: a short V3 report whose trailing byte is a CRC
/// rather than the usual checksum. It must still decode.
///
/// Origin: mill1000/midea-ac-py#11.
#[test]
fn short_report_with_a_crc_tail() {
    let p = payload_of("aa1eac00000000000003c0004b1e7f7f000000000069630000000000000d33");
    let s = State::parse(&p).unwrap();
    assert_eq!(s.target_temperature, 27.0);
    assert_eq!(s.indoor_temperature, Some(27.5));
    assert_eq!(s.outdoor_temperature, Some(24.5));
}

/// msmart `test_message_v2`: no outdoor sensor fitted, so it reports the 0xFF
/// sentinel and must come back as `None` rather than a plausible number.
#[test]
fn v2_report_without_an_outdoor_sensor() {
    let p = payload_of("aa22ac00000000000303c0014566000000300010045eff00000000000000000069fdb9");
    let s = State::parse(&p).unwrap();
    assert_eq!(s.target_temperature, 21.0);
    assert_eq!(s.indoor_temperature, Some(22.0));
    assert_eq!(s.outdoor_temperature, None);
}

/// msmart `test_message_v3`.
#[test]
fn v3_report_with_both_sensors() {
    let p = payload_of("aa23ac00000000000303c00145660000003c0010045c6b20000000000000000000020d79");
    let s = State::parse(&p).unwrap();
    assert_eq!(s.target_temperature, 21.0);
    assert_eq!(s.indoor_temperature, Some(21.0));
    assert_eq!(s.outdoor_temperature, Some(28.5));
}

/// msmart `test_message_additional_precision`, whole-message half.
#[test]
fn reports_with_tenth_degree_precision() {
    // (target, indoor, outdoor) -> message
    let cases: [(f32, f32, f32, &str); 3] = [
        (
            24.0,
            24.6,
            9.5,
            "aa23ac00000000000203c00188647f7f000000000063450c0056190000000000000497c3",
        ),
        (
            24.0,
            26.5,
            9.7,
            "aa23ac00000000000203c00188647f7f000000000067450c00750000000000000001a3b0",
        ),
        (
            24.0,
            25.0,
            9.5,
            "aa23ac00000000000203c00188647f7f000080000064450c00501d00000000000001508e",
        ),
    ];
    for (target, indoor, outdoor, message) in cases {
        let p = payload_of(message);
        let s = State::parse(&p).unwrap_or_else(|e| panic!("{message}: {e}"));
        assert_eq!(s.target_temperature, target, "target in {message}");
        assert_eq!(s.indoor_temperature, Some(indoor), "indoor in {message}");
        assert_eq!(s.outdoor_temperature, Some(outdoor), "outdoor in {message}");
    }
}

// ---------------------------------------------------------------------------
// State responses: bare payloads
// ---------------------------------------------------------------------------

/// msmart `test_message_additional_precision`, bare-payload half, plus
/// `test_target_temperature`. Sweeps the setpoint in half-degree steps, which is
/// what exercises both the 0x10 half-degree bit and the byte-13 alternate
/// setpoint at once.
#[test]
fn target_temperature_across_half_degree_steps() {
    let cases: [(f32, f32, f32, &str); 8] = [
        (
            16.0,
            23.2,
            18.4,
            "c00181667f7f003c00000060560400420000000000000048",
        ),
        (
            16.5,
            23.4,
            18.4,
            "c00191667f7f003c00000060560400440000000000000049",
        ),
        (
            17.0,
            23.6,
            18.3,
            "c00181667f7f003c0000006156050036000000000000004a",
        ),
        (
            17.5,
            23.8,
            18.2,
            "c00191667f7f003c0000006156050028000000000000004b",
        ),
        (
            18.0,
            23.8,
            18.2,
            "c00182667f7f003c0000006156060028000000000000004c",
        ),
        (
            18.5,
            23.8,
            18.2,
            "c00192667f7f003c0000006156060028000000000000004d",
        ),
        (
            19.0,
            23.8,
            18.2,
            "c00183667f7f003c0000006156070028000000000000004e",
        ),
        (
            19.5,
            23.5,
            18.5,
            "c00193667f7f003c00000061570700550000000000000050",
        ),
    ];
    for (target, indoor, outdoor, payload) in cases {
        let s = State::parse(&hex(payload)).unwrap_or_else(|e| panic!("{payload}: {e}"));
        assert_eq!(s.target_temperature, target, "target in {payload}");
        assert_eq!(s.indoor_temperature, Some(indoor), "indoor in {payload}");
        assert_eq!(s.outdoor_temperature, Some(outdoor), "outdoor in {payload}");
    }
}

/// The two payloads from msmart `test_target_temperature` that take the *other*
/// branch: byte 13 is zero, so the setpoint comes from byte 2 alone.
#[test]
fn target_temperature_without_the_alternate_field() {
    for (target, payload) in [
        (16.0, "c00040660000003c00000062680400000000000000000004"),
        (16.5, "c00050660000003c00000062670400000000000000000004"),
    ] {
        let s = State::parse(&hex(payload)).unwrap_or_else(|e| panic!("{payload}: {e}"));
        assert_eq!(s.target_temperature, target, "target in {payload}");
    }
}

// ---------------------------------------------------------------------------
// Regressions
// ---------------------------------------------------------------------------

/// Both bugs found while building this crate, pinned so they cannot come back.
///
/// The swing axis was transposed in the first draft and read HORIZONTAL as
/// VERTICAL — invisible in review, and only caught by diffing live output
/// against the Python implementation.
#[test]
fn regression_swing_axis_is_not_transposed() {
    // Payload with swing = 0x3, which is HORIZONTAL.
    let mut p = vec![0u8; 24];
    p[0] = 0xC0;
    p[7] = 0x3;
    assert_eq!(
        State::parse(&p).unwrap().swing_mode,
        Some(SwingMode::Horizontal)
    );
    p[7] = 0xC;
    assert_eq!(
        State::parse(&p).unwrap().swing_mode,
        Some(SwingMode::Vertical)
    );
}

/// Reading a mode we do not recognise must not silently become a mode we do.
#[test]
fn regression_unknown_mode_is_not_coerced() {
    let mut p = vec![0u8; 24];
    p[0] = 0xC0;
    p[2] = 0x6 << 5;
    let s = State::parse(&p).unwrap();
    assert_eq!(s.mode, None);
    assert_eq!(s.mode_raw, 6);
}

/// Every vector must survive being truncated at any point without panicking.
/// This code parses bytes straight off a socket.
#[test]
fn truncation_never_panics() {
    let full =
        payload_of("aa23ac00000000000303c00145660000003c0010045c6b20000000000000000000020d79");
    for n in 0..=full.len() {
        let _ = State::parse(&full[..n]);
    }
}

/// A frame whose checksum is wrong must be refused before anything reads it.
#[test]
fn a_corrupt_frame_is_refused() {
    let mut raw = hex("aa23ac00000000000303c00145660000003c0010045c6b20000000000000000000020d79");
    let last = raw.len() - 1;
    raw[last] ^= 0xFF;
    assert!(matches!(
        frame::parse(&raw, DeviceType::AirConditioner),
        Err(frame::FrameError::BadChecksum { .. })
    ));
}

/// Sanity: the modes named in these vectors are the ones the REST API publishes,
/// so a client written against Breeze Core sees no change.
#[test]
fn decoded_modes_use_the_published_names() {
    let p = payload_of("aa23ac00000000000303c00145660000003c0010045c6b20000000000000000000020d79");
    let s = State::parse(&p).unwrap();
    let name = s.mode.map(Mode::as_str);
    assert!(
        matches!(name, Some("AUTO" | "COOL" | "DRY" | "HEAT" | "FAN_ONLY")),
        "unexpected mode name {name:?}"
    );
    assert!(response::State::parse(&p).is_ok());
}
