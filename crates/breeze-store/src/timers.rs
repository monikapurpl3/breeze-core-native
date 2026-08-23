//! Operations on one-shot timers.
//!
//! The design decision worth knowing, inherited deliberately: a client asks for
//! **minutes**, never a wall-clock time, and the server computes the moment from
//! its own clock. The alternative puts the burden of knowing the server's
//! timezone on every client — and the phone may be in another zone, or its clock
//! adrift, which this project has already been bitten by.
//!
//! Stored times are **naive server-local** ISO strings, the same clock the
//! scheduler uses. That is also DST-naive: adding 45 minutes adds 45 minutes of
//! wall clock, exactly as `datetime.now() + timedelta(minutes=45)` does. That is
//! the behaviour to match rather than improve on, because the stored strings and
//! the scheduler have to agree with each other.

use chrono::{Local, NaiveDateTime, TimeDelta};

use crate::control::ControlRequest;
use crate::models::{Timer, TimersDoc};

/// A day is the ceiling on purpose: this is "I am going to sleep", not a
/// scheduling system. Anything longer is a program.
pub const MAX_MINUTES: u32 = 24 * 60;

/// How many unit ids one timer may name.
pub const MAX_UNITS: usize = 64;

/// The format Breeze Core writes: seconds precision, no zone, no fraction.
const ISO_SECONDS: &str = "%Y-%m-%dT%H:%M:%S";

/// Server-local wall clock, which is what everything scheduled here uses.
pub fn now_local() -> NaiveDateTime {
    Local::now().naive_local()
}

pub fn format_local(t: NaiveDateTime) -> String {
    t.format(ISO_SECONDS).to_string()
}

pub fn parse_local(text: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(text, ISO_SECONDS).ok()
}

#[derive(Debug, PartialEq, Eq)]
pub enum TimerError {
    BadMinutes(u32),
    TooManyUnits(usize),
}

impl core::fmt::Display for TimerError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadMinutes(m) => {
                write!(f, "minutes must be between 1 and {MAX_MINUTES}, got {m}")
            }
            Self::TooManyUnits(n) => write!(f, "at most {MAX_UNITS} units, got {n}"),
        }
    }
}

impl std::error::Error for TimerError {}

/// Build a timer, computing its moment from `now`.
pub fn build_timer(
    id: impl Into<String>,
    unit_ids: Vec<String>,
    minutes: u32,
    settings: Option<ControlRequest>,
    label: &str,
    now: NaiveDateTime,
) -> Result<Timer, TimerError> {
    if minutes == 0 || minutes > MAX_MINUTES {
        return Err(TimerError::BadMinutes(minutes));
    }
    if unit_ids.len() > MAX_UNITS {
        return Err(TimerError::TooManyUnits(unit_ids.len()));
    }
    let fires_at = now + TimeDelta::minutes(minutes as i64);
    Ok(Timer {
        id: id.into(),
        unit_ids,
        minutes,
        created_at: format_local(now),
        fires_at: format_local(fires_at),
        // Defaults to switching the unit off, which is what a sleep timer is
        // for -- but the field exists, so "switch to eco in an hour" needs no
        // second feature.
        settings: settings.unwrap_or_else(ControlRequest::power_off),
        label: label.to_string(),
    })
}

impl Timer {
    /// Whole seconds until this fires, floored at zero.
    ///
    /// Computed server-side and handed to clients precisely so a phone with a
    /// drifting clock still counts down correctly.
    pub fn seconds_remaining(&self, now: NaiveDateTime) -> i64 {
        match parse_local(&self.fires_at) {
            // An unparseable time counts as due rather than never: "off, late"
            // is the safe direction to be wrong in.
            None => 0,
            Some(due) => (due - now).num_seconds().max(0),
        }
    }

    pub fn is_due(&self, now: NaiveDateTime) -> bool {
        self.seconds_remaining(now) == 0
    }

    /// Whether this timer covers `unit_id`. An empty list means every unit.
    pub fn covers(&self, unit_id: &str) -> bool {
        self.unit_ids.is_empty() || self.unit_ids.iter().any(|u| u == unit_id)
    }
}

impl TimersDoc {
    /// Remove any timer already promising something about these units.
    ///
    /// Asking for "off in 30" when one is pending means the user changed their
    /// mind, not that they want two competing promises about one unit. Returns
    /// how many were displaced.
    pub fn replace_for_units(&mut self, unit_ids: &[String]) -> usize {
        let before = self.timers.len();
        self.timers
            .retain(|t| !unit_ids.iter().any(|u| t.covers(u)));
        before - self.timers.len()
    }

    pub fn remove(&mut self, timer_id: &str) -> bool {
        let before = self.timers.len();
        self.timers.retain(|t| t.id != timer_id);
        self.timers.len() != before
    }

    /// Timers that have come due, in creation order.
    pub fn due(&self, now: NaiveDateTime) -> Vec<Timer> {
        self.timers
            .iter()
            .filter(|t| t.is_due(now))
            .cloned()
            .collect()
    }

    /// The soonest `fires_at`, for diagnostics.
    pub fn next_fires_at(&self) -> Option<String> {
        self.timers.iter().map(|t| t.fires_at.clone()).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> NaiveDateTime {
        parse_local(text).expect("test timestamp")
    }

    #[test]
    fn a_timer_fires_the_requested_number_of_minutes_later() {
        let t = build_timer(
            "t1",
            vec!["1".into()],
            45,
            None,
            "bedtime",
            at("2026-08-23T21:00:00"),
        )
        .unwrap();
        assert_eq!(t.created_at, "2026-08-23T21:00:00");
        assert_eq!(t.fires_at, "2026-08-23T21:45:00");
        assert_eq!(t.label, "bedtime");
    }

    #[test]
    fn the_default_action_is_to_switch_off() {
        let t = build_timer("t1", vec![], 30, None, "", at("2026-08-23T21:00:00")).unwrap();
        assert_eq!(t.settings.power_state, Some(false));
        assert_eq!(
            t.settings.target_temperature, None,
            "nothing else is touched"
        );
    }

    #[test]
    fn a_supplied_action_is_kept_verbatim() {
        let eco = ControlRequest {
            eco: Some(true),
            ..Default::default()
        };
        let t = build_timer("t1", vec![], 60, Some(eco), "", at("2026-08-23T21:00:00")).unwrap();
        assert_eq!(t.settings.eco, Some(true));
        assert_eq!(
            t.settings.power_state, None,
            "must not add an unasked-for power-off"
        );
    }

    #[test]
    fn minutes_are_bounded_at_a_day() {
        let now = at("2026-08-23T21:00:00");
        assert!(build_timer("t", vec![], 1, None, "", now).is_ok());
        assert!(build_timer("t", vec![], MAX_MINUTES, None, "", now).is_ok());
        assert_eq!(
            build_timer("t", vec![], 0, None, "", now),
            Err(TimerError::BadMinutes(0))
        );
        assert_eq!(
            build_timer("t", vec![], MAX_MINUTES + 1, None, "", now),
            Err(TimerError::BadMinutes(MAX_MINUTES + 1))
        );
    }

    #[test]
    fn too_many_units_is_refused() {
        let many: Vec<String> = (0..MAX_UNITS + 1).map(|i| i.to_string()).collect();
        assert_eq!(
            build_timer("t", many, 30, None, "", at("2026-08-23T21:00:00")),
            Err(TimerError::TooManyUnits(MAX_UNITS + 1))
        );
    }

    #[test]
    fn the_countdown_comes_from_the_servers_clock() {
        let t = build_timer("t", vec![], 45, None, "", at("2026-08-23T21:00:00")).unwrap();
        assert_eq!(t.seconds_remaining(at("2026-08-23T21:00:00")), 2700);
        assert_eq!(t.seconds_remaining(at("2026-08-23T21:30:00")), 900);
        assert_eq!(t.seconds_remaining(at("2026-08-23T21:45:00")), 0);
    }

    #[test]
    fn an_overdue_timer_is_due_rather_than_negative() {
        // If the server was asleep when it came due, "off, late" beats "never".
        let t = build_timer("t", vec![], 45, None, "", at("2026-08-23T21:00:00")).unwrap();
        assert_eq!(t.seconds_remaining(at("2026-08-24T09:00:00")), 0);
        assert!(t.is_due(at("2026-08-24T09:00:00")));
    }

    #[test]
    fn an_unparseable_time_counts_as_due() {
        let mut t = build_timer("t", vec![], 45, None, "", at("2026-08-23T21:00:00")).unwrap();
        t.fires_at = "not a time".into();
        assert!(
            t.is_due(at("2026-08-23T21:00:00")),
            "the safe direction is to fire"
        );
    }

    #[test]
    fn an_empty_unit_list_covers_everything() {
        let now = at("2026-08-23T21:00:00");
        let all = build_timer("t", vec![], 30, None, "", now).unwrap();
        assert!(all.covers("anything"));
        let one = build_timer("t", vec!["7".into()], 30, None, "", now).unwrap();
        assert!(one.covers("7"));
        assert!(!one.covers("8"));
    }

    #[test]
    fn a_new_timer_displaces_the_units_existing_one() {
        let now = at("2026-08-23T21:00:00");
        let mut doc = TimersDoc {
            timers: vec![
                build_timer("a", vec!["1".into()], 30, None, "", now).unwrap(),
                build_timer("b", vec!["2".into()], 30, None, "", now).unwrap(),
            ],
        };
        assert_eq!(doc.replace_for_units(&["1".to_string()]), 1);
        assert_eq!(doc.timers.len(), 1);
        assert_eq!(doc.timers[0].id, "b", "the other unit's timer must survive");
    }

    #[test]
    fn an_all_units_timer_is_displaced_by_any_unit() {
        let now = at("2026-08-23T21:00:00");
        let mut doc = TimersDoc {
            timers: vec![build_timer("all", vec![], 30, None, "", now).unwrap()],
        };
        // It promises something about unit 1, so a timer on unit 1 replaces it --
        // otherwise two promises would fight over the same air conditioner.
        assert_eq!(doc.replace_for_units(&["1".to_string()]), 1);
        assert!(doc.timers.is_empty());
    }

    #[test]
    fn due_finds_only_what_has_come_due() {
        let now = at("2026-08-23T21:00:00");
        let doc = TimersDoc {
            timers: vec![
                build_timer("soon", vec![], 1, None, "", now).unwrap(),
                build_timer("later", vec![], 600, None, "", now).unwrap(),
            ],
        };
        assert!(doc.due(now).is_empty());
        let due = doc.due(at("2026-08-23T21:05:00"));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, "soon");
    }

    #[test]
    fn next_fires_at_is_the_soonest() {
        let now = at("2026-08-23T21:00:00");
        let doc = TimersDoc {
            timers: vec![
                build_timer("late", vec![], 600, None, "", now).unwrap(),
                build_timer("soon", vec![], 10, None, "", now).unwrap(),
            ],
        };
        // ISO strings sort lexicographically, which is why this format is used.
        assert_eq!(doc.next_fires_at().as_deref(), Some("2026-08-23T21:10:00"));
        assert!(TimersDoc::default().next_fires_at().is_none());
    }

    #[test]
    fn removing_reports_whether_anything_went() {
        let now = at("2026-08-23T21:00:00");
        let mut doc = TimersDoc {
            timers: vec![build_timer("a", vec![], 30, None, "", now).unwrap()],
        };
        assert!(doc.remove("a"));
        assert!(!doc.remove("a"));
    }

    #[test]
    fn adding_minutes_is_wall_clock_not_calendar_aware() {
        // Matching Python's naive datetime arithmetic exactly. Across a DST
        // boundary this adds wall-clock minutes, which is what the scheduler
        // does too -- the two must agree, so this is behaviour to keep rather
        // than to improve.
        let t = build_timer("t", vec![], 120, None, "", at("2026-10-25T01:30:00")).unwrap();
        assert_eq!(t.fires_at, "2026-10-25T03:30:00");
    }
}
