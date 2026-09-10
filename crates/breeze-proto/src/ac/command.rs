//! Commands we send to an air conditioner.
//!
//! A command is a payload, a rolling message id and a CRC, wrapped in a frame.
//! The message id is the caller's business: msmart increments a global, but a
//! device does not appear to care, and making it explicit keeps these functions
//! pure and therefore testable against fixed vectors.

use crate::ac::response::State;
use crate::ac::types::{FanSpeed, Mode, SwingMode, TemperatureType};
use crate::crc8;
use crate::frame::{self, DeviceType, FrameType};

/// Marks a command as coming from an app rather than the unit's own remote.
const CONTROL_SOURCE: u8 = 0x02;

/// Append the message id and CRC, then frame it.
fn finish(frame_type: FrameType, payload: &[u8], message_id: u8) -> Vec<u8> {
    let mut body = Vec::with_capacity(payload.len() + 2);
    body.extend_from_slice(payload);
    body.push(message_id);
    let crc = crc8::calculate(&body);
    body.push(crc);
    frame::build(DeviceType::AirConditioner, frame_type, &body)
}

/// Query basic state — power, mode, setpoint, fan, flap, temperatures.
///
/// The payload is mostly fixed bytes whose meaning is not documented anywhere;
/// they are reproduced exactly because the units expect them.
pub fn get_state(message_id: u8, temperature: TemperatureType) -> Vec<u8> {
    let payload = [
        0x41,
        0x81,
        0x00,
        0xFF,
        0x03,
        0xFF,
        0x00,
        temperature as u8,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x03,
    ];
    finish(FrameType::Query, &payload, message_id)
}

/// Query what the unit supports, so clients can hide controls the hardware lacks.
pub fn get_capabilities(message_id: u8) -> Vec<u8> {
    finish(FrameType::Query, &[0xB5, 0x01, 0x00], message_id)
}

/// The second half of the capability set. Some units answer only the first.
pub fn get_more_capabilities(message_id: u8) -> Vec<u8> {
    finish(FrameType::Query, &[0xB5, 0x01, 0x01, 0x01], message_id)
}

/// Everything a set-state command carries.
///
/// A unit has no notion of "change only the fan speed": every control command
/// restates the whole configuration, so a partial update has to be built by
/// reading current state and modifying it. That is what [`Setpoint::from_state`]
/// is for, and why it exists rather than a `Default` — defaulting the fields
/// nobody asked about would silently turn eco off, or wake a sleeping unit.
#[derive(Debug, Clone, PartialEq)]
pub struct Setpoint {
    pub power_on: bool,
    pub mode: Mode,
    pub target_temperature: f32,
    pub fan_speed: FanSpeed,
    pub swing_mode: SwingMode,
    pub eco: bool,
    pub turbo: bool,
    pub sleep: bool,
    pub fahrenheit: bool,
    pub purifier: bool,
    pub aux_heat: bool,
    pub independent_aux_heat: bool,
    pub follow_me: bool,
    pub freeze_protection: bool,
    pub target_humidity: u8,
    /// Whether the unit chirps on accepting this. Off by default everywhere in
    /// Breeze Core, because a schedule firing at 2 a.m. should be silent.
    pub beep: bool,
}

impl Setpoint {
    /// Start from what the unit currently reports, so a caller can change one
    /// field and leave everything else exactly as it was.
    ///
    /// Values the unit reported as unrecognised fall back to something safe:
    /// there is no way to echo back a mode we could not name.
    pub fn from_state(state: &State) -> Self {
        Self {
            power_on: state.power_on,
            mode: state.mode.unwrap_or(Mode::Auto),
            target_temperature: state.target_temperature,
            fan_speed: state.fan_speed,
            swing_mode: state.swing_mode.unwrap_or(SwingMode::Off),
            eco: state.eco,
            turbo: state.turbo,
            sleep: state.sleep,
            fahrenheit: state.fahrenheit,
            purifier: state.purifier,
            aux_heat: state.aux_heat,
            independent_aux_heat: state.independent_aux_heat,
            follow_me: state.follow_me,
            freeze_protection: state.freeze_protection.unwrap_or(false),
            target_humidity: state.target_humidity.unwrap_or(40),
            beep: false,
        }
    }

    /// Split the setpoint into the two temperature fields the protocol uses.
    ///
    /// 17–30 °C goes in the low nibble of byte 2 biased by 16; anything outside
    /// that range — notably 16 °C, the bottom of Breeze Core's own range — has to
    /// travel in byte 21 biased by 12 instead. The half-degree bit is separate
    /// and applies either way.
    fn temperature_bytes(&self) -> (u8, u8) {
        let integral = self.target_temperature.trunc() as i32;
        let has_half = self.target_temperature - integral as f32 > 0.0;
        let (mut primary, alternate) = if (17..=30).contains(&integral) {
            (((integral - 16) as u8) & 0xF, 0u8)
        } else {
            (0u8, ((integral - 12) as u8) & 0x1F)
        };
        if has_half {
            primary |= 0x10;
        }
        (primary, alternate)
    }
}

/// Build a control command from a complete setpoint.
pub fn set_state(message_id: u8, s: &Setpoint) -> Vec<u8> {
    let (temperature, temperature_alt) = s.temperature_bytes();
    let payload = [
        // Set state
        0x40,
        // Control source, beep and power
        CONTROL_SOURCE | if s.beep { 0x40 } else { 0 } | if s.power_on { 0x01 } else { 0 },
        // Setpoint and mode share a byte
        temperature | ((s.mode as u8 & 0x7) << 5),
        s.fan_speed.0,
        // Timer fields, disabled
        0x7F,
        0x7F,
        0x00,
        // 0x30 is always set alongside the flap selection
        0x30 | (s.swing_mode as u8 & 0x3F),
        if s.follow_me { 0x80 } else { 0 } | if s.turbo { 0x20 } else { 0 },
        if s.eco { 0x80 } else { 0 }
            | if s.purifier { 0x20 } else { 0 }
            | if s.aux_heat { 0x08 } else { 0 },
        if s.sleep { 0x01 } else { 0 }
            | if s.turbo { 0x02 } else { 0 }
            | if s.fahrenheit { 0x04 } else { 0 },
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        temperature_alt,
        s.target_humidity & 0x7F,
        0x00,
        if s.freeze_protection { 0x80 } else { 0 },
        if s.independent_aux_heat { 0x08 } else { 0 },
        0x00,
    ];
    finish(FrameType::Control, &payload, message_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::HEADER_LEN;

    /// msmart-ng's own SetStateCommand bytes, for a pinned setpoint.
    ///
    /// This is the vector `set_state` never had. `get_state` was pinned to the
    /// reference from the start; the CONTROL frame -- the one that actually
    /// changes an air conditioner -- was only ever checked against our own
    /// reading of the protocol, which is a test that agrees with whatever the
    /// code happens to do.
    ///
    /// Generated with msmart-ng, with EVERY field pinned rather than left at
    /// its default -- msmart defaults eco, fahrenheit and beep_on to True, and a
    /// vector built on those encodes msmart's defaults instead of the protocol.
    /// power_on=True, mode=2 (cool), 24.0 C, fan 60, humidity 40, everything
    /// else false, message id 0x11.
    #[test]
    fn set_state_matches_the_reference_bytes() {
        let sp = Setpoint {
            power_on: true,
            beep: false,
            mode: Mode::Cool,
            target_temperature: 24.0,
            fan_speed: FanSpeed(60),
            swing_mode: SwingMode::Off,
            eco: false,
            turbo: false,
            sleep: false,
            fahrenheit: false,
            purifier: false,
            aux_heat: false,
            independent_aux_heat: false,
            follow_me: false,
            freeze_protection: false,
            target_humidity: 40,
        };
        let got = set_state(0x11, &sp);
        let hex: String = got.iter().map(|b| format!("{b:02x}")).collect();
        let want = "aa24ac000000000000024003483c7f7f00300000000000000000000000280000000011867a";
        assert_eq!(
            hex, want,
            "
 got {hex}
want {want}"
        );
    }

    /// msmart's own framing test: with message id 0x11, the GetState payload is
    /// exactly these bytes. This is the vector that pins the whole command layer.
    #[test]
    fn get_state_matches_the_reference_bytes() {
        const EXPECTED: [u8; 23] = [
            0x41, 0x81, 0x00, 0xFF, 0x03, 0xFF, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x11, 0xF4,
        ];
        let f = get_state(0x11, TemperatureType::Indoor);
        assert_eq!(&f[HEADER_LEN..f.len() - 1], &EXPECTED[..]);
        assert_eq!(f[1] as usize, EXPECTED.len() + HEADER_LEN, "length byte");
        assert_eq!(f[2], DeviceType::AirConditioner as u8);
        assert_eq!(f[9], FrameType::Query as u8);
        // And the frame must validate under its own rules.
        assert!(frame::parse(&f, DeviceType::AirConditioner).is_ok());
    }

    #[test]
    fn the_message_id_lands_where_the_crc_covers_it() {
        // Two ids differ in exactly two bytes: the id and the CRC over it.
        let a = get_state(0x11, TemperatureType::Indoor);
        let b = get_state(0x12, TemperatureType::Indoor);
        let diff: Vec<usize> = a
            .iter()
            .zip(b.iter())
            .enumerate()
            .filter(|(_, (x, y))| x != y)
            .map(|(i, _)| i)
            .collect();
        // message id, its CRC, and the frame checksum that covers both.
        assert_eq!(diff.len(), 3, "differing byte positions: {diff:?}");
    }

    #[test]
    fn outdoor_query_differs_only_in_the_sensor_byte() {
        let indoor = get_state(1, TemperatureType::Indoor);
        let outdoor = get_state(1, TemperatureType::Outdoor);
        assert_eq!(indoor[HEADER_LEN + 7], 0x02);
        assert_eq!(outdoor[HEADER_LEN + 7], 0x03);
    }

    #[test]
    fn capability_queries_are_framed_as_queries() {
        for f in [get_capabilities(1), get_more_capabilities(1)] {
            assert_eq!(f[9], FrameType::Query as u8);
            assert_eq!(f[HEADER_LEN], 0xB5);
            assert!(frame::parse(&f, DeviceType::AirConditioner).is_ok());
        }
        // The "more" variant carries one extra selector byte.
        assert_eq!(
            get_more_capabilities(1).len(),
            get_capabilities(1).len() + 1
        );
    }

    fn setpoint(target: f32) -> Setpoint {
        Setpoint {
            power_on: true,
            mode: Mode::Cool,
            target_temperature: target,
            fan_speed: FanSpeed::AUTO,
            swing_mode: SwingMode::Off,
            eco: false,
            turbo: false,
            sleep: false,
            fahrenheit: false,
            purifier: false,
            aux_heat: false,
            independent_aux_heat: false,
            follow_me: false,
            freeze_protection: false,
            target_humidity: 40,
            beep: false,
        }
    }

    #[test]
    fn set_state_is_framed_as_a_control_command() {
        let f = set_state(1, &setpoint(24.0));
        assert_eq!(f[9], FrameType::Control as u8, "must not be a Query");
        assert_eq!(f[HEADER_LEN], 0x40, "set-state opcode");
        assert!(frame::parse(&f, DeviceType::AirConditioner).is_ok());
    }

    #[test]
    fn the_whole_range_survives_a_round_trip_through_the_state_decoder() {
        // Every setpoint Breeze Core allows, encoded then decoded back. This is
        // what catches the 16 C boundary, where the value has to move into the
        // alternate byte instead of the primary nibble.
        let mut t = 16.0f32;
        while t <= 30.0 {
            let f = set_state(1, &setpoint(t));
            let payload = frame::parse(&f, DeviceType::AirConditioner).unwrap();
            // Rebuild a state report from the control payload's shared layout:
            // bytes 1..=13 line up, which is what lets a unit echo state back.
            let mut report = vec![0u8; 24];
            report[0] = 0xC0;
            report[2] = payload[2];
            report[13] = payload[18];
            let decoded = State::parse(&report).unwrap();
            assert_eq!(
                decoded.target_temperature, t,
                "setpoint {t} did not survive"
            );
            t += 0.5;
        }
    }

    #[test]
    fn sixteen_degrees_uses_the_alternate_byte() {
        let (primary, alternate) = setpoint(16.0).temperature_bytes();
        assert_eq!(primary, 0, "16 C is outside the primary 17-30 range");
        assert_eq!(alternate, 4, "and must appear as 16 - 12 in the alternate");

        let (primary, alternate) = setpoint(24.0).temperature_bytes();
        assert_eq!(primary, 8, "24 C is 24 - 16 in the primary nibble");
        assert_eq!(alternate, 0);
    }

    #[test]
    fn the_half_degree_bit_is_independent_of_which_byte_carries_the_degree() {
        assert_eq!(setpoint(16.5).temperature_bytes(), (0x10, 4));
        assert_eq!(setpoint(24.5).temperature_bytes(), (0x18, 0));
    }

    #[test]
    fn beep_is_off_unless_asked_for() {
        let quiet = set_state(1, &setpoint(24.0));
        let mut noisy_sp = setpoint(24.0);
        noisy_sp.beep = true;
        let noisy = set_state(1, &noisy_sp);
        assert_eq!(quiet[HEADER_LEN + 1] & 0x40, 0, "no beep bit by default");
        assert_eq!(noisy[HEADER_LEN + 1] & 0x40, 0x40);
        // Control source must be present either way, or the unit ignores us.
        assert_eq!(quiet[HEADER_LEN + 1] & 0x02, 0x02);
    }

    #[test]
    fn turbo_sets_both_places_the_protocol_reads_it_from() {
        let mut sp = setpoint(24.0);
        sp.turbo = true;
        let f = set_state(1, &sp);
        // Payload index 8 carries follow-me and the alternate turbo bit; index 10
        // carries sleep, turbo and the temperature unit.
        assert_eq!(f[HEADER_LEN + 8] & 0x20, 0x20, "alternate turbo bit");
        assert_eq!(f[HEADER_LEN + 10] & 0x02, 0x02, "primary turbo bit");
    }

    #[test]
    fn from_state_preserves_everything_it_is_given() {
        let mut report = vec![0u8; 24];
        report[0] = 0xC0;
        report[1] = 0x01; // power on
        report[2] = (Mode::Heat as u8) << 5 | 0x08; // 24 C, heat
        report[3] = 60; // fan
        report[7] = SwingMode::Both as u8;
        report[9] = 0x10; // eco
        let state = State::parse(&report).unwrap();
        let sp = Setpoint::from_state(&state);
        assert!(sp.power_on);
        assert_eq!(sp.mode, Mode::Heat);
        assert_eq!(sp.target_temperature, 24.0);
        assert_eq!(sp.fan_speed, FanSpeed(60));
        assert_eq!(sp.swing_mode, SwingMode::Both);
        assert!(sp.eco);
        assert!(!sp.beep, "a read-modify-write must never start beeping");
    }
}
