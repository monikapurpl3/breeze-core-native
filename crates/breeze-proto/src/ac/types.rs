//! The wire enums for air conditioners.
//!
//! These values are the *protocol's*, not ours, and several are counter-intuitive
//! — see [`SwingMode`]. The names must keep matching what Breeze Core's REST API
//! already publishes (`AUTO COOL DRY HEAT FAN_ONLY`, `OFF VERTICAL HORIZONTAL
//! BOTH`), because three clients depend on those strings.

/// Operating mode, as carried in the top three bits of state byte 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Mode {
    Auto = 1,
    Cool = 2,
    Dry = 3,
    Heat = 4,
    FanOnly = 5,
}

impl Mode {
    pub fn from_wire(v: u8) -> Option<Self> {
        Some(match v {
            1 => Self::Auto,
            2 => Self::Cool,
            3 => Self::Dry,
            4 => Self::Heat,
            5 => Self::FanOnly,
            _ => return None,
        })
    }

    /// The name the REST API uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "AUTO",
            Self::Cool => "COOL",
            Self::Dry => "DRY",
            Self::Heat => "HEAT",
            Self::FanOnly => "FAN_ONLY",
        }
    }
}

/// Flap movement.
///
/// The values are not sequential and the two axes are easy to transpose:
/// **HORIZONTAL is 0x3 and VERTICAL is 0xC**, not the other way round. Getting
/// this backwards is invisible in a code review and only shows up as a unit
/// waving the wrong flap, so it is pinned by a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SwingMode {
    Off = 0x0,
    Horizontal = 0x3,
    Vertical = 0xC,
    Both = 0xF,
}

impl SwingMode {
    pub fn from_wire(v: u8) -> Option<Self> {
        Some(match v {
            0x0 => Self::Off,
            0x3 => Self::Horizontal,
            0xC => Self::Vertical,
            0xF => Self::Both,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Horizontal => "HORIZONTAL",
            Self::Vertical => "VERTICAL",
            Self::Both => "BOTH",
        }
    }
}

/// Fan speed. The named steps are what clients offer; 102 means "let the unit
/// decide" and is not on the same scale as the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanSpeed(pub u8);

impl FanSpeed {
    pub const AUTO: Self = Self(102);
    pub const SILENT: Self = Self(20);
    pub const LOW: Self = Self(40);
    pub const MEDIUM: Self = Self(60);
    pub const HIGH: Self = Self(80);
    pub const FULL: Self = Self(100);

    pub fn is_auto(self) -> bool {
        self == Self::AUTO
    }
}

/// Which sensor a state query should report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TemperatureType {
    Indoor = 0x2,
    Outdoor = 0x3,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swing_axes_are_not_transposed() {
        // The bug this file warns about, made unrepeatable.
        assert_eq!(SwingMode::Horizontal as u8, 0x3);
        assert_eq!(SwingMode::Vertical as u8, 0xC);
        assert_eq!(SwingMode::from_wire(0x3), Some(SwingMode::Horizontal));
        assert_eq!(SwingMode::from_wire(0xC), Some(SwingMode::Vertical));
        assert_eq!(SwingMode::from_wire(0x3).unwrap().as_str(), "HORIZONTAL");
        assert_eq!(SwingMode::from_wire(0xC).unwrap().as_str(), "VERTICAL");
    }

    #[test]
    fn unknown_wire_values_are_none_not_a_guess() {
        assert_eq!(SwingMode::from_wire(0x7), None);
        assert_eq!(Mode::from_wire(0), None);
        assert_eq!(Mode::from_wire(6), None);
    }

    #[test]
    fn mode_names_match_the_rest_api() {
        for (m, s) in [
            (Mode::Auto, "AUTO"),
            (Mode::Cool, "COOL"),
            (Mode::Dry, "DRY"),
            (Mode::Heat, "HEAT"),
            (Mode::FanOnly, "FAN_ONLY"),
        ] {
            assert_eq!(m.as_str(), s);
        }
    }

    #[test]
    fn auto_fan_is_distinguishable() {
        assert!(FanSpeed::AUTO.is_auto());
        assert!(!FanSpeed::HIGH.is_auto());
        assert_eq!(FanSpeed::AUTO.0, 102);
    }
}
