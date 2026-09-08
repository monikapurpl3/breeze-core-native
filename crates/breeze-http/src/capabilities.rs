//! `GET /api/units/{id}/capabilities` — what one unit's firmware supports.
//!
//! Lets a client hide controls the hardware does not have. A unit with no
//! horizontal flap should not offer left-right swing: the firmware silently
//! ignores an unsupported swing mode rather than refusing it, so without this
//! the panel shows a button that appears to work and does nothing.
//!
//! Costs a LAN round-trip the first time and nothing afterwards — the device
//! layer caches the answer, which cannot change while the unit is powered.
//!
//! The fields are the reference's, including the two tri-state booleans. Those
//! are `null` when the unit reported no swing capability at all, which means
//! "unknown, show everything" rather than "no flaps" — an older firmware that
//! says nothing must not have its controls taken away.

use breeze_proto::ac::capabilities::Capabilities;

use crate::respond::Reply;
use crate::state::AppState;

/// Serialise a unit's capabilities in the reference's shape.
/// Whole degrees as a JSON integer, fractional ones as a float.
///
/// The reference holds these as Python ints and serialises `30`; this held them
/// as f64 and serialised `30.0`. JSON has one number type and consumers do not:
/// JavaScript cannot tell the difference, and a strictly typed client asking for
/// an integer breaks outright -- Dart's `as int` throws on a double. The Android
/// app happens to use `as num` and survived it; a third-party client need not.
///
/// Found by diffing this endpoint against a live 3.2.0.
fn degrees(v: f64) -> serde_json::Value {
    if v.fract() == 0.0 && v.is_finite() {
        serde_json::json!(v as i64)
    } else {
        serde_json::json!(v)
    }
}

pub fn view(unit_id: &str, caps: &Capabilities) -> serde_json::Value {
    // `null`, not `false`, when nothing was reported: see the module docs.
    let (vertical, horizontal) = if caps.swing_horizontal || caps.swing_vertical {
        (
            serde_json::json!(caps.swing_vertical || caps.swing_both()),
            serde_json::json!(caps.swing_horizontal || caps.swing_both()),
        )
    } else {
        (serde_json::Value::Null, serde_json::Value::Null)
    };

    serde_json::json!({
        "id": unit_id,
        "operational_modes": caps.operational_modes(),
        "swing_modes": caps.swing_modes(),
        "supports_vertical_swing": vertical,
        "supports_horizontal_swing": horizontal,
        "fan_speeds": caps.fan_speeds(),
        "supports_custom_fan_speed": caps.fan_custom,
        "min_target_temperature": degrees(caps.min_temperature()),
        "max_target_temperature": degrees(caps.max_temperature()),
        "supports_eco": caps.eco,
        "supports_turbo": caps.turbo(),
        "supports_display_control": caps.display_control,
        "supports_freeze_protection": caps.freeze_protection,
        "supports_humidity": caps.humidity_manual_set,
        // Beyond the reference, and cheap: a unit reporting capabilities this
        // build does not interpret is worth seeing rather than silently
        // flattening to "unsupported".
        "unrecognised_capability_ids": caps
            .unknown_ids
            .iter()
            .map(|id| format!("0x{id:04X}"))
            .collect::<Vec<_>>(),
    })
}

/// `GET /api/units/{id}/capabilities`
pub fn get(state: &AppState, id: u64) -> Reply {
    let result = state.manager.with_unit(id, |device| device.capabilities());
    match result {
        Ok(caps) => Reply::json(200, &view(&id.to_string(), &caps)),
        // The same 503 a state read gives for the same reason: a unit that will
        // not answer is not a bad request.
        Err(e) => Reply::detail(503, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A payload in the wire shape: `[0xB5, count, (id, size, value)...]`.
    fn payload(records: &[(u16, &[u8])]) -> Vec<u8> {
        let mut out = vec![0xB5, records.len() as u8];
        for (id, value) in records {
            out.extend_from_slice(&id.to_le_bytes());
            out.push(value.len() as u8);
            out.extend_from_slice(value);
        }
        out
    }

    #[test]
    fn a_fully_capable_unit_reports_everything() {
        let caps = Capabilities::parse(&payload(&[
            (0x0214, &[1]), // all modes
            (0x0215, &[1]), // both flaps
            (0x0210, &[1]), // custom fan
            (0x0212, &[1]), // eco
            (0x021A, &[1]), // turbo
            (0x0224, &[1]), // display control
            (0x0213, &[1]), // freeze protection
            (0x021F, &[3]), // humidity, manual
            (0x0225, &[32, 60, 32, 60, 32, 60]),
        ]));
        let view = view("7", &caps);

        assert_eq!(view["id"], "7");
        assert_eq!(
            view["operational_modes"],
            serde_json::json!(["FAN_ONLY", "DRY", "COOL", "HEAT", "AUTO"])
        );
        assert_eq!(
            view["swing_modes"],
            serde_json::json!(["OFF", "HORIZONTAL", "VERTICAL", "BOTH"])
        );
        assert_eq!(view["supports_vertical_swing"], true);
        assert_eq!(view["supports_horizontal_swing"], true);
        assert_eq!(view["supports_custom_fan_speed"], true);
        assert_eq!(view["supports_eco"], true);
        assert_eq!(view["supports_turbo"], true);
        assert_eq!(view["supports_display_control"], true);
        assert_eq!(view["supports_freeze_protection"], true);
        assert_eq!(view["supports_humidity"], true);
        assert_eq!(view["min_target_temperature"], 16, "whole degrees must be an integer, as the reference sends");
        assert_eq!(view["max_target_temperature"], 30, "whole degrees must be an integer, as the reference sends");
    }

    #[test]
    fn a_unit_with_one_flap_does_not_offer_both() {
        // The case this endpoint exists for. Value 0 is vertical only.
        let caps = Capabilities::parse(&payload(&[(0x0215, &[0])]));
        let view = view("7", &caps);
        assert_eq!(view["swing_modes"], serde_json::json!(["OFF", "VERTICAL"]));
        assert_eq!(view["supports_vertical_swing"], true);
        assert_eq!(
            view["supports_horizontal_swing"], false,
            "a unit with no horizontal flap must say so"
        );
    }

    #[test]
    fn a_unit_that_said_nothing_about_swing_reports_null_not_false() {
        // "Unknown, show everything" -- an older firmware that reports no swing
        // capability must not have its controls taken away.
        let caps = Capabilities::default();
        let view = view("7", &caps);
        assert!(view["supports_vertical_swing"].is_null());
        assert!(view["supports_horizontal_swing"].is_null());
        // The mode list still contains OFF, which every unit can do.
        assert_eq!(view["swing_modes"], serde_json::json!(["OFF"]));
    }

    #[test]
    fn a_silent_unit_still_gets_usable_defaults() {
        // A unit that reported nothing at all: the response must still be
        // something a client can build a UI from.
        let view = view("7", &Capabilities::default());
        assert_eq!(view["operational_modes"], serde_json::json!(["FAN_ONLY"]));
        assert_eq!(
            view["fan_speeds"],
            serde_json::json!(["LOW", "MEDIUM", "HIGH", "AUTO"])
        );
        assert_eq!(view["min_target_temperature"], 16, "whole degrees must be an integer, as the reference sends");
        assert_eq!(view["max_target_temperature"], 30, "whole degrees must be an integer, as the reference sends");
        assert_eq!(view["supports_eco"], false);
    }

    #[test]
    fn the_response_has_exactly_the_reference_fields() {
        // A client reads these by name; a missing one is a blank control.
        let view = view("7", &Capabilities::default());
        let object = view.as_object().unwrap();
        for required in [
            "id",
            "operational_modes",
            "swing_modes",
            "supports_vertical_swing",
            "supports_horizontal_swing",
            "fan_speeds",
            "supports_custom_fan_speed",
            "min_target_temperature",
            "max_target_temperature",
            "supports_eco",
            "supports_turbo",
            "supports_display_control",
            "supports_freeze_protection",
            "supports_humidity",
        ] {
            assert!(object.contains_key(required), "missing {required}");
        }
    }

    #[test]
    fn unrecognised_ids_are_surfaced_as_hex() {
        let caps = Capabilities::parse(&payload(&[(0x0999, &[1])]));
        let view = view("7", &caps);
        assert_eq!(
            view["unrecognised_capability_ids"],
            serde_json::json!(["0x0999"])
        );
    }
}
