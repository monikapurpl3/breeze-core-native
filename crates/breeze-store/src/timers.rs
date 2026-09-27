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
//!
//! A **scheduled start** ("on in three days at 07:00", 4.2.0) is the one timer
//! that names a wall-clock time, and it is still the server's clock that
//! decides: the client sends a number of days and an `HH:MM`, and the server
//! adds the days to *its* date, so "07:00" means seven o'clock at home whatever
//! the phone thinks the time is.

use chrono::{Local, NaiveDateTime, NaiveTime, TimeDelta};

use crate::control::ControlRequest;
use crate::models::{Timer, TimerKind, TimersDoc};

/// A day is the ceiling on purpose: this is "I am going to sleep", not a
/// scheduling system. Anything longer is a program.
pub const MAX_MINUTES: u32 = 24 * 60;

/// How far ahead a scheduled start may be: a long holiday. Anything that
/// repeats, or reaches further, is a schedule in a program.
pub const MAX_START_DAYS: u32 = 30;

/// How many unit ids one timer may name.
pub const MAX_UNITS: usize = 64;

/// How late a scheduled start may still fire: enough to ride out a restart or
/// an upgrade, not so much that a unit comes on in an empty house hours after
/// the moment it was asked for because the server was down through it.
pub const START_GRACE_MINUTES: i64 = 15;

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
    BadDays(u32),
    BadTime(String),
    /// A start whose moment has already gone - today at a time already past.
    /// Refused rather than moved to tomorrow: guessing which day was meant is
    /// how a unit comes on a day early or late.
    InThePast(String),
}

impl core::fmt::Display for TimerError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadMinutes(m) => {
                write!(f, "minutes must be between 1 and {MAX_MINUTES}, got {m}")
            }
            Self::TooManyUnits(n) => write!(f, "at most {MAX_UNITS} units, got {n}"),
            Self::BadDays(d) => write!(f, "days must be between 0 and {MAX_START_DAYS}, got {d}"),
            Self::BadTime(t) => write!(f, "at must be a time as HH:MM, got {t:?}"),
            Self::InThePast(t) => write!(f, "{t} has already passed on the server's clock"),
        }
    }
}

/// `HH:MM`, 24-hour, as the clients send it. One-digit hours are accepted
/// because a person typing a time will send them.
pub fn parse_hhmm(text: &str) -> Option<NaiveTime> {
    let (h, m) = text.trim().split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    let hour: u32 = h.parse().ok()?;
    let minute: u32 = m.parse().ok()?;
    NaiveTime::from_hms_opt(hour, minute, 0)
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
        kind: TimerKind::Sleep,
    })
}

/// Build a scheduled start: `days` from today, at `at` on the server's clock.
///
/// It switches the unit on and nothing else. `minutes` is still filled in -
/// the whole minutes from now until it fires, rounded up - because it is a
/// required field every older client reads; for a start it is informational.
pub fn build_start_timer(
    id: impl Into<String>,
    unit_ids: Vec<String>,
    days: u32,
    at: &str,
    label: &str,
    now: NaiveDateTime,
) -> Result<Timer, TimerError> {
    if days > MAX_START_DAYS {
        return Err(TimerError::BadDays(days));
    }
    if unit_ids.len() > MAX_UNITS {
        return Err(TimerError::TooManyUnits(unit_ids.len()));
    }
    let time = parse_hhmm(at).ok_or_else(|| TimerError::BadTime(at.to_string()))?;
    let fires_at = (now.date() + TimeDelta::days(days as i64)).and_time(time);
    if fires_at <= now {
        // Said the way a person would read it back: "17:29 on 2026-09-27".
        return Err(TimerError::InThePast(
            fires_at.format("%H:%M on %Y-%m-%d").to_string(),
        ));
    }
    let seconds = (fires_at - now).num_seconds();
    Ok(Timer {
        id: id.into(),
        unit_ids,
        minutes: ((seconds + 59) / 60) as u32,
        created_at: format_local(now),
        fires_at: format_local(fires_at),
        settings: ControlRequest::power_on(),
        label: label.to_string(),
        kind: TimerKind::Start,
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

    /// A scheduled start whose moment passed more than
    /// [`START_GRACE_MINUTES`] ago, while the server was not running. It is
    /// dropped rather than fired: a schedule skips a minute it missed too, and
    /// "on at 07:30" happening at 14:00 is not what anybody asked for.
    ///
    /// A late sleep timer still fires, because "off, late" is the safe
    /// direction to be wrong in; "on, at some unknown time" is not, so a start
    /// whose time cannot be read counts as missed.
    pub fn missed(&self, now: NaiveDateTime) -> bool {
        self.kind == TimerKind::Start
            && parse_local(&self.fires_at)
                .is_none_or(|due| now - due > TimeDelta::minutes(START_GRACE_MINUTES))
    }

    /// Whether this timer covers `unit_id`. An empty list means every unit.
    pub fn covers(&self, unit_id: &str) -> bool {
        self.unit_ids.is_empty() || self.unit_ids.iter().any(|u| u == unit_id)
    }
}

impl TimersDoc {
    /// Remove any timer of this `kind` already promising something about
    /// these units.
    ///
    /// Asking for "off in 30" when one is pending means the user changed their
    /// mind, not that they want two competing promises about one unit. But
    /// only within a kind: a sleep timer and a scheduled start are different
    /// promises, and "off tonight, on on Monday" has to be possible. Returns
    /// how many were displaced.
    pub fn replace_for_units(&mut self, unit_ids: &[String], kind: TimerKind) -> usize {
        let before = self.timers.len();
        self.timers
            .retain(|t| t.kind != kind || !unit_ids.iter().any(|u| t.covers(u)));
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
    fn a_start_missed_by_more_than_the_grace_is_dropped_not_fired() {
        let now = at("2026-09-28T07:00:00");
        let start = build_start_timer("s1", vec!["1".into()], 1, "07:30", "", now).unwrap();
        // A restart across the moment: still fires.
        assert!(!start.missed(at("2026-09-29T07:30:00")));
        assert!(!start.missed(at("2026-09-29T07:45:00")));
        // The server was down through it: dropped.
        assert!(start.missed(at("2026-09-29T07:45:01")));
        assert!(start.missed(at("2026-10-01T09:00:00")));

        // A sleep timer is never "missed": switching off late is harmless.
        let sleep = build_timer("t1", vec!["1".into()], 30, None, "", now).unwrap();
        assert!(!sleep.missed(at("2026-10-01T09:00:00")));

        // A start with an unreadable time is not switched on at a guess.
        let broken = Timer {
            fires_at: "whenever".into(),
            ..start
        };
        assert!(broken.missed(now));
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
        assert_eq!(
            doc.replace_for_units(&["1".to_string()], TimerKind::Sleep),
            1
        );
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
        assert_eq!(
            doc.replace_for_units(&["1".to_string()], TimerKind::Sleep),
            1
        );
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

    // ------------------------------------------------------ scheduled starts

    #[test]
    fn a_start_fires_on_the_servers_date_plus_days_at_the_time() {
        let now = at("2026-09-27T12:00:00");
        let t = build_start_timer("s", vec!["1".into()], 3, "07:30", "", now).unwrap();
        assert_eq!(t.fires_at, "2026-09-30T07:30:00");
        assert_eq!(t.kind, TimerKind::Start);
        assert_eq!(
            t.settings,
            ControlRequest::power_on(),
            "it only switches on"
        );
        // Informational, but a whole number of minutes, rounded up.
        assert_eq!(t.minutes, 2 * 24 * 60 + 19 * 60 + 30);
    }

    #[test]
    fn a_start_later_today_is_days_zero() {
        let now = at("2026-09-27T12:00:00");
        let t = build_start_timer("s", vec![], 0, "18:05", "", now).unwrap();
        assert_eq!(t.fires_at, "2026-09-27T18:05:00");
    }

    #[test]
    fn a_start_already_past_today_is_refused_not_moved_to_tomorrow() {
        let now = at("2026-09-27T12:00:00");
        assert_eq!(
            build_start_timer("s", vec![], 0, "11:59", "", now),
            Err(TimerError::InThePast("11:59 on 2026-09-27".into()))
        );
        // The same minute counts as past: it would fire at once.
        assert!(build_start_timer("s", vec![], 0, "12:00", "", now).is_err());
        assert!(build_start_timer("s", vec![], 1, "11:59", "", now).is_ok());
    }

    #[test]
    fn a_start_reaches_thirty_days_and_no_further() {
        let now = at("2026-09-27T12:00:00");
        let t = build_start_timer("s", vec![], MAX_START_DAYS, "06:00", "", now).unwrap();
        assert_eq!(
            t.fires_at, "2026-10-27T06:00:00",
            "across the month boundary"
        );
        assert_eq!(
            build_start_timer("s", vec![], MAX_START_DAYS + 1, "06:00", "", now),
            Err(TimerError::BadDays(MAX_START_DAYS + 1))
        );
    }

    #[test]
    fn a_start_time_is_hh_mm() {
        for good in ["7:05", "07:05", "00:00", "23:59", " 07:05 "] {
            assert!(parse_hhmm(good).is_some(), "{good:?}");
        }
        for bad in [
            "24:00", "07:60", "7", "07:5", "07-05", "007:05", "", "ab:cd", "07:05:00",
        ] {
            assert!(parse_hhmm(bad).is_none(), "{bad:?}");
        }
        let now = at("2026-09-27T12:00:00");
        assert_eq!(
            build_start_timer("s", vec![], 1, "25:00", "", now),
            Err(TimerError::BadTime("25:00".into()))
        );
    }

    #[test]
    fn a_unit_keeps_one_timer_of_each_kind() {
        let now = at("2026-09-27T21:00:00");
        let mut doc = TimersDoc {
            timers: vec![
                build_timer("sleep", vec!["1".into()], 30, None, "", now).unwrap(),
                build_start_timer("start", vec!["1".into()], 2, "07:00", "", now).unwrap(),
            ],
        };
        // A new start replaces the old start and leaves the sleep timer alone...
        assert_eq!(
            doc.replace_for_units(&["1".to_string()], TimerKind::Start),
            1
        );
        assert_eq!(doc.timers.len(), 1);
        assert_eq!(doc.timers[0].id, "sleep");
        // ...and a new sleep timer replaces only the sleep timer.
        doc.timers
            .push(build_start_timer("start2", vec!["1".into()], 3, "07:00", "", now).unwrap());
        assert_eq!(
            doc.replace_for_units(&["1".to_string()], TimerKind::Sleep),
            1
        );
        assert_eq!(doc.timers.len(), 1);
        assert_eq!(doc.timers[0].id, "start2");
    }

    #[test]
    fn a_sleep_timer_is_stored_exactly_as_before() {
        // No `kind` key: timers.json files written before 4.2.0 read and write
        // back byte for byte.
        let now = at("2026-09-27T21:00:00");
        let sleep = build_timer("a", vec!["1".into()], 30, None, "", now).unwrap();
        let stored = crate::to_json(&sleep).unwrap();
        assert!(!stored.contains("kind"), "{stored}");
        let old: Timer = serde_json::from_str(&stored).unwrap();
        assert_eq!(
            old.kind,
            TimerKind::Sleep,
            "an old file's timers are sleep timers"
        );
        // A start says what it is.
        let start = build_start_timer("b", vec![], 1, "07:00", "", now).unwrap();
        assert!(crate::to_json(&start)
            .unwrap()
            .contains(r#""kind": "start""#));
    }
}
