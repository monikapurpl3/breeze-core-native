//! What a unit says it can do.
//!
//! The reply to a `0xB5` query is a count followed by that many
//! `[id_lo, id_hi, size, value…]` records. Most records are a single byte whose
//! meaning depends on the id — often "which *set* of features you get", not a
//! boolean — so each one needs its own table of accepted values. Those tables
//! are msmart's, and they are the only source for them: the values are a vendor
//! matrix nobody has documented publicly.
//!
//! Pure decoding, like the rest of `breeze-proto`. The socket work and the
//! caching live in `breeze-device`.
//!
//! # Why a client wants this
//!
//! So it can hide controls the hardware does not have. A unit with no horizontal
//! flap should not offer left-right swing, and a panel that offers it anyway
//! produces a button that silently does nothing — the firmware ignores an
//! unsupported swing mode rather than refusing it.

use std::collections::BTreeSet;

/// Capability ids this decoder understands. The numbering is the vendor's.
mod id {
    pub const SWING_UD_ANGLE: u16 = 0x0009;
    pub const SWING_LR_ANGLE: u16 = 0x000A;
    pub const BREEZELESS: u16 = 0x0018;
    pub const SMART_EYE: u16 = 0x0030;
    pub const WIND_ON_ME: u16 = 0x0032;
    pub const WIND_OFF_ME: u16 = 0x0033;
    pub const SELF_CLEAN: u16 = 0x0039;
    /// Seen in real logs with no known meaning. Recognised so it is not reported
    /// as unknown on every single unit.
    pub const UNKNOWN_0040: u16 = 0x0040;
    pub const BREEZE_AWAY: u16 = 0x0042;
    pub const BREEZE_CONTROL: u16 = 0x0043;
    pub const FRESH_AIR: u16 = 0x004B;
    pub const CASCADE: u16 = 0x0059;
    pub const FLASH: u16 = 0x0067;
    pub const AUX_FAN_SPEED_CONTROL: u16 = 0x0093;
    pub const AUX_HEAT_FAN_SPEED_CONTROL: u16 = 0x0094;
    pub const OUT_SILENT: u16 = 0x00CD;
    pub const FAN_SPEED_CONTROL: u16 = 0x0210;
    pub const PRESET_ECO: u16 = 0x0212;
    pub const PRESET_FREEZE_PROTECTION: u16 = 0x0213;
    pub const MODES: u16 = 0x0214;
    pub const SWING_MODES: u16 = 0x0215;
    pub const ENERGY: u16 = 0x0216;
    pub const FILTER_REMIND: u16 = 0x0217;
    pub const AUX_ELECTRIC_HEAT: u16 = 0x0219;
    pub const PRESET_TURBO: u16 = 0x021A;
    pub const ANION: u16 = 0x021E;
    pub const HUMIDITY: u16 = 0x021F;
    pub const FAHRENHEIT: u16 = 0x0222;
    pub const DISPLAY_CONTROL: u16 = 0x0224;
    pub const TEMPERATURES: u16 = 0x0225;
    pub const BUZZER: u16 = 0x022C;
}

/// Everything a unit reported, before it is turned into something a client can
/// use.
///
/// Fields are `false` until a record says otherwise, which matches the
/// reference: an absent capability is an absent feature.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Capabilities {
    // Operating modes.
    pub cool_mode: bool,
    pub heat_mode: bool,
    pub dry_mode: bool,
    pub auto_mode: bool,
    pub aux_heat_mode: bool,
    pub aux_mode: bool,
    pub aux_electric_heat: bool,

    // Flaps.
    pub swing_horizontal: bool,
    pub swing_vertical: bool,
    pub swing_horizontal_angle: bool,
    pub swing_vertical_angle: bool,

    // Fan. Read through the accessors, not directly: a unit that reported no fan
    // capability at all gets a default set rather than "no fan speeds".
    fan_silent: bool,
    fan_low: bool,
    fan_medium: bool,
    fan_high: bool,
    fan_auto: bool,
    /// Custom (arbitrary percentage) speeds.
    pub fan_custom: bool,
    /// Whether *any* fan record was present.
    saw_fan_capability: bool,

    // Presets and extras.
    pub eco: bool,
    pub turbo_cool: bool,
    pub turbo_heat: bool,
    pub freeze_protection: bool,
    pub display_control: bool,
    pub anion: bool,
    pub filter_notice: bool,
    pub filter_clean: bool,
    pub humidity_auto_set: bool,
    pub humidity_manual_set: bool,
    pub self_clean: bool,
    pub breeze_away: bool,
    pub breeze_control: bool,
    pub breezeless: bool,
    pub buzzer: bool,
    pub fahrenheit: bool,
    pub energy_stats: bool,
    pub smart_eye: bool,
    pub wind_on_me: bool,
    pub wind_off_me: bool,
    pub fresh_air: bool,
    pub cascade: bool,
    pub flash: bool,
    pub out_silent: bool,
    pub aux_fan_speed: bool,
    pub aux_heat_fan_speed: bool,

    // Temperature limits, per mode, in whole or half degrees.
    pub cool_min_temperature: Option<f64>,
    pub cool_max_temperature: Option<f64>,
    pub auto_min_temperature: Option<f64>,
    pub auto_max_temperature: Option<f64>,
    pub heat_min_temperature: Option<f64>,
    pub heat_max_temperature: Option<f64>,
    /// Whether the unit accepts half degrees.
    pub decimals: bool,

    /// The unit says it has more to report, so a second query is worth sending.
    pub additional: bool,
    /// Ids that appeared and are not interpreted here. Kept so a unit with
    /// features this build does not know about is visible rather than silently
    /// reduced.
    pub unknown_ids: BTreeSet<u16>,
}

/// `true` if the byte is one of `accepted`.
fn one_of(value: &[u8], accepted: &[u8]) -> bool {
    value.first().is_some_and(|v| accepted.contains(v))
}

impl Capabilities {
    /// Decode a `0xB5` capability payload.
    ///
    /// `payload` starts at the command byte, as the frame layer hands it over:
    /// `[0xB5, count, records…]`. Never fails — a malformed or truncated record
    /// stops the walk, because half a capability list is more useful than an
    /// error, and this data arrives from firmware nobody controls.
    pub fn parse(payload: &[u8]) -> Self {
        let mut caps = Self::default();
        if payload.len() < 2 {
            return caps;
        }
        let count = payload[1] as usize;
        let mut rest = &payload[2..];

        for _ in 0..count {
            if rest.len() < 3 {
                break;
            }
            let id = u16::from_le_bytes([rest[0], rest[1]]);
            let size = rest[2] as usize;
            if size == 0 {
                // An empty record still occupies its header.
                rest = &rest[3..];
                continue;
            }
            if rest.len() < 3 + size {
                break;
            }
            let value = &rest[3..3 + size];
            caps.apply(id, value, rest);
            rest = &rest[3 + size..];
        }

        // A trailing byte says whether a second query would return more.
        if rest.len() > 1 {
            caps.additional = rest[rest.len() - 2] != 0;
        }
        caps
    }

    /// Apply one record.
    ///
    /// `record` is the whole record including its header, which the temperature
    /// capability needs: it reads by absolute offset, as the reference does.
    fn apply(&mut self, id: u16, value: &[u8], record: &[u8]) {
        match id {
            id::MODES => {
                self.heat_mode = one_of(value, &[1, 2, 4, 6, 7, 9, 10, 11, 12, 13]);
                self.cool_mode = one_of(value, &[0, 1, 3, 4, 5, 6, 7, 8, 9, 11, 13, 14, 15]);
                self.dry_mode = one_of(value, &[0, 1, 5, 6, 9, 11, 13, 14, 15]);
                self.auto_mode = one_of(value, &[0, 1, 2, 7, 8, 9, 13, 14]);
                self.aux_heat_mode = one_of(value, &[9]);
                self.aux_mode = one_of(value, &[9, 10, 11, 13, 14, 15]);
            }
            id::SWING_MODES => {
                self.swing_horizontal = one_of(value, &[1, 3]);
                self.swing_vertical = one_of(value, &[0, 1]);
            }
            id::SWING_UD_ANGLE => self.swing_vertical_angle = one_of(value, &[1]),
            id::SWING_LR_ANGLE => self.swing_horizontal_angle = one_of(value, &[1]),
            id::FAN_SPEED_CONTROL => {
                self.saw_fan_capability = true;
                self.fan_silent = one_of(value, &[6, 9]);
                self.fan_low = one_of(value, &[3, 4, 5, 6, 7, 9]);
                self.fan_medium = one_of(value, &[5, 6, 7]);
                self.fan_high = one_of(value, &[3, 4, 5, 6, 7, 9]);
                self.fan_auto = one_of(value, &[4, 5, 6, 9]);
                self.fan_custom = one_of(value, &[1]);
            }
            id::PRESET_ECO => self.eco = one_of(value, &[1, 2]),
            id::PRESET_TURBO => {
                self.turbo_heat = one_of(value, &[1, 3]);
                self.turbo_cool = one_of(value, &[0, 1]);
            }
            id::PRESET_FREEZE_PROTECTION => self.freeze_protection = one_of(value, &[1]),
            id::DISPLAY_CONTROL => self.display_control = one_of(value, &[1, 2, 100]),
            id::ANION => self.anion = one_of(value, &[1]),
            id::FILTER_REMIND => {
                self.filter_notice = one_of(value, &[1, 2, 4]);
                self.filter_clean = one_of(value, &[3, 4]);
            }
            id::HUMIDITY => {
                self.humidity_auto_set = one_of(value, &[1, 2]);
                self.humidity_manual_set = one_of(value, &[2, 3]);
            }
            id::ENERGY => self.energy_stats = one_of(value, &[2, 3, 4, 5]),
            id::FAHRENHEIT => self.fahrenheit = one_of(value, &[0]),
            id::BUZZER => self.buzzer = one_of(value, &[1]),
            id::SELF_CLEAN => self.self_clean = one_of(value, &[1]),
            id::BREEZE_AWAY => self.breeze_away = one_of(value, &[1]),
            id::BREEZE_CONTROL => self.breeze_control = one_of(value, &[1]),
            id::BREEZELESS => self.breezeless = one_of(value, &[1]),
            id::SMART_EYE => self.smart_eye = one_of(value, &[1]),
            id::WIND_ON_ME => self.wind_on_me = one_of(value, &[1]),
            id::WIND_OFF_ME => self.wind_off_me = one_of(value, &[1]),
            id::FRESH_AIR => self.fresh_air = one_of(value, &[1]),
            id::CASCADE => self.cascade = one_of(value, &[1]),
            id::FLASH => self.flash = one_of(value, &[1, 2, 3, 4]),
            id::OUT_SILENT => self.out_silent = one_of(value, &[1, 3]),
            id::AUX_ELECTRIC_HEAT => self.aux_electric_heat = one_of(value, &[1]),
            id::AUX_FAN_SPEED_CONTROL => self.aux_fan_speed = one_of(value, &[1]),
            id::AUX_HEAT_FAN_SPEED_CONTROL => self.aux_heat_fan_speed = one_of(value, &[1]),
            id::TEMPERATURES => {
                // Read by absolute offset into the record, per the reference,
                // and only when there is enough of it.
                if record.len() < 9 || value.len() < 6 {
                    return;
                }
                self.cool_min_temperature = Some(record[3] as f64 * 0.5);
                self.cool_max_temperature = Some(record[4] as f64 * 0.5);
                self.auto_min_temperature = Some(record[5] as f64 * 0.5);
                self.auto_max_temperature = Some(record[6] as f64 * 0.5);
                self.heat_min_temperature = Some(record[7] as f64 * 0.5);
                self.heat_max_temperature = Some(record[8] as f64 * 0.5);
                self.decimals = if value.len() > 6 && record.len() > 9 {
                    record[9] != 0
                } else {
                    record[2] != 0
                };
            }
            // Known, and nothing is derived from it.
            id::UNKNOWN_0040 => {}
            other => {
                self.unknown_ids.insert(other);
            }
        }
    }

    /// Fold a second response into this one.
    ///
    /// A unit that sets `additional` answers a second query with the rest.
    /// Merging is a logical OR: the second response reports what the first left
    /// out, never the reverse.
    pub fn merge(&mut self, other: &Capabilities) {
        macro_rules! or {
            ($($field:ident),* $(,)?) => { $( self.$field |= other.$field; )* };
        }
        or!(
            cool_mode,
            heat_mode,
            dry_mode,
            auto_mode,
            aux_heat_mode,
            aux_mode,
            aux_electric_heat,
            swing_horizontal,
            swing_vertical,
            swing_horizontal_angle,
            swing_vertical_angle,
            fan_silent,
            fan_low,
            fan_medium,
            fan_high,
            fan_auto,
            fan_custom,
            saw_fan_capability,
            eco,
            turbo_cool,
            turbo_heat,
            freeze_protection,
            display_control,
            anion,
            filter_notice,
            filter_clean,
            humidity_auto_set,
            humidity_manual_set,
            self_clean,
            breeze_away,
            breeze_control,
            breezeless,
            buzzer,
            fahrenheit,
            energy_stats,
            smart_eye,
            wind_on_me,
            wind_off_me,
            fresh_air,
            cascade,
            flash,
            out_silent,
            aux_fan_speed,
            aux_heat_fan_speed,
            decimals,
        );
        // Temperatures: whichever response actually carried them.
        for (mine, theirs) in [
            (&mut self.cool_min_temperature, other.cool_min_temperature),
            (&mut self.cool_max_temperature, other.cool_max_temperature),
            (&mut self.auto_min_temperature, other.auto_min_temperature),
            (&mut self.auto_max_temperature, other.auto_max_temperature),
            (&mut self.heat_min_temperature, other.heat_min_temperature),
            (&mut self.heat_max_temperature, other.heat_max_temperature),
        ] {
            if mine.is_none() {
                *mine = theirs;
            }
        }
        self.unknown_ids.extend(other.unknown_ids.iter().copied());
        // Not OR-ed: the second response is the authority on whether a *third*
        // is needed, and it says no.
        self.additional = other.additional;
    }

    /// Whether a given named fan speed is available.
    ///
    /// A unit that reported no fan capability at all gets the reference's
    /// default set rather than "no fan speeds", which would leave a client
    /// unable to offer any. And a unit that supports custom speeds is taken to
    /// support every named one.
    fn fan(&self, named: bool) -> bool {
        if self.saw_fan_capability {
            named || self.fan_custom
        } else {
            false
        }
    }

    pub fn supports_fan_silent(&self) -> bool {
        self.fan(self.fan_silent)
    }

    pub fn supports_fan_low(&self) -> bool {
        if self.saw_fan_capability {
            self.fan(self.fan_low)
        } else {
            true
        }
    }

    pub fn supports_fan_medium(&self) -> bool {
        if self.saw_fan_capability {
            self.fan(self.fan_medium)
        } else {
            true
        }
    }

    pub fn supports_fan_high(&self) -> bool {
        if self.saw_fan_capability {
            self.fan(self.fan_high)
        } else {
            true
        }
    }

    pub fn supports_fan_auto(&self) -> bool {
        if self.saw_fan_capability {
            self.fan(self.fan_auto)
        } else {
            true
        }
    }

    /// Both flaps, which is what `BOTH` needs.
    pub fn swing_both(&self) -> bool {
        self.swing_horizontal && self.swing_vertical
    }

    pub fn turbo(&self) -> bool {
        self.turbo_cool || self.turbo_heat
    }

    /// The narrowest minimum across the three modes.
    ///
    /// Each unreported mode counts as the API's own 16, exactly as the reference
    /// does it -- so a unit that says nothing yields 16, and one that reports a
    /// wider cool range yields that.
    pub fn min_temperature(&self) -> f64 {
        [
            self.cool_min_temperature,
            self.auto_min_temperature,
            self.heat_min_temperature,
        ]
        .iter()
        .map(|v| v.unwrap_or(16.0))
        .fold(f64::INFINITY, f64::min)
    }

    /// The widest maximum across the three modes, unreported ones counting as 30.
    pub fn max_temperature(&self) -> f64 {
        [
            self.cool_max_temperature,
            self.auto_max_temperature,
            self.heat_max_temperature,
        ]
        .iter()
        .map(|v| v.unwrap_or(30.0))
        .fold(f64::NEG_INFINITY, f64::max)
    }

    /// Mode names a client may offer, in the reference's order.
    ///
    /// `FAN_ONLY` is unconditional: every unit can move air, and the reference
    /// seeds the list with it before consulting anything.
    pub fn operational_modes(&self) -> Vec<&'static str> {
        let mut modes = vec!["FAN_ONLY"];
        if self.dry_mode {
            modes.push("DRY");
        }
        if self.cool_mode {
            modes.push("COOL");
        }
        if self.heat_mode {
            modes.push("HEAT");
        }
        if self.auto_mode {
            modes.push("AUTO");
        }
        modes
    }

    /// Swing modes a client may offer. `OFF` is always possible.
    pub fn swing_modes(&self) -> Vec<&'static str> {
        let mut modes = vec!["OFF"];
        if self.swing_horizontal {
            modes.push("HORIZONTAL");
        }
        if self.swing_vertical {
            modes.push("VERTICAL");
        }
        if self.swing_both() {
            modes.push("BOTH");
        }
        modes
    }

    /// Fan speeds a client may offer, as the enum **names** the reference sends.
    ///
    /// Names, not the numbers `POST /control` takes -- which is a quirk of the
    /// reference rather than a choice: `serialize_capabilities` reads `.name` off
    /// each supported speed. Clients read this list, so it matches. The order is
    /// the reference's too, with MAX last because it is appended only when custom
    /// speeds are supported.
    pub fn fan_speeds(&self) -> Vec<&'static str> {
        let mut speeds = Vec::new();
        if self.supports_fan_silent() {
            speeds.push("SILENT");
        }
        if self.supports_fan_low() {
            speeds.push("LOW");
        }
        if self.supports_fan_medium() {
            speeds.push("MEDIUM");
        }
        if self.supports_fan_high() {
            speeds.push("HIGH");
        }
        if self.supports_fan_auto() {
            speeds.push("AUTO");
        }
        // A unit that takes any percentage also exposes the 100% step.
        if self.fan_custom {
            speeds.push("MAX");
        }
        speeds
    }

    /// The same list as the numbers `POST /control` accepts.
    ///
    /// Not on the wire; useful to anything validating a speed against what the
    /// hardware actually has.
    pub fn fan_speed_values(&self) -> Vec<u8> {
        self.fan_speeds()
            .into_iter()
            .map(|name| match name {
                "SILENT" => 20,
                "LOW" => 40,
                "MEDIUM" => 60,
                "HIGH" => 80,
                "MAX" => 100,
                _ => 102,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a payload: `[0xB5, count, (id, size, value)...]`.
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
    fn an_empty_payload_is_no_capabilities_rather_than_a_panic() {
        for short in [&[][..], &[0xB5][..], &[0xB5, 0][..]] {
            let caps = Capabilities::parse(short);
            assert_eq!(caps, Capabilities::default());
        }
    }

    #[test]
    fn modes_come_from_one_byte_meaning_a_whole_set() {
        // Value 1 is the common "everything" case for these units.
        let caps = Capabilities::parse(&payload(&[(id::MODES, &[1])]));
        assert!(caps.cool_mode && caps.heat_mode && caps.dry_mode && caps.auto_mode);
        assert_eq!(
            caps.operational_modes(),
            vec!["FAN_ONLY", "DRY", "COOL", "HEAT", "AUTO"]
        );

        // Value 3 is cool-only: no heat, no dry, no auto.
        let caps = Capabilities::parse(&payload(&[(id::MODES, &[3])]));
        assert!(caps.cool_mode);
        assert!(!caps.heat_mode);
        assert!(!caps.dry_mode);
        assert!(!caps.auto_mode);
        assert_eq!(caps.operational_modes(), vec!["FAN_ONLY", "COOL"]);
    }

    #[test]
    fn fan_only_is_always_offered() {
        // Even for a unit that reported no modes at all: it can still move air,
        // and a client with an empty mode list can do nothing.
        let caps = Capabilities::default();
        assert_eq!(caps.operational_modes(), vec!["FAN_ONLY"]);
    }

    #[test]
    fn swing_modes_derive_both_from_the_two_axes() {
        // 1 = both axes.
        let caps = Capabilities::parse(&payload(&[(id::SWING_MODES, &[1])]));
        assert!(caps.swing_horizontal && caps.swing_vertical);
        assert!(caps.swing_both());
        assert_eq!(
            caps.swing_modes(),
            vec!["OFF", "HORIZONTAL", "VERTICAL", "BOTH"]
        );

        // 0 = vertical only, so BOTH must not be offered -- the exact case that
        // makes this endpoint worth having.
        let caps = Capabilities::parse(&payload(&[(id::SWING_MODES, &[0])]));
        assert!(!caps.swing_horizontal);
        assert!(caps.swing_vertical);
        assert!(!caps.swing_both());
        assert_eq!(caps.swing_modes(), vec!["OFF", "VERTICAL"]);

        // 3 = horizontal only.
        let caps = Capabilities::parse(&payload(&[(id::SWING_MODES, &[3])]));
        assert_eq!(caps.swing_modes(), vec!["OFF", "HORIZONTAL"]);
    }

    #[test]
    fn a_unit_that_reports_no_fan_capability_gets_the_default_set() {
        // Not an empty list: a client with no fan speeds to offer is worse than
        // one offering the four every unit has.
        let caps = Capabilities::default();
        assert!(!caps.supports_fan_silent(), "silent is not assumed");
        assert!(caps.supports_fan_low());
        assert!(caps.supports_fan_medium());
        assert!(caps.supports_fan_high());
        assert!(caps.supports_fan_auto());
        assert_eq!(caps.fan_speeds(), vec!["LOW", "MEDIUM", "HIGH", "AUTO"]);
        assert_eq!(caps.fan_speed_values(), vec![40, 60, 80, 102]);
    }

    #[test]
    fn custom_fan_support_implies_every_named_speed() {
        // Value 1 is "custom": a unit that takes any percentage can manage the
        // named ones.
        let caps = Capabilities::parse(&payload(&[(id::FAN_SPEED_CONTROL, &[1])]));
        assert!(caps.fan_custom);
        assert!(caps.supports_fan_silent());
        assert!(caps.supports_fan_low());
        assert!(caps.supports_fan_auto());
        assert_eq!(
            caps.fan_speeds(),
            vec!["SILENT", "LOW", "MEDIUM", "HIGH", "AUTO", "MAX"]
        );
    }

    #[test]
    fn a_restricted_fan_reports_only_what_it_has() {
        // 3 = low and high, no medium, no auto, no silent.
        let caps = Capabilities::parse(&payload(&[(id::FAN_SPEED_CONTROL, &[3])]));
        assert!(!caps.supports_fan_silent());
        assert!(caps.supports_fan_low());
        assert!(!caps.supports_fan_medium());
        assert!(caps.supports_fan_high());
        assert!(!caps.supports_fan_auto());
        assert_eq!(caps.fan_speeds(), vec!["LOW", "HIGH"]);
    }

    #[test]
    fn turbo_is_either_direction() {
        // 1 = both; 0 = cool only; 3 = heat only.
        assert!(Capabilities::parse(&payload(&[(id::PRESET_TURBO, &[1])])).turbo());
        assert!(Capabilities::parse(&payload(&[(id::PRESET_TURBO, &[0])])).turbo());
        assert!(Capabilities::parse(&payload(&[(id::PRESET_TURBO, &[3])])).turbo());
        assert!(!Capabilities::default().turbo());
    }

    #[test]
    fn temperature_limits_are_half_degree_units() {
        // Six bytes: cool min/max, auto min/max, heat min/max, each doubled.
        let caps = Capabilities::parse(&payload(&[(id::TEMPERATURES, &[32, 60, 32, 60, 32, 60])]));
        assert_eq!(caps.cool_min_temperature, Some(16.0));
        assert_eq!(caps.cool_max_temperature, Some(30.0));
        assert_eq!(caps.min_temperature(), 16.0);
        assert_eq!(caps.max_temperature(), 30.0);
    }

    #[test]
    fn a_wider_range_than_the_api_allows_is_reported_as_the_unit_gave_it() {
        // A unit claiming 10-32 is reported as 10-32 here; the API's own 16-30
        // bound is enforced where a setpoint is applied, not by pretending the
        // hardware said something else.
        let caps = Capabilities::parse(&payload(&[(id::TEMPERATURES, &[20, 64, 20, 64, 20, 64])]));
        assert_eq!(caps.cool_min_temperature, Some(10.0));
        assert_eq!(caps.max_temperature(), 32.0);
    }

    #[test]
    fn a_truncated_temperature_record_is_skipped() {
        let caps = Capabilities::parse(&payload(&[(id::TEMPERATURES, &[32, 60])]));
        assert_eq!(caps.cool_min_temperature, None);
        // And the defaults still answer.
        assert_eq!(caps.min_temperature(), 16.0);
        assert_eq!(caps.max_temperature(), 30.0);
    }

    #[test]
    fn several_records_are_all_read() {
        let caps = Capabilities::parse(&payload(&[
            (id::MODES, &[1]),
            (id::SWING_MODES, &[1]),
            (id::PRESET_ECO, &[1]),
            (id::PRESET_TURBO, &[1]),
            (id::DISPLAY_CONTROL, &[1]),
            (id::ANION, &[1]),
        ]));
        assert!(caps.cool_mode);
        assert!(caps.swing_both());
        assert!(caps.eco);
        assert!(caps.turbo());
        assert!(caps.display_control);
        assert!(caps.anion);
    }

    #[test]
    fn an_empty_record_is_skipped_without_losing_the_rest() {
        // A zero-size record still occupies its three header bytes; mis-stepping
        // here would misread every capability after it.
        let mut raw = vec![0xB5, 2];
        raw.extend_from_slice(&id::MODES.to_le_bytes());
        raw.push(0); // empty
        raw.extend_from_slice(&id::PRESET_ECO.to_le_bytes());
        raw.push(1);
        raw.push(1);
        let caps = Capabilities::parse(&raw);
        assert!(!caps.cool_mode, "the empty record set nothing");
        assert!(caps.eco, "and the one after it was still read");
    }

    #[test]
    fn an_unknown_id_is_recorded_rather_than_ignored() {
        // So a unit with features this build does not know about is visible.
        let caps = Capabilities::parse(&payload(&[(0x0999, &[1]), (id::PRESET_ECO, &[1])]));
        assert!(caps.unknown_ids.contains(&0x0999));
        assert!(caps.eco, "parsing continued past it");
    }

    #[test]
    fn a_truncated_payload_stops_cleanly() {
        // Every prefix of a well-formed payload, since a short read is a real
        // possibility on a flaky unit.
        let full = payload(&[(id::MODES, &[1]), (id::SWING_MODES, &[1])]);
        for cut in 0..full.len() {
            let _ = Capabilities::parse(&full[..cut]);
        }
    }

    #[test]
    fn a_count_larger_than_the_data_does_not_over_read() {
        // A unit claiming ten records and sending one.
        let mut raw = vec![0xB5, 10];
        raw.extend_from_slice(&id::PRESET_ECO.to_le_bytes());
        raw.push(1);
        raw.push(1);
        let caps = Capabilities::parse(&raw);
        assert!(caps.eco);
    }

    #[test]
    fn merging_adds_what_the_second_response_reported() {
        let mut first = Capabilities::parse(&payload(&[(id::MODES, &[1])]));
        let second = Capabilities::parse(&payload(&[
            (id::PRESET_ECO, &[1]),
            (id::TEMPERATURES, &[32, 60, 32, 60, 32, 60]),
        ]));
        first.merge(&second);
        assert!(first.cool_mode, "the first response survives");
        assert!(first.eco, "and the second is folded in");
        assert_eq!(first.cool_min_temperature, Some(16.0));
        assert!(
            !first.additional,
            "the second response decides whether a third is needed"
        );
    }

    #[test]
    fn merging_never_removes_a_capability() {
        let mut first = Capabilities::parse(&payload(&[(id::PRESET_ECO, &[1])]));
        let second = Capabilities::default();
        first.merge(&second);
        assert!(first.eco, "an absent capability is not a denial");
    }
}
