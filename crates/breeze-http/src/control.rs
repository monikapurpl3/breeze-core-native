//! Applying a partial control request to a unit.
//!
//! A control command restates the whole configuration — the protocol has no
//! notion of "change only the fan speed" — so a partial request is laid over the
//! unit's last report and the result sent. The unit then echoes its resulting
//! state, which is what makes an optimistic UI honest: the client reconciles
//! against what actually happened rather than what it asked for.
//!
//! Requests that arrive while the unit is busy are merged by the device layer
//! and answered together; see `breeze_device::DeviceManager::control`.

use breeze_device::{Change, DeviceManager};
use breeze_proto::ac::types::{FanSpeed, Mode, SwingMode};
use breeze_store::ControlRequest;

use crate::units::UnitState;

/// Whether to trace the control path. `BREEZE_DEBUG=1` turns it on, along with
/// the authentication dump -- one switch, because anyone debugging a control
/// that does nothing wants both halves and should not have to know two names.
fn debug_control() -> bool {
    matches!(
        std::env::var("BREEZE_DEBUG").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// The device layer's view of a request: typed, and only what it sets.
///
/// Unknown enum names are rejected earlier, by validation, so one reaching here
/// is dropped rather than guessed at.
pub fn change_from(request: &ControlRequest) -> Change {
    Change {
        power_on: request.power_state,
        mode: request.operational_mode.as_deref().and_then(mode_from_name),
        target_temperature: request.target_temperature.map(|v| v as f32),
        fan_speed: request.fan_speed.map(|v| FanSpeed(v as u8)),
        swing_mode: request.swing_mode.as_deref().and_then(swing_from_name),
        eco: request.eco,
        turbo: request.turbo,
        // Absent means silent. Older clients never send `beep`, and a
        // schedule firing at 2 a.m. should not chirp.
        beep: request.beep,
    }
}

/// Apply `request` to a unit and return the state it reports back.
pub fn apply(
    manager: &DeviceManager,
    id: u64,
    request: &ControlRequest,
) -> Result<serde_json::Value, String> {
    let info = manager
        .info(id)
        .ok_or_else(|| "unit has no V3 token and key configured".to_string())?;
    if debug_control() {
        eprintln!(
            "  control {id}: requested {}",
            serde_json::to_string(request).unwrap_or_default()
        );
    }
    let outcome = manager
        .control(id, change_from(request))
        .map_err(|e| e.to_string())?;
    if debug_control() && !outcome.sent_by_this_request {
        eprintln!(
            "  control {id}: folded into a command carrying {} requests; unit echoed temp={}",
            outcome.merged, outcome.state.target_temperature
        );
    } else if debug_control() {
        let (base, sent, echoed) = (&outcome.base, &outcome.sent, &outcome.state);
        eprintln!(
            "  control {id}: built on {} report mode={:?} temp={} fan={:?} power={}",
            if outcome.base_was_cached {
                "the last"
            } else {
                "a fresh"
            },
            base.mode,
            base.target_temperature,
            base.fan_speed,
            base.power_on
        );
        eprintln!(
            "  control {id}: sent mode={:?} temp={} fan={:?} power={} swing={:?} ({} request{} merged)",
            sent.mode,
            sent.target_temperature,
            sent.fan_speed,
            sent.power_on,
            sent.swing_mode,
            outcome.merged,
            if outcome.merged == 1 { "" } else { "s" }
        );
        eprintln!(
            "  control {id}: unit echoed mode={:?} temp={} fan={:?} power={}",
            echoed.mode, echoed.target_temperature, echoed.fan_speed, echoed.power_on
        );
    }
    let state = UnitState::from_info(&info, &outcome.state);
    Ok(serde_json::to_value(state).unwrap_or_else(|_| serde_json::json!({})))
}

/// Names as the REST API publishes them. Unknown values are rejected earlier,
/// by validation, so reaching here with one means the caller skipped that.
pub fn mode_from_name(name: &str) -> Option<Mode> {
    match name.to_uppercase().as_str() {
        "AUTO" => Some(Mode::Auto),
        "COOL" => Some(Mode::Cool),
        "DRY" => Some(Mode::Dry),
        "HEAT" => Some(Mode::Heat),
        "FAN_ONLY" => Some(Mode::FanOnly),
        _ => None,
    }
}

pub fn swing_from_name(name: &str) -> Option<SwingMode> {
    match name.to_uppercase().as_str() {
        "OFF" => Some(SwingMode::Off),
        "VERTICAL" => Some(SwingMode::Vertical),
        "HORIZONTAL" => Some(SwingMode::Horizontal),
        "BOTH" => Some(SwingMode::Both),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_becomes_a_change_with_only_what_it_sets() {
        let request: ControlRequest =
            serde_json::from_str(r#"{"target_temperature": 23.5, "operational_mode": "heat"}"#)
                .unwrap();
        let c = change_from(&request);
        assert_eq!(c.target_temperature, Some(23.5));
        assert_eq!(c.mode, Some(Mode::Heat), "names are case-insensitive");
        assert_eq!(
            c.power_on, None,
            "an absent field must stay absent, or merging breaks"
        );
        assert_eq!(c.beep, None, "absent beep is silent, decided at send time");
    }

    #[test]
    fn mode_names_round_trip_through_the_api_spelling() {
        for (name, mode) in [
            ("COOL", Mode::Cool),
            ("cool", Mode::Cool),
            ("FAN_ONLY", Mode::FanOnly),
            ("HEAT", Mode::Heat),
        ] {
            assert_eq!(mode_from_name(name), Some(mode));
            assert_eq!(mode_from_name(mode.as_str()), Some(mode));
        }
        assert_eq!(
            mode_from_name("FANONLY"),
            None,
            "must not accept a near miss"
        );
        assert_eq!(mode_from_name(""), None);
    }

    #[test]
    fn swing_names_are_not_transposed_here_either() {
        assert_eq!(swing_from_name("HORIZONTAL"), Some(SwingMode::Horizontal));
        assert_eq!(swing_from_name("VERTICAL"), Some(SwingMode::Vertical));
        assert_eq!(swing_from_name("BOTH"), Some(SwingMode::Both));
        assert_eq!(swing_from_name("OFF"), Some(SwingMode::Off));
        assert_eq!(swing_from_name("SIDEWAYS"), None);
    }
}
