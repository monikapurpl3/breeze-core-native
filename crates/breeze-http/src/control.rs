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
use breeze_proto::ac::command::Setpoint;
use breeze_proto::ac::response::State;
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
    let refused = not_applied(request, &outcome.sent, &outcome.state);
    if debug_control() && !refused.is_empty() {
        eprintln!("  control {id}: the unit did not take {refused:?}");
    }
    let mut value = serde_json::to_value(UnitState::from_info(&info, &outcome.state))
        .unwrap_or_else(|_| serde_json::json!({}));
    if let Some(obj) = value.as_object_mut() {
        obj.insert("not_applied".into(), serde_json::json!(refused));
    }
    Ok(value)
}

/// The fields of `request` the unit did not take: sent one value, reported
/// another.
///
/// Why it exists: a unit that ignores part of a command still answers it, with
/// its state unchanged, so a client saw its change "spring back" and could not
/// tell a refusal from a bug. Units do refuse, and not rarely -- none will
/// move its flaps while switched off, most hold them still while heating until
/// warm air is coming out (flaps change instantly in fan, dry, cool and auto),
/// and many offer eco only while cooling. The reply says which fields, and the
/// clients say why in words.
///
/// Compared against what was *sent*, not what this request asked for. A
/// request merged with later ones may have been superseded -- 24.5 overtaken by
/// a tap to 25 -- and a superseded value is not the unit refusing anything.
/// Only fields this request set are reported: nobody asked about the rest.
///
/// Reported in the control reply only, as `not_applied`, always present (empty
/// when everything was taken), and advertised as the `control_feedback`
/// feature. Additive: a client that does not know the key ignores it.
pub fn not_applied(request: &ControlRequest, sent: &Setpoint, echoed: &State) -> Vec<&'static str> {
    let mut out = Vec::new();
    if request.power_state.is_some() && sent.power_on != echoed.power_on {
        out.push("power_state");
    }
    if request.operational_mode.is_some() && Some(sent.mode) != echoed.mode {
        out.push("operational_mode");
    }
    if request.target_temperature.is_some()
        && (sent.target_temperature - echoed.target_temperature).abs() > 0.01
    {
        out.push("target_temperature");
    }
    if request.fan_speed.is_some() && sent.fan_speed != echoed.fan_speed {
        out.push("fan_speed");
    }
    if request.swing_mode.is_some() && Some(sent.swing_mode) != echoed.swing_mode {
        out.push("swing_mode");
    }
    if request.eco.is_some() && sent.eco != echoed.eco {
        out.push("eco");
    }
    if request.turbo.is_some() && sent.turbo != echoed.turbo {
        out.push("turbo");
    }
    out
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

    /// A state report with the given flaps, eco and target -- the fields these
    /// tests look at -- and nothing surprising elsewhere.
    fn report(swing: SwingMode, eco: bool, target: f32) -> State {
        let mut p = vec![0u8; 24];
        p[0] = 0xC0;
        p[1] = 0x01;
        let whole = target.trunc() as u8;
        p[2] = ((Mode::Heat as u8) << 5)
            | ((whole - 16) & 0xF)
            | if target.fract() > 0.0 { 0x10 } else { 0 };
        p[3] = 102;
        p[7] = swing as u8;
        if eco {
            p[9] |= 0x10;
        }
        p[11] = 0xFF;
        p[12] = 0xFF;
        State::parse(&p).unwrap()
    }

    fn request(json: &str) -> ControlRequest {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_flap_change_the_unit_ignored_is_reported() {
        // The heating warm-up case, measured: flaps sent VERTICAL, the unit
        // answered HORIZONTAL.
        let before = report(SwingMode::Horizontal, false, 24.0);
        let req = request(r#"{"swing_mode": "VERTICAL"}"#);
        let mut sent = Setpoint::from_state(&before);
        change_from(&req).apply_to(&mut sent);
        assert_eq!(not_applied(&req, &sent, &before), ["swing_mode"]);
    }

    #[test]
    fn a_change_the_unit_took_is_not_reported() {
        let req = request(r#"{"swing_mode": "VERTICAL"}"#);
        let mut sent = Setpoint::from_state(&report(SwingMode::Horizontal, false, 24.0));
        change_from(&req).apply_to(&mut sent);
        let after = report(SwingMode::Vertical, false, 24.0);
        assert!(not_applied(&req, &sent, &after).is_empty());
    }

    #[test]
    fn only_the_fields_this_request_set_are_reported() {
        // The unit refused eco; a request that only moved the temperature was
        // not about eco and must not be told it failed.
        let req = request(r#"{"target_temperature": 22.5}"#);
        let mut sent = Setpoint::from_state(&report(SwingMode::Off, false, 24.0));
        change_from(&req).apply_to(&mut sent);
        sent.eco = true; // as if a merged request had asked for eco
        let after = report(SwingMode::Off, false, 22.5);
        assert!(not_applied(&req, &sent, &after).is_empty());
    }

    #[test]
    fn a_value_overtaken_by_a_merged_tap_is_not_a_refusal() {
        // Tapped 24.5, then 25 before the command left: 25 was sent, and the
        // unit took it. The 24.5 request was superseded, not refused.
        let req = request(r#"{"target_temperature": 24.5}"#);
        let mut sent = Setpoint::from_state(&report(SwingMode::Off, false, 24.0));
        sent.target_temperature = 25.0;
        let after = report(SwingMode::Off, false, 25.0);
        assert!(not_applied(&req, &sent, &after).is_empty());
    }

    #[test]
    fn eco_refused_while_heating_is_reported_with_the_flaps() {
        let before = report(SwingMode::Horizontal, false, 24.0);
        let req = request(r#"{"eco": true, "swing_mode": "BOTH"}"#);
        let mut sent = Setpoint::from_state(&before);
        change_from(&req).apply_to(&mut sent);
        assert_eq!(not_applied(&req, &sent, &before), ["swing_mode", "eco"]);
    }

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
