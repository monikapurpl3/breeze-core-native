//! Decoding a unit's state report.
//!
//! The layout is a bit-field salad and the temperature encoding has three
//! separate paths, so every rule here is pinned by a vector in
//! `tests/vectors.rs` taken from msmart's own test suite.

use crate::ac::types::{FanSpeed, Mode, SwingMode};

/// A `0xC0` state report.
///
/// Fields are `Option` where the protocol genuinely has an absent value — an
/// unfitted outdoor probe reports `0xFF`, and a mode byte can hold a value no
/// released firmware documents. Guessing in those cases is how a client ends up
/// displaying "AUTO" for something else entirely.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub power_on: bool,
    pub target_temperature: f32,
    pub mode: Option<Mode>,
    pub mode_raw: u8,
    pub fan_speed: FanSpeed,
    pub swing_mode: Option<SwingMode>,
    pub swing_raw: u8,
    pub turbo: bool,
    pub eco: bool,
    pub sleep: bool,
    pub fahrenheit: bool,
    pub indoor_temperature: Option<f32>,
    pub outdoor_temperature: Option<f32>,
    pub filter_alert: bool,
    pub display_on: bool,
    pub aux_heat: bool,
    pub independent_aux_heat: bool,
    pub follow_me: bool,
    pub purifier: bool,
    pub error_code: u8,
    pub target_humidity: Option<u8>,
    pub freeze_protection: Option<bool>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResponseError {
    /// Not enough bytes to read the mandatory fields.
    TooShort(usize),
    /// First byte was not 0xC0, so this is some other report.
    NotAStateReport(u8),
}

impl core::fmt::Display for ResponseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooShort(n) => write!(f, "state report is only {n} bytes"),
            Self::NotAStateReport(t) => write!(f, "report type is 0x{t:02X}, not 0xC0"),
        }
    }
}

impl std::error::Error for ResponseError {}

/// The shortest report we can read every mandatory field from.
const MIN_LEN: usize = 17;

/// Decode a temperature byte, folding in the extra precision nibble.
///
/// Three paths, all of them load-bearing:
/// * `0xFF` means there is no sensor — not 77.5 degrees;
/// * in Celsius with a decimal nibble, the half-degree bit is *ignored* and the
///   nibble supplies the fraction;
/// * in Fahrenheit the nibble is only consulted for a half-degree.
///
/// Ported from msmart, which credits MideaUART.
fn parse_temperature(data: u8, decimals: f32, fahrenheit: bool) -> Option<f32> {
    if data == 0xFF {
        return None;
    }
    let temperature = (data as f32 - 50.0) / 2.0;
    if !fahrenheit && decimals != 0.0 {
        let whole = temperature.trunc();
        return Some(if temperature >= 0.0 {
            whole + decimals
        } else {
            whole - decimals
        });
    }
    if decimals >= 0.5 {
        let whole = temperature.trunc();
        return Some(if temperature >= 0.0 {
            whole + 0.5
        } else {
            whole - 0.5
        });
    }
    Some(temperature)
}

impl State {
    /// Parse the payload of a framed `0xC0` report.
    pub fn parse(p: &[u8]) -> Result<Self, ResponseError> {
        if p.len() < MIN_LEN {
            return Err(ResponseError::TooShort(p.len()));
        }
        if p[0] != 0xC0 {
            return Err(ResponseError::NotAStateReport(p[0]));
        }

        let half_degree = p[2] & 0x10 != 0;
        let mut target_temperature =
            (p[2] & 0xF) as f32 + 16.0 + if half_degree { 0.5 } else { 0.0 };

        let mode_raw = (p[2] >> 5) & 0x7;
        let swing_raw = p[7] & 0xF;
        let fahrenheit = p[10] & 0x04 != 0;

        // Byte 15 carries the fractional part of both temperatures: low nibble
        // for indoor, high nibble for outdoor, in tenths.
        let indoor_decimals = (p[15] & 0xF) as f32 / 10.0;
        let outdoor_decimals = (p[15] >> 4) as f32 / 10.0;

        // Some firmware reports the setpoint again in byte 13 with a different
        // bias. When present it wins -- it is the one that survives the unit
        // being set from its own remote.
        let alt = p[13] & 0x1F;
        if alt != 0 {
            target_temperature = alt as f32 + 12.0 + if half_degree { 0.5 } else { 0.0 };
        }

        Ok(Self {
            power_on: p[1] & 0x1 != 0,
            target_temperature,
            mode: Mode::from_wire(mode_raw),
            mode_raw,
            fan_speed: FanSpeed(p[3] & 0x7F),
            swing_mode: SwingMode::from_wire(swing_raw),
            swing_raw,
            // Turbo is reported in two different places depending on firmware.
            turbo: (p[8] & 0x20 != 0) || (p[10] & 0x02 != 0),
            eco: p[9] & 0x10 != 0,
            sleep: p[10] & 0x1 != 0,
            fahrenheit,
            indoor_temperature: parse_temperature(p[11], indoor_decimals, fahrenheit),
            outdoor_temperature: parse_temperature(p[12], outdoor_decimals, fahrenheit),
            filter_alert: p[13] & 0x20 != 0,
            display_on: p[14] != 0x70,
            aux_heat: p[9] & 0x08 != 0,
            independent_aux_heat: p[8] & 0x40 != 0,
            follow_me: p[8] & 0x80 != 0,
            purifier: p[9] & 0x20 != 0,
            error_code: p[16],
            target_humidity: p.get(19).map(|b| b & 0x7F),
            freeze_protection: p.get(21).map(|b| b & 0x80 != 0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_sensor_is_none_not_a_temperature() {
        assert_eq!(parse_temperature(0xFF, 0.0, false), None);
        // 0xFF would otherwise decode to a plausible-looking 102.5 C.
        assert_eq!(parse_temperature(0xFE, 0.0, false), Some(102.0));
    }

    #[test]
    fn celsius_uses_the_decimal_nibble() {
        // (96-50)/2 = 23.0, nibble 2 -> 23.2
        assert_eq!(parse_temperature(0x60, 0.2, false), Some(23.2));
        // (86-50)/2 = 18.0, nibble 4 -> 18.4
        assert_eq!(parse_temperature(0x56, 0.4, false), Some(18.4));
    }

    #[test]
    fn fahrenheit_only_rounds_to_a_half() {
        assert_eq!(parse_temperature(0x60, 0.4, true), Some(23.0));
        assert_eq!(parse_temperature(0x60, 0.7, true), Some(23.5));
    }

    #[test]
    fn negative_temperatures_move_away_from_zero() {
        // (0-50)/2 = -25.0 with a 0.5 nibble must be -25.5, not -24.5.
        assert_eq!(parse_temperature(0x00, 0.5, false), Some(-25.5));
    }

    #[test]
    fn short_and_wrong_type_payloads_are_refused() {
        assert_eq!(State::parse(&[0xC0; 4]), Err(ResponseError::TooShort(4)));
        let mut p = [0u8; 24];
        p[0] = 0xA0;
        assert_eq!(State::parse(&p), Err(ResponseError::NotAStateReport(0xA0)));
    }

    #[test]
    fn unrecognised_enum_values_are_preserved_raw() {
        let mut p = [0u8; 24];
        p[0] = 0xC0;
        p[2] = 0x7 << 5; // a mode no firmware documents
        p[7] = 0x7; // a swing value likewise
        let s = State::parse(&p).unwrap();
        assert_eq!(s.mode, None);
        assert_eq!(s.mode_raw, 0x7);
        assert_eq!(s.swing_mode, None);
        assert_eq!(s.swing_raw, 0x7);
    }
}
