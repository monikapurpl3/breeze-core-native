//! Program logic: validating what a client sends, and working out what a
//! schedule fires or a curve is asking for.
//!
//! Kept here, beside the models and free of I/O, because it is all arithmetic
//! and validation over stored data -- testable without a scheduler, a socket or
//! an air conditioner.

use chrono::{NaiveDateTime, Timelike};
use serde::Deserialize;

use crate::control::{
    ControlRequest, ValidationError, ALLOWED_FAN_SPEEDS, OPERATIONAL_MODES, TEMP_MAX, TEMP_MIN,
};
use crate::models::{CurveConfig, CurvePoint, Program, ProgramsDoc, ScheduleEntry};

const MINUTES_PER_DAY: i64 = 1440;

/// Parse `HH:MM` into minutes past midnight.
pub fn minutes_of_day(hhmm: &str) -> Option<i64> {
    let (h, m) = hhmm.split_once(':')?;
    let h: i64 = h.parse().ok()?;
    let m: i64 = m.parse().ok()?;
    if !(0..24).contains(&h) || !(0..60).contains(&m) {
        return None;
    }
    Some(h * 60 + m)
}

/// Snap to the nearest 0.5°, clamped to the API's 16–30 range.
///
/// Uses **banker's rounding** — half to even — because that is what Python's
/// `round()` does, and this value is compared against what the Python
/// implementation would have set. Rust's `f64::round` rounds half *away from
/// zero*, so 20.25 would become 20.5 here and 20.0 there: a visible half-degree
/// disagreement on exactly the midpoints a curve produces.
pub fn round_half(t: f64) -> f64 {
    let doubled = t * 2.0;
    let rounded = if (doubled - doubled.trunc()).abs() == 0.5 {
        // Exactly halfway: pick the even neighbour.
        let lower = doubled.floor();
        if (lower as i64) % 2 == 0 {
            lower
        } else {
            lower + 1.0
        }
    } else {
        doubled.round()
    };
    (rounded / 2.0).clamp(16.0, 30.0)
}

/// The interpolated setpoint for `now`, treating the points as a cyclic 24-hour
/// curve.
///
/// Cyclic matters: a curve with points at 08:00 and 22:00 has to say something
/// at 03:00, and the answer is a position on the segment that wraps through
/// midnight rather than "nothing".
pub fn curve_setpoint(points: &[CurvePoint], now: NaiveDateTime) -> Option<f64> {
    if points.is_empty() {
        return None;
    }
    let mut pts: Vec<(i64, f64)> = points
        .iter()
        .filter_map(|p| minutes_of_day(&p.time).map(|m| (m, p.temperature)))
        .collect();
    if pts.is_empty() {
        return None;
    }
    pts.sort_by_key(|(m, _)| *m);
    if pts.len() == 1 {
        return Some(round_half(pts[0].1));
    }

    let mins = now.hour() as i64 * 60 + now.minute() as i64;
    let n = pts.len();
    for i in 0..n {
        let (am, at) = pts[i];
        let (bm, bt) = pts[(i + 1) % n];
        // A single-segment wrap spans the whole day rather than zero minutes.
        let span = match (bm - am).rem_euclid(MINUTES_PER_DAY) {
            0 => MINUTES_PER_DAY,
            s => s,
        };
        let off = (mins - am).rem_euclid(MINUTES_PER_DAY);
        if off < span {
            let frac = off as f64 / span as f64;
            return Some(round_half(at + (bt - at) * frac));
        }
    }
    Some(round_half(pts[0].1))
}

/// What a curve wants applied right now.
///
/// A curve implies the unit is on and in its configured mode: driving only the
/// setpoint would leave a switched-off unit switched off and a curve that
/// silently does nothing.
///
/// `target_temperature` is left `None` when the curve has no points, and the two
/// callers then diverge -- deliberately, matching the reference. `POST
/// /{id}/apply` sends the request anyway, so an empty curve still switches the
/// unit on in its configured mode; the scheduler skips the program entirely. Odd
/// on its face, but it is the behaviour clients have, so it is not ours to
/// tidy up.
pub fn curve_request(curve: &CurveConfig, now: NaiveDateTime) -> ControlRequest {
    ControlRequest {
        power_state: Some(true),
        operational_mode: Some(curve.operational_mode.clone()),
        fan_speed: Some(curve.fan_speed),
        target_temperature: curve_setpoint(&curve.points, now),
        ..Default::default()
    }
}

/// Which schedule entries are due at `now`, by index.
///
/// Matching is to the *minute*: an entry fires when the clock reads its time, and
/// the caller is responsible for not firing it twice within that minute. Days are
/// Monday = 0, as `datetime.weekday()` gives; an empty list means every day.
pub fn due_entries(entries: &[ScheduleEntry], now: NaiveDateTime) -> Vec<usize> {
    use chrono::Datelike;
    let weekday = now.weekday().num_days_from_monday() as u8;
    let hhmm = format!("{:02}:{:02}", now.hour(), now.minute());
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.days.is_empty() || e.days.contains(&weekday))
        .filter(|(_, e)| e.time == hhmm)
        .map(|(i, _)| i)
        .collect()
}

/// The minute stamp used to fire a schedule entry at most once.
pub fn minute_stamp(now: NaiveDateTime) -> String {
    now.format("%Y-%m-%d %H:%M").to_string()
}

impl Program {
    /// Units this program targets, falling back to every configured unit.
    pub fn targets(&self, all: &[String]) -> Vec<String> {
        if self.unit_ids.is_empty() {
            all.to_vec()
        } else {
            self.unit_ids.clone()
        }
    }

    pub fn is_schedule(&self) -> bool {
        self.kind == "schedule"
    }

    pub fn is_curve(&self) -> bool {
        self.kind == "curve"
    }
}

// ------------------------------------------------------------------ ingest

/// The three kinds a program can be.
pub const PROGRAM_KINDS: [&str; 3] = ["favourite", "schedule", "curve"];

/// Matches the reference's `Field(min_length=1, max_length=64)`. Pydantic counts
/// characters, not bytes, so this does too — a 64-emoji name is legal there and
/// must stay legal here.
pub const NAME_MAX_CHARS: usize = 64;

/// The client-supplied shape of a program: everything except the id, which the
/// server assigns.
#[derive(Debug, Clone, Deserialize)]
pub struct ProgramSpec {
    pub name: String,
    #[serde(default = "spec_default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub unit_ids: Vec<String>,
    #[serde(default = "spec_default_kind")]
    pub kind: String,
    #[serde(default)]
    pub favourite: Option<ControlRequest>,
    #[serde(default)]
    pub schedule: Vec<ScheduleEntry>,
    #[serde(default)]
    pub curve: Option<CurveConfig>,
}

fn spec_default_true() -> bool {
    true
}

fn spec_default_kind() -> String {
    "favourite".to_string()
}

#[derive(Debug, PartialEq)]
pub enum ProgramError {
    EmptyName,
    NameTooLong(usize),
    UnknownKind(String),
    BadTime(String),
    BadDay(u8),
    TemperatureOutOfRange(f64),
    UnknownMode(String),
    UnknownFanSpeed(i64),
    Settings(ValidationError),
}

impl core::fmt::Display for ProgramError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyName => write!(f, "name must not be empty"),
            Self::NameTooLong(n) => {
                write!(
                    f,
                    "name must be at most {NAME_MAX_CHARS} characters, got {n}"
                )
            }
            Self::UnknownKind(k) => write!(
                f,
                "kind must be one of {}, got '{k}'",
                PROGRAM_KINDS.join(", ")
            ),
            Self::BadTime(t) => write!(f, "time must be 'HH:MM' (24h), got '{t}'"),
            Self::BadDay(d) => write!(f, "days must be 0 (Mon) .. 6 (Sun), got {d}"),
            Self::TemperatureOutOfRange(t) => write!(
                f,
                "curve temperatures must be between {TEMP_MIN} and {TEMP_MAX}, got {t}"
            ),
            Self::UnknownMode(m) => write!(
                f,
                "operational_mode must be one of {}, got '{m}'",
                OPERATIONAL_MODES.join(", ")
            ),
            Self::UnknownFanSpeed(s) => write!(
                f,
                "fan_speed must be one of {:?}, got {s}",
                ALLOWED_FAN_SPEEDS
            ),
            Self::Settings(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ProgramError {}

/// Strict `HH:MM`, matching the reference's `^([01]\d|2[0-3]):([0-5]\d)$`.
///
/// Stricter than [`minutes_of_day`], on purpose: input is rejected rather than
/// guessed at, so `7:30` is an error here, while a file already on disk is read
/// as leniently as possible because refusing to start is worse.
pub fn valid_hhmm(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 5 || b[2] != b':' {
        return false;
    }
    if !b
        .iter()
        .enumerate()
        .all(|(i, c)| i == 2 || c.is_ascii_digit())
    {
        return false;
    }
    let h = (b[0] - b'0') * 10 + (b[1] - b'0');
    let m = (b[3] - b'0') * 10 + (b[4] - b'0');
    h <= 23 && m <= 59
}

impl ProgramSpec {
    /// Validate, normalise, and attach the server-assigned id.
    ///
    /// The normalisations are not cosmetic: the reference's validators sort and
    /// dedupe `days` and upper-case a curve's `operational_mode` *before* the
    /// program is stored, so skipping them would write different bytes than
    /// Breeze Core would for the same request.
    pub fn into_program(mut self, id: impl Into<String>) -> Result<Program, ProgramError> {
        let chars = self.name.chars().count();
        if chars == 0 {
            return Err(ProgramError::EmptyName);
        }
        if chars > NAME_MAX_CHARS {
            return Err(ProgramError::NameTooLong(chars));
        }
        if !PROGRAM_KINDS.contains(&self.kind.as_str()) {
            return Err(ProgramError::UnknownKind(self.kind));
        }

        if let Some(fav) = &self.favourite {
            fav.validate().map_err(ProgramError::Settings)?;
        }

        for entry in &mut self.schedule {
            if let Some(bad) = entry.days.iter().find(|d| **d > 6) {
                return Err(ProgramError::BadDay(*bad));
            }
            // `sorted(set(v))`, which changes what gets written.
            entry.days.sort_unstable();
            entry.days.dedup();
            if !valid_hhmm(&entry.time) {
                return Err(ProgramError::BadTime(entry.time.clone()));
            }
            entry.settings.validate().map_err(ProgramError::Settings)?;
        }

        if let Some(curve) = &mut self.curve {
            curve.operational_mode = curve.operational_mode.to_uppercase();
            if !OPERATIONAL_MODES.contains(&curve.operational_mode.as_str()) {
                return Err(ProgramError::UnknownMode(curve.operational_mode.clone()));
            }
            if !ALLOWED_FAN_SPEEDS.contains(&curve.fan_speed) {
                return Err(ProgramError::UnknownFanSpeed(curve.fan_speed));
            }
            for point in &curve.points {
                if !valid_hhmm(&point.time) {
                    return Err(ProgramError::BadTime(point.time.clone()));
                }
                if !(TEMP_MIN..=TEMP_MAX).contains(&point.temperature) {
                    return Err(ProgramError::TemperatureOutOfRange(point.temperature));
                }
            }
        }

        Ok(Program {
            name: self.name,
            enabled: self.enabled,
            unit_ids: self.unit_ids,
            kind: self.kind,
            favourite: self.favourite,
            schedule: self.schedule,
            curve: self.curve,
            id: id.into(),
        })
    }
}

impl ProgramsDoc {
    pub fn get(&self, program_id: &str) -> Option<&Program> {
        self.programs.iter().find(|p| p.id == program_id)
    }

    /// Replace a program in place, keeping its position in the file.
    ///
    /// Position matters only for tidiness, but an update that reordered the file
    /// would make a diff against the pre-migration copy unreadable.
    pub fn replace(&mut self, program_id: &str, updated: Program) -> bool {
        match self.programs.iter_mut().find(|p| p.id == program_id) {
            Some(slot) => {
                *slot = updated;
                true
            }
            None => false,
        }
    }

    pub fn remove(&mut self, program_id: &str) -> bool {
        let before = self.programs.len();
        self.programs.retain(|p| p.id != program_id);
        self.programs.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timers::parse_local;

    fn at(text: &str) -> NaiveDateTime {
        parse_local(text).expect("timestamp")
    }

    fn point(time: &str, temperature: f64) -> CurvePoint {
        CurvePoint {
            time: time.into(),
            temperature,
        }
    }

    #[test]
    fn times_parse_and_reject_nonsense() {
        assert_eq!(minutes_of_day("00:00"), Some(0));
        assert_eq!(minutes_of_day("07:30"), Some(450));
        assert_eq!(minutes_of_day("23:59"), Some(1439));
        assert_eq!(minutes_of_day("24:00"), None);
        assert_eq!(minutes_of_day("12:60"), None);
        assert_eq!(minutes_of_day("noon"), None);
        assert_eq!(minutes_of_day(""), None);
    }

    #[test]
    fn rounding_is_bankers_like_pythons() {
        // The reason this function exists. Rust's f64::round would give 20.5 and
        // 21.5 for the first two, disagreeing with the Python implementation by
        // half a degree at exactly the midpoints a curve produces.
        assert_eq!(round_half(20.25), 20.0, "40.5 rounds to even 40");
        assert_eq!(round_half(20.75), 21.0, "41.5 rounds to even 42");
        assert_eq!(round_half(21.25), 21.0, "42.5 rounds to even 42");
        // Non-midpoints round normally.
        assert_eq!(round_half(20.3), 20.5);
        assert_eq!(round_half(20.2), 20.0);
        assert_eq!(round_half(22.0), 22.0);
    }

    #[test]
    fn rounding_clamps_to_the_api_range() {
        assert_eq!(round_half(5.0), 16.0);
        assert_eq!(round_half(40.0), 30.0);
        assert_eq!(round_half(-100.0), 16.0);
    }

    #[test]
    fn a_curve_with_no_points_asks_for_nothing() {
        assert_eq!(curve_setpoint(&[], at("2026-08-23T12:00:00")), None);
    }

    #[test]
    fn a_single_point_holds_all_day() {
        let pts = [point("08:00", 22.0)];
        for time in [
            "2026-08-23T00:00:00",
            "2026-08-23T08:00:00",
            "2026-08-23T23:59:00",
        ] {
            assert_eq!(curve_setpoint(&pts, at(time)), Some(22.0));
        }
    }

    #[test]
    fn interpolation_is_linear_between_points() {
        let pts = [point("00:00", 20.0), point("12:00", 26.0)];
        assert_eq!(curve_setpoint(&pts, at("2026-08-23T00:00:00")), Some(20.0));
        assert_eq!(curve_setpoint(&pts, at("2026-08-23T06:00:00")), Some(23.0));
        assert_eq!(curve_setpoint(&pts, at("2026-08-23T12:00:00")), Some(26.0));
    }

    #[test]
    fn the_curve_wraps_through_midnight() {
        // 22:00 -> 08:00 is a ten-hour segment crossing midnight. At 03:00 we are
        // half way along it, so the answer must be the midpoint -- not "nothing"
        // and not the first point.
        let pts = [point("08:00", 24.0), point("22:00", 20.0)];
        assert_eq!(curve_setpoint(&pts, at("2026-08-23T22:00:00")), Some(20.0));
        assert_eq!(curve_setpoint(&pts, at("2026-08-24T03:00:00")), Some(22.0));
        assert_eq!(curve_setpoint(&pts, at("2026-08-24T08:00:00")), Some(24.0));
    }

    #[test]
    fn points_out_of_order_are_sorted_first() {
        let jumbled = [point("22:00", 20.0), point("08:00", 24.0)];
        let ordered = [point("08:00", 24.0), point("22:00", 20.0)];
        for time in ["2026-08-23T09:00:00", "2026-08-23T23:00:00"] {
            assert_eq!(
                curve_setpoint(&jumbled, at(time)),
                curve_setpoint(&ordered, at(time)),
                "order of the stored points must not matter"
            );
        }
    }

    #[test]
    fn an_unparseable_point_is_skipped_not_fatal() {
        let pts = [point("nonsense", 20.0), point("08:00", 24.0)];
        assert_eq!(curve_setpoint(&pts, at("2026-08-23T09:00:00")), Some(24.0));
    }

    #[test]
    fn a_curve_switches_the_unit_on_and_sets_its_mode() {
        // Driving only the setpoint would leave a switched-off unit off, and the
        // curve would appear to do nothing at all.
        let curve = CurveConfig {
            operational_mode: "HEAT".into(),
            fan_speed: 60,
            points: vec![point("00:00", 20.0), point("12:00", 24.0)],
        };
        let req = curve_request(&curve, at("2026-08-23T06:00:00"));
        assert_eq!(req.power_state, Some(true));
        assert_eq!(req.operational_mode.as_deref(), Some("HEAT"));
        assert_eq!(req.fan_speed, Some(60));
        assert_eq!(req.target_temperature, Some(22.0));
        // And nothing else is touched.
        assert_eq!(req.eco, None);
        assert_eq!(req.turbo, None);
    }

    #[test]
    fn a_pointless_curve_still_asks_for_mode_and_fan() {
        // The reference builds the request with `target_temperature=None` rather
        // than bailing out, so the apply route switches the unit on anyway. The
        // scheduler is the one that skips -- see the note on curve_request.
        let curve = CurveConfig {
            operational_mode: "COOL".into(),
            fan_speed: 102,
            points: vec![],
        };
        let req = curve_request(&curve, at("2026-08-23T06:00:00"));
        assert_eq!(req.target_temperature, None);
        assert_eq!(req.power_state, Some(true));
        assert_eq!(req.fan_speed, Some(102));
    }

    fn entry(days: Vec<u8>, time: &str) -> ScheduleEntry {
        ScheduleEntry {
            days,
            time: time.into(),
            settings: ControlRequest::power_off(),
        }
    }

    #[test]
    fn an_entry_is_due_only_on_its_minute() {
        let entries = [entry(vec![], "07:30")];
        // 2026-08-24 is a Monday.
        assert_eq!(due_entries(&entries, at("2026-08-24T07:30:00")), vec![0]);
        assert!(due_entries(&entries, at("2026-08-24T07:29:00")).is_empty());
        assert!(due_entries(&entries, at("2026-08-24T07:31:00")).is_empty());
        // Any second within the minute still counts.
        assert_eq!(due_entries(&entries, at("2026-08-24T07:30:59")), vec![0]);
    }

    #[test]
    fn days_are_monday_zero_as_python_counts_them() {
        let weekdays = [entry(vec![0, 1, 2, 3, 4], "07:30")];
        // Monday the 24th fires; Saturday the 22nd does not.
        assert_eq!(due_entries(&weekdays, at("2026-08-24T07:30:00")), vec![0]);
        assert!(due_entries(&weekdays, at("2026-08-22T07:30:00")).is_empty());

        let sunday_only = [entry(vec![6], "07:30")];
        // The 23rd is a Sunday.
        assert_eq!(
            due_entries(&sunday_only, at("2026-08-23T07:30:00")),
            vec![0]
        );
        assert!(due_entries(&sunday_only, at("2026-08-24T07:30:00")).is_empty());
    }

    #[test]
    fn an_empty_day_list_means_every_day() {
        let daily = [entry(vec![], "07:30")];
        for day in ["2026-08-22", "2026-08-23", "2026-08-24"] {
            assert_eq!(
                due_entries(&daily, at(&format!("{day}T07:30:00"))),
                vec![0],
                "should fire on {day}"
            );
        }
    }

    #[test]
    fn several_entries_at_the_same_minute_all_fire() {
        let entries = [entry(vec![], "07:30"), entry(vec![], "07:30")];
        assert_eq!(due_entries(&entries, at("2026-08-24T07:30:00")), vec![0, 1]);
    }

    #[test]
    fn the_minute_stamp_changes_only_with_the_minute() {
        assert_eq!(
            minute_stamp(at("2026-08-24T07:30:00")),
            minute_stamp(at("2026-08-24T07:30:59"))
        );
        assert_ne!(
            minute_stamp(at("2026-08-24T07:30:00")),
            minute_stamp(at("2026-08-24T07:31:00"))
        );
    }

    #[test]
    fn an_empty_unit_list_targets_everything() {
        let all = vec!["1".to_string(), "2".to_string()];
        let mut p = Program {
            name: "x".into(),
            enabled: true,
            unit_ids: vec![],
            kind: "curve".into(),
            favourite: None,
            schedule: vec![],
            curve: None,
            id: "p1".into(),
        };
        assert_eq!(p.targets(&all), all);
        p.unit_ids = vec!["2".to_string()];
        assert_eq!(p.targets(&all), vec!["2".to_string()]);
    }

    fn spec(json: &str) -> Result<Program, ProgramError> {
        let spec: ProgramSpec = serde_json::from_str(json).expect("spec should parse");
        spec.into_program("id1")
    }

    #[test]
    fn a_spec_needs_only_a_name() {
        let p = spec(r#"{"name":"Ljeto"}"#).unwrap();
        assert_eq!(p.name, "Ljeto");
        assert!(p.enabled, "programs are enabled unless said otherwise");
        assert_eq!(p.kind, "favourite");
        assert!(p.unit_ids.is_empty());
        assert_eq!(p.id, "id1");
    }

    #[test]
    fn a_nameless_program_is_refused() {
        assert_eq!(spec(r#"{"name":""}"#), Err(ProgramError::EmptyName));
        // And a missing name is a parse failure, not a program called "".
        let spec: Result<ProgramSpec, _> = serde_json::from_str(r#"{"kind":"curve"}"#);
        assert!(spec.is_err());
    }

    #[test]
    fn the_name_limit_counts_characters_not_bytes() {
        // Pydantic's max_length counts characters, so a 64-character name of
        // multi-byte characters is legal and must stay legal.
        let name = "č".repeat(64);
        assert!(spec(&format!(r#"{{"name":"{name}"}}"#)).is_ok());
        let name = "č".repeat(65);
        assert_eq!(
            spec(&format!(r#"{{"name":"{name}"}}"#)),
            Err(ProgramError::NameTooLong(65))
        );
    }

    #[test]
    fn an_unknown_kind_is_refused() {
        assert_eq!(
            spec(r#"{"name":"x","kind":"cronjob"}"#),
            Err(ProgramError::UnknownKind("cronjob".into()))
        );
        for kind in PROGRAM_KINDS {
            assert!(spec(&format!(r#"{{"name":"x","kind":"{kind}"}}"#)).is_ok());
        }
    }

    #[test]
    fn schedule_days_are_sorted_and_deduped_on_the_way_in() {
        // `sorted(set(v))` in the reference. This changes the bytes written, so
        // it is not cosmetic: the same request must produce the same file.
        let p = spec(
            r#"{"name":"x","kind":"schedule","schedule":[
                 {"days":[4,0,4,2],"time":"07:30","settings":{}}]}"#,
        )
        .unwrap();
        assert_eq!(p.schedule[0].days, vec![0, 2, 4]);
    }

    #[test]
    fn a_day_outside_the_week_is_refused() {
        let bad = r#"{"name":"x","kind":"schedule","schedule":[
                      {"days":[7],"time":"07:30","settings":{}}]}"#;
        assert_eq!(spec(bad), Err(ProgramError::BadDay(7)));
    }

    #[test]
    fn ingest_demands_a_strict_two_digit_time() {
        // The reference regex is ^([01]\d|2[0-3]):([0-5]\d)$ -- "7:30" is not it.
        assert!(valid_hhmm("00:00"));
        assert!(valid_hhmm("23:59"));
        assert!(!valid_hhmm("7:30"));
        assert!(!valid_hhmm("24:00"));
        assert!(!valid_hhmm("07:60"));
        assert!(!valid_hhmm("07:3"));
        assert!(!valid_hhmm("07-30"));
        assert!(!valid_hhmm(""));

        let bad = r#"{"name":"x","kind":"schedule","schedule":[
                      {"days":[],"time":"7:30","settings":{}}]}"#;
        assert_eq!(spec(bad), Err(ProgramError::BadTime("7:30".into())));
    }

    #[test]
    fn a_lenient_read_and_a_strict_write_coexist() {
        // A file on disk is read as leniently as possible -- refusing to start
        // because of one odd string is worse than interpreting it -- while the
        // same string is rejected at the API boundary.
        assert_eq!(minutes_of_day("7:30"), Some(450));
        assert!(!valid_hhmm("7:30"));
    }

    #[test]
    fn a_curves_mode_is_upper_cased_on_the_way_in() {
        let p = spec(
            r#"{"name":"x","kind":"curve","curve":{"operational_mode":"heat",
                 "fan_speed":60,"points":[]}}"#,
        )
        .unwrap();
        assert_eq!(p.curve.unwrap().operational_mode, "HEAT");
    }

    #[test]
    fn a_curve_defaults_to_cool_on_auto_fan() {
        let p = spec(r#"{"name":"x","kind":"curve","curve":{}}"#).unwrap();
        let curve = p.curve.unwrap();
        assert_eq!(curve.operational_mode, "COOL");
        assert_eq!(curve.fan_speed, 102);
        assert!(curve.points.is_empty());
    }

    #[test]
    fn a_curve_with_nonsense_settings_is_refused() {
        assert_eq!(
            spec(r#"{"name":"x","kind":"curve","curve":{"operational_mode":"BOOST"}}"#),
            Err(ProgramError::UnknownMode("BOOST".into()))
        );
        assert_eq!(
            spec(r#"{"name":"x","kind":"curve","curve":{"fan_speed":55}}"#),
            Err(ProgramError::UnknownFanSpeed(55))
        );
    }

    #[test]
    fn curve_temperatures_are_bounded_like_the_reference() {
        // The reference bounds these on the *model*, so an out-of-range point can
        // never reach the file -- round_half's clamp is a second line of defence,
        // not the only one.
        let ok = r#"{"name":"x","kind":"curve","curve":{"points":[
                    {"time":"00:00","temperature":16.0},
                    {"time":"12:00","temperature":30.0}]}}"#;
        assert!(spec(ok).is_ok());
        let bad = r#"{"name":"x","kind":"curve","curve":{"points":[
                     {"time":"00:00","temperature":31.0}]}}"#;
        assert_eq!(spec(bad), Err(ProgramError::TemperatureOutOfRange(31.0)));
    }

    #[test]
    fn scene_settings_are_bounds_checked_too() {
        let bad = r#"{"name":"x","favourite":{"target_temperature":24.3}}"#;
        assert_eq!(
            spec(bad),
            Err(ProgramError::Settings(
                ValidationError::TemperatureNotHalfDegree
            ))
        );
        let bad = r#"{"name":"x","kind":"schedule","schedule":[
                     {"days":[],"time":"07:30","settings":{"fan_speed":55}}]}"#;
        assert_eq!(
            spec(bad),
            Err(ProgramError::Settings(ValidationError::UnknownFanSpeed(55)))
        );
    }

    #[test]
    fn an_updated_program_keeps_its_place_in_the_file() {
        // Reordering on update would make a diff against the pre-migration copy
        // unreadable, which is the one thing a migration needs to stay checkable.
        let mut doc = ProgramsDoc {
            programs: vec![
                spec(r#"{"name":"first"}"#).unwrap(),
                Program {
                    id: "id2".into(),
                    ..spec(r#"{"name":"second"}"#).unwrap()
                },
            ],
        };
        let updated = Program {
            id: "id1".into(),
            ..spec(r#"{"name":"renamed"}"#).unwrap()
        };
        assert!(doc.replace("id1", updated));
        assert_eq!(doc.programs[0].name, "renamed");
        assert_eq!(doc.programs[1].name, "second");
        assert_eq!(doc.programs.len(), 2);
    }

    #[test]
    fn getting_and_removing_report_what_they_found() {
        let mut doc = ProgramsDoc {
            programs: vec![spec(r#"{"name":"only"}"#).unwrap()],
        };
        assert_eq!(doc.get("id1").unwrap().name, "only");
        assert!(doc.get("nope").is_none());
        assert!(!doc.replace("nope", spec(r#"{"name":"x"}"#).unwrap()));
        assert!(doc.remove("id1"));
        assert!(!doc.remove("id1"));
    }
}
