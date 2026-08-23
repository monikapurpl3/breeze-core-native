//! Applying a partial control request to a unit.
//!
//! A control command restates the whole configuration — the protocol has no
//! notion of "change only the fan speed" — so a partial request has to be read,
//! merged and written. The unit then echoes its resulting state, which is what
//! makes an optimistic UI honest: the client reconciles against what actually
//! happened rather than what it asked for.

use breeze_device::DeviceManager;
use breeze_proto::ac::command::Setpoint;
use breeze_proto::ac::types::{FanSpeed, Mode, SwingMode};
use breeze_store::ControlRequest;

use crate::units::UnitState;

/// Merge `request` into the unit's current state and apply it.
pub fn apply(
    manager: &DeviceManager,
    id: u64,
    request: &ControlRequest,
) -> Result<serde_json::Value, String> {
    manager
        .with_unit(id, |device| {
            let current = device.refresh()?;
            let mut setpoint = Setpoint::from_state(&current);

            if let Some(v) = request.power_state {
                setpoint.power_on = v;
            }
            if let Some(v) = request.target_temperature {
                setpoint.target_temperature = v as f32;
            }
            if let Some(v) = request.fan_speed {
                setpoint.fan_speed = FanSpeed(v as u8);
            }
            if let Some(v) = &request.operational_mode {
                if let Some(m) = mode_from_name(v) {
                    setpoint.mode = m;
                }
            }
            if let Some(v) = &request.swing_mode {
                if let Some(s) = swing_from_name(v) {
                    setpoint.swing_mode = s;
                }
            }
            if let Some(v) = request.eco {
                setpoint.eco = v;
            }
            if let Some(v) = request.turbo {
                setpoint.turbo = v;
            }
            // Absent means silent. Older clients never send `beep`, and a
            // schedule firing at 2 a.m. should not chirp.
            setpoint.beep = request.beep.unwrap_or(false);

            let applied = device.apply(&setpoint)?;
            let state = UnitState::from_device(device, &applied);
            Ok(serde_json::to_value(state).unwrap_or_else(|_| serde_json::json!({})))
        })
        .map_err(|e| e.to_string())
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
