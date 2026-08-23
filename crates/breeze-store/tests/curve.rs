//! The curve maths, checked against Breeze Core's own scheduler.
//!
//! `fixtures/curve-vectors.json` is produced by running the *reference*
//! implementation (see `fixtures/generate-curve-vectors.py`), so these tests
//! compare against what Python actually returns rather than against my reading
//! of it. That distinction has already earned its keep: Python's `round()` is
//! banker's rounding and Rust's `f64::round` is not, which moved every exact
//! midpoint by half a degree.

use breeze_store::models::CurvePoint;
use breeze_store::{curve_setpoint, round_half};
use chrono::NaiveDate;
use serde::Deserialize;

#[derive(Deserialize)]
struct Vectors {
    round_half: Vec<(f64, f64)>,
    curve: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    points: Vec<CurvePoint>,
    /// `[minutes past midnight, expected setpoint]`.
    samples: Vec<(i64, Option<f64>)>,
}

fn vectors() -> Vectors {
    let raw = include_str!("fixtures/curve-vectors.json");
    serde_json::from_str(raw).expect("curve vectors")
}

#[test]
fn rounding_matches_the_reference_everywhere() {
    let v = vectors();
    assert!(v.round_half.len() > 100, "vectors look truncated");
    for (input, expected) in v.round_half {
        assert_eq!(
            round_half(input),
            expected,
            "round_half({input}) disagrees with the Python reference"
        );
    }
}

#[test]
fn every_curve_sample_matches_the_reference() {
    let v = vectors();
    assert!(v.curve.len() >= 20, "vectors look truncated");
    let mut checked = 0usize;
    for case in &v.curve {
        for (mins, expected) in &case.samples {
            let now = NaiveDate::from_ymd_opt(2026, 8, 23)
                .unwrap()
                .and_hms_opt((mins / 60) as u32, (mins % 60) as u32, 13)
                .unwrap();
            let got = curve_setpoint(&case.points, now);
            assert_eq!(
                got,
                *expected,
                "at {:02}:{:02} for {:?}",
                mins / 60,
                mins % 60,
                case.points
                    .iter()
                    .map(|p| (p.time.as_str(), p.temperature))
                    .collect::<Vec<_>>()
            );
            checked += 1;
        }
    }
    // A silently empty fixture would make this test pass while checking nothing.
    assert!(checked > 4000, "only {checked} samples compared");
}
