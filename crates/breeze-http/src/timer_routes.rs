//! `/api/timers` — one-shot deferred commands, and the thread that fires them.
//!
//! Requires the same authentication as controlling a unit, because a timer *is* a
//! control command, just a deferred one.
//!
//! Every response carries `seconds_remaining`, computed from the server's clock.
//! A client counts down from that rather than parsing `fires_at` against its own
//! clock — a phone's idea of the time is exactly what this design avoids relying
//! on.

use std::sync::Arc;
use std::time::Duration;

use breeze_store::{ControlRequest, Timer};
use serde::Deserialize;

use crate::respond::Reply;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct TimerRequest {
    #[serde(default)]
    pub unit_ids: Vec<String>,
    pub minutes: u32,
    #[serde(default)]
    pub settings: Option<ControlRequest>,
    #[serde(default)]
    pub label: String,
}

/// How the runner is doing, for diagnostics.
#[derive(Debug, Default, Clone)]
pub struct RunnerStats {
    pub running: bool,
    pub runs: u64,
    pub fired: u64,
    pub errors: u64,
    pub last_run: Option<String>,
}

/// Serialise a timer for the wire.
///
/// `settings` drops its nulls here, unlike the stored form which keeps them. A
/// `ControlRequest` is mostly nulls, and a client rendering "what happens when
/// this fires" should not have to filter them out — while the stored file must
/// keep them to stay byte-compatible. Two audiences, two shapes.
fn serialise(timer: &Timer, now: chrono::NaiveDateTime) -> serde_json::Value {
    let mut value = serde_json::to_value(timer).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(settings) = value.get_mut("settings").and_then(|s| s.as_object_mut()) {
        settings.retain(|_, v| !v.is_null());
    }
    value["seconds_remaining"] = serde_json::json!(timer.seconds_remaining(now));
    value
}

/// `GET /api/timers`
pub fn list(state: &AppState) -> Reply {
    let timers = match state.timers.read() {
        Ok(t) => t,
        Err(_) => return Reply::detail(500, "timer store unavailable"),
    };
    let now = breeze_store::now_local();
    let list: Vec<serde_json::Value> = timers.timers.iter().map(|t| serialise(t, now)).collect();
    Reply::json_body(200, &list)
}

/// `POST /api/timers`
pub fn create(state: &AppState, body: &[u8]) -> Reply {
    let request: TimerRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return Reply::detail(422, format!("invalid timer request: {e}")),
    };
    if let Some(settings) = &request.settings {
        if let Err(e) = settings.validate() {
            return Reply::detail(422, e.to_string());
        }
    }

    // Naming a unit that does not exist is a client bug worth reporting, not
    // something to silently drop -- a timer that quietly covers nothing is worse
    // than an error.
    let unknown: Vec<String> = request
        .unit_ids
        .iter()
        .filter(|id| {
            !id.parse::<u64>()
                .map(|n| state.manager.contains(n))
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    if !unknown.is_empty() {
        return Reply::detail(404, format!("unknown unit(s): {}", unknown.join(", ")));
    }

    let id = match random_id() {
        Some(id) => id,
        None => return Reply::detail(500, "no entropy available"),
    };
    let now = breeze_store::now_local();
    let timer = match breeze_store::build_timer(
        id,
        request.unit_ids.clone(),
        request.minutes,
        request.settings,
        &request.label,
        now,
    ) {
        Ok(t) => t,
        Err(e) => return Reply::detail(422, e.to_string()),
    };

    let mut timers = match state.timers.write() {
        Ok(t) => t,
        Err(_) => return Reply::detail(500, "timer store unavailable"),
    };
    timers.replace_for_units(&request.unit_ids);
    timers.timers.push(timer.clone());
    if let Err(e) = breeze_store::save(
        &state.settings.timers_path,
        &*timers,
        breeze_store::Mode::Private,
    ) {
        // Roll back, or the API would report a timer that vanishes on restart.
        timers.remove(&timer.id);
        return Reply::detail(500, format!("cannot write the timer store: {e}"));
    }
    Reply::json(201, &serialise(&timer, now))
}

/// `DELETE /api/timers/{id}`
pub fn cancel(state: &AppState, timer_id: &str) -> Reply {
    let mut timers = match state.timers.write() {
        Ok(t) => t,
        Err(_) => return Reply::detail(500, "timer store unavailable"),
    };
    if !timers.remove(timer_id) {
        return Reply::detail(404, "no such timer");
    }
    if let Err(e) = breeze_store::save(
        &state.settings.timers_path,
        &*timers,
        breeze_store::Mode::Private,
    ) {
        return Reply::detail(500, format!("cannot write the timer store: {e}"));
    }
    Reply {
        status: 204,
        body: Vec::new(),
        content_type: "application/json",
        extra: Vec::new(),
    }
}

/// `GET /api/timers/status`
pub fn status(state: &AppState) -> Reply {
    let stats = match state.runner.lock() {
        Ok(s) => s.clone(),
        Err(_) => RunnerStats::default(),
    };
    let (pending, next) = match state.timers.read() {
        Ok(t) => (t.timers.len(), t.next_fires_at()),
        Err(_) => (0, None),
    };
    Reply::json(
        200,
        &serde_json::json!({
            "running": stats.running,
            "tick_seconds": state.settings.timer_tick_seconds,
            "runs": stats.runs,
            "fired": stats.fired,
            "errors": stats.errors,
            "last_run": stats.last_run,
            "pending": pending,
            "next_fires_at": next,
        }),
    )
}

fn random_id() -> Option<String> {
    let mut buf = [0u8; 6];
    getrandom::getrandom(&mut buf).ok()?;
    let mut out = String::with_capacity(12);
    for b in buf {
        use core::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    Some(out)
}

/// Start the background thread that fires due timers.
///
/// Its own thread, separate from the scheduler, so a disk error while a timer
/// deletes itself cannot stop schedules and curves from running. The tick is
/// finer than the scheduler's because a schedule only has to hit the right
/// minute, while a timer is a promise about a moment.
pub fn spawn_runner(state: Arc<AppState>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        if let Ok(mut s) = state.runner.lock() {
            s.running = true;
        }
        let tick = Duration::from_secs(state.settings.timer_tick_seconds.max(1));
        loop {
            std::thread::sleep(tick);
            run_once(&state);
        }
    })
}

/// One pass: fire whatever is due, then forget it.
pub fn run_once(state: &AppState) {
    let now = breeze_store::now_local();
    let due: Vec<Timer> = match state.timers.read() {
        Ok(t) => t.due(now),
        Err(_) => return,
    };

    let mut fired = 0u64;
    let mut errors = 0u64;
    for timer in &due {
        let targets: Vec<u64> = if timer.unit_ids.is_empty() {
            state.manager.known_units()
        } else {
            timer
                .unit_ids
                .iter()
                .filter_map(|s| s.parse().ok())
                .collect()
        };
        for unit in targets {
            match crate::control::apply(&state.manager, unit, &timer.settings) {
                Ok(value) => {
                    fired += 1;
                    // "Dispatched", not "applied": a unit that reports itself
                    // offline may not have heard us, and this is unattended, so
                    // saying so is the honest thing.
                    if value.get("online").and_then(|v| v.as_bool()) == Some(false) {
                        eprintln!(
                            "timer {} dispatched to unit {unit}, but the unit is offline \
                             -- it may not have received it",
                            timer.id
                        );
                    }
                }
                Err(e) => {
                    errors += 1;
                    eprintln!("timer {} failed on unit {unit}: {e}", timer.id);
                }
            }
        }
    }

    if !due.is_empty() {
        // Delete in one write: a timer that fires must not fire twice, and
        // retrying every tick would eventually switch a unit off hours after
        // anybody asked.
        if let Ok(mut timers) = state.timers.write() {
            for timer in &due {
                timers.remove(&timer.id);
            }
            if let Err(e) = breeze_store::save(
                &state.settings.timers_path,
                &*timers,
                breeze_store::Mode::Private,
            ) {
                eprintln!("cannot write the timer store after firing: {e}");
                errors += 1;
            }
        }
    }

    if let Ok(mut stats) = state.runner.lock() {
        stats.runs += 1;
        stats.fired += fired;
        stats.errors += errors;
        stats.last_run = Some(breeze_store::timers::format_local(now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use breeze_store::timers::parse_local;

    fn at(text: &str) -> chrono::NaiveDateTime {
        parse_local(text).unwrap()
    }

    #[test]
    fn the_wire_form_drops_nulls_from_settings() {
        // The stored form keeps them for byte-compatibility; a client rendering
        // "what happens when this fires" should not have to filter them.
        let t = breeze_store::build_timer(
            "t1",
            vec!["1".into()],
            45,
            None,
            "bedtime",
            at("2026-08-23T21:00:00"),
        )
        .unwrap();
        let wire = serialise(&t, at("2026-08-23T21:00:00"));
        let settings = wire["settings"].as_object().unwrap();
        assert_eq!(
            settings.len(),
            1,
            "only the field that applies: {settings:?}"
        );
        assert_eq!(settings["power_state"], false);

        // But the stored form still carries every key.
        let stored = serde_json::to_value(&t).unwrap();
        assert_eq!(stored["settings"].as_object().unwrap().len(), 8);
    }

    #[test]
    fn every_response_carries_a_server_computed_countdown() {
        let t = breeze_store::build_timer("t", vec![], 45, None, "", at("2026-08-23T21:00:00"))
            .unwrap();
        let wire = serialise(&t, at("2026-08-23T21:30:00"));
        assert_eq!(wire["seconds_remaining"], 900);
        // fires_at is still there, but a client is meant to use the countdown.
        assert_eq!(wire["fires_at"], "2026-08-23T21:45:00");
    }

    #[test]
    fn a_timer_request_needs_only_minutes() {
        let r: TimerRequest = serde_json::from_slice(br#"{"minutes":45}"#).unwrap();
        assert_eq!(r.minutes, 45);
        assert!(r.unit_ids.is_empty(), "empty means every unit");
        assert!(r.settings.is_none());
        assert_eq!(r.label, "");
    }

    #[test]
    fn a_request_without_minutes_is_rejected() {
        // No default: "in zero minutes" and "you forgot to say when" are
        // different, and guessing would fire something immediately.
        assert!(serde_json::from_slice::<TimerRequest>(br#"{"unit_ids":["1"]}"#).is_err());
    }

    #[test]
    fn ids_are_hex_and_do_not_repeat() {
        let a = random_id().unwrap();
        let b = random_id().unwrap();
        assert_eq!(a.len(), 12);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
