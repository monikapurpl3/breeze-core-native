//! The control payload, shared by favourites, schedule entries and timers.
//!
//! Every field is optional and **absent means "leave it alone"**, which is why
//! `None` must serialise as `null` rather than being skipped: an explicit null is
//! how the existing files record "this scene does not touch eco". Skipping the key
//! would still read back the same, but the bytes would differ, and a migration
//! that rewrites every stored file is not a migration anyone should have to trust.

use serde::{Deserialize, Serialize};

/// Bounds the REST API enforces, restated here so a hand-edited file is rejected
/// at load rather than passed to the firmware.
pub const TEMP_MIN: f64 = 16.0;
pub const TEMP_MAX: f64 = 30.0;
pub const ALLOWED_FAN_SPEEDS: [i64; 6] = [20, 40, 60, 80, 100, 102];
pub const OPERATIONAL_MODES: [&str; 5] = ["AUTO", "COOL", "DRY", "HEAT", "FAN_ONLY"];
pub const SWING_MODES: [&str; 4] = ["OFF", "VERTICAL", "HORIZONTAL", "BOTH"];

/// Field order matches Breeze Core's `ControlRequest` exactly. Reordering these
/// changes the bytes written to every store file.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ControlRequest {
    #[serde(default)]
    pub power_state: Option<bool>,
    #[serde(default)]
    pub operational_mode: Option<String>,
    #[serde(default)]
    pub target_temperature: Option<f64>,
    #[serde(default)]
    pub fan_speed: Option<i64>,
    #[serde(default)]
    pub swing_mode: Option<String>,
    #[serde(default)]
    pub eco: Option<bool>,
    #[serde(default)]
    pub turbo: Option<bool>,
    /// Absent means silent, which is what keeps older clients quiet.
    #[serde(default)]
    pub beep: Option<bool>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ValidationError {
    TemperatureOutOfRange,
    TemperatureNotHalfDegree,
    UnknownFanSpeed(i64),
    UnknownMode(String),
    UnknownSwingMode(String),
}

impl core::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TemperatureOutOfRange => {
                write!(
                    f,
                    "target_temperature must be between {TEMP_MIN} and {TEMP_MAX}"
                )
            }
            Self::TemperatureNotHalfDegree => {
                write!(f, "target_temperature must be in 0.5 degree steps")
            }
            Self::UnknownFanSpeed(v) => {
                write!(f, "fan_speed {v} is not one of {ALLOWED_FAN_SPEEDS:?}")
            }
            // Verbatim from the reference, which raises these by hand in
            // devices/control.py -- unlike the 422 bodies, which are FastAPI's
            // own shape rather than anything anyone designed. A client that
            // matches on the message text keeps working.
            Self::UnknownMode(v) => write!(f, "Unknown mode: {v}"),
            Self::UnknownSwingMode(v) => write!(f, "Unknown swing mode: {v}"),
        }
    }
}

impl std::error::Error for ValidationError {}

impl ValidationError {
    /// The HTTP status the reference answers for this kind of bad value.
    ///
    /// Not one status for all of them, because the reference does not use one:
    /// a **bad enum member is 400** and an out-of-range number is 422. That
    /// looks arbitrary until you see where each comes from -- pydantic rejects
    /// the numeric bounds while parsing the body, so FastAPI turns it into its
    /// own 422, whereas an unknown mode name survives parsing and is refused by
    /// the route with an explicit 400.
    ///
    /// Answering 422 for everything, as this did, is invisible until a client
    /// branches on the status. CLAUDE.md states the contract as "400 bad enum";
    /// a differential run against the live 3.2.0 confirmed it.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::UnknownMode(_) | Self::UnknownSwingMode(_) => 400,
            Self::TemperatureOutOfRange
            | Self::TemperatureNotHalfDegree
            | Self::UnknownFanSpeed(_) => 422,
        }
    }
}

impl ControlRequest {
    /// Check the same bounds the REST API does, so a bad value is refused before
    /// it can reach a unit.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some(t) = self.target_temperature {
            if !(TEMP_MIN..=TEMP_MAX).contains(&t) {
                return Err(ValidationError::TemperatureOutOfRange);
            }
            if (t * 2.0).fract() != 0.0 {
                return Err(ValidationError::TemperatureNotHalfDegree);
            }
        }
        if let Some(f) = self.fan_speed {
            if !ALLOWED_FAN_SPEEDS.contains(&f) {
                return Err(ValidationError::UnknownFanSpeed(f));
            }
        }
        if let Some(m) = &self.operational_mode {
            if !OPERATIONAL_MODES.contains(&m.to_uppercase().as_str()) {
                return Err(ValidationError::UnknownMode(m.clone()));
            }
        }
        if let Some(m) = &self.swing_mode {
            if !SWING_MODES.contains(&m.to_uppercase().as_str()) {
                return Err(ValidationError::UnknownSwingMode(m.clone()));
            }
        }
        Ok(())
    }

    /// A scene that switches the unit off — what a sleep timer defaults to.
    pub fn power_off() -> Self {
        Self {
            power_state: Some(false),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_enum_messages_are_the_reference_wording() {
        // Matched against the live 3.2.0, which raises these by hand:
        //   raise HTTPException(400, f"Unknown mode: {req.operational_mode}")
        let mode = ControlRequest {
            operational_mode: Some("TELEPORT".into()),
            ..Default::default()
        };
        let swing = ControlRequest {
            swing_mode: Some("DIAGONAL".into()),
            ..Default::default()
        };
        assert_eq!(mode.validate().unwrap_err().to_string(), "Unknown mode: TELEPORT");
        assert_eq!(
            swing.validate().unwrap_err().to_string(),
            "Unknown swing mode: DIAGONAL"
        );
    }

    #[test]
    fn a_bad_enum_is_400_and_a_bad_number_is_422() {
        // Measured against a running 3.2.0, not guessed: the reference answers
        // 400 for an unknown mode or swing name and 422 for a numeric bound.
        let bad = |r: ControlRequest| r.validate().unwrap_err().http_status();
        assert_eq!(
            bad(ControlRequest {
                operational_mode: Some("TELEPORT".into()),
                ..Default::default()
            }),
            400
        );
        assert_eq!(
            bad(ControlRequest {
                swing_mode: Some("DIAGONAL".into()),
                ..Default::default()
            }),
            400
        );
        assert_eq!(
            bad(ControlRequest {
                target_temperature: Some(99.0),
                ..Default::default()
            }),
            422
        );
        assert_eq!(
            bad(ControlRequest {
                fan_speed: Some(7),
                ..Default::default()
            }),
            422
        );
    }

    #[test]
    fn none_serialises_as_null_not_omitted() {
        // The existing files record absent fields explicitly. Skipping them would
        // read back identically but rewrite every stored file.
        let json = serde_json::to_string(&ControlRequest::default()).unwrap();
        assert!(json.contains("\"beep\":null"), "got {json}");
        assert_eq!(json.matches("null").count(), 8);
    }

    #[test]
    fn field_order_matches_breeze_core() {
        let json = serde_json::to_string(&ControlRequest::default()).unwrap();
        let order: Vec<&str> = json
            .split(',')
            .filter_map(|p| p.split(':').next())
            .map(|k| k.trim_matches(|c| c == '{' || c == '"'))
            .collect();
        assert_eq!(
            order,
            [
                "power_state",
                "operational_mode",
                "target_temperature",
                "fan_speed",
                "swing_mode",
                "eco",
                "turbo",
                "beep"
            ]
        );
    }

    #[test]
    fn a_partial_document_loads_with_the_rest_absent() {
        // Hand-written or older files may omit keys entirely.
        let r: ControlRequest = serde_json::from_str(r#"{"power_state":true}"#).unwrap();
        assert_eq!(r.power_state, Some(true));
        assert_eq!(r.beep, None);
    }

    #[test]
    fn bounds_match_the_rest_api() {
        let ok = ControlRequest {
            target_temperature: Some(24.5),
            fan_speed: Some(102),
            operational_mode: Some("cool".into()),
            swing_mode: Some("BOTH".into()),
            ..Default::default()
        };
        assert_eq!(ok.validate(), Ok(()));

        for (bad, want) in [
            (
                ControlRequest {
                    target_temperature: Some(15.5),
                    ..Default::default()
                },
                ValidationError::TemperatureOutOfRange,
            ),
            (
                ControlRequest {
                    target_temperature: Some(24.25),
                    ..Default::default()
                },
                ValidationError::TemperatureNotHalfDegree,
            ),
            (
                ControlRequest {
                    fan_speed: Some(50),
                    ..Default::default()
                },
                ValidationError::UnknownFanSpeed(50),
            ),
        ] {
            assert_eq!(bad.validate(), Err(want));
        }
    }

    #[test]
    fn power_off_touches_only_power() {
        let off = ControlRequest::power_off();
        assert_eq!(off.power_state, Some(false));
        assert_eq!(off.target_temperature, None);
        assert_eq!(off.eco, None, "a sleep timer must not change anything else");
    }
}
