//! `/api/programs` — favourites, schedules and curves, and the thread that runs
//! them.
//!
//! Requires the same authentication as controlling a unit: a program *is* a
//! control command, stored and possibly repeating. The scheduler itself runs
//! server-side, so a phone can go flat without a schedule being missed — the
//! whole reason schedules do not live in the app.
//!
//! Three kinds, one store:
//!
//! * **favourite** — a named scene, applied only when asked. Never auto-fires.
//! * **schedule** — day/time triggers, each applying a scene when the clock
//!   reaches its minute.
//! * **curve** — a daily time→temperature curve the scheduler tracks
//!   continuously.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use breeze_store::{Program, ProgramSpec, ProgramsDoc};

use crate::respond::Reply;
use crate::state::AppState;

/// The reference clamps its tick this way, and a five-second floor is already
/// far finer than a schedule needs: a trigger only has to land inside the right
/// minute.
pub const MIN_TICK_SECONDS: u64 = 5;

/// The tick actually used, whatever `AC_SCHED_TICK` says.
///
/// Clamped rather than rejected: a misconfigured value should slow the loop
/// down, not stop schedules firing.
pub fn tick_seconds(configured: u64) -> u64 {
    configured.max(MIN_TICK_SECONDS)
}

/// The scheduler's own bookkeeping.
#[derive(Debug, Default)]
pub struct SchedulerState {
    pub running: bool,
    pub runs: u64,
    /// Tick-level failures only — a lock or a store that could not be read.
    ///
    /// Deliberately *not* incremented for a unit that would not answer, because
    /// the reference does not either: those are logged and normal (an air
    /// conditioner is unplugged, or the household router rebooted). Counting
    /// them here would make a healthy overnight run look like a broken one to
    /// anyone comparing the two servers' diagnostics.
    pub errors: u64,
    pub last_run: Option<String>,
    /// `(program id, entry index) -> "YYYY-MM-DD HH:MM"`, so a trigger fires
    /// once per minute however often the loop wakes up.
    fired: HashMap<(String, usize), String>,
    /// `(program id, unit id) -> the setpoint last applied`.
    ///
    /// Two reasons, both load-bearing: a curve moves in half-degree steps, so
    /// re-applying every tick would be a device round-trip a minute for nothing;
    /// and someone who nudges the temperature by hand keeps their nudge until
    /// the curve genuinely moves on.
    curve_applied: HashMap<(String, String), f64>,
}

/// `GET /api/programs`
pub fn list(state: &AppState) -> Reply {
    match state.programs.read() {
        Ok(p) => Reply::json_body(200, &p.programs),
        Err(_) => Reply::detail(500, "program store unavailable"),
    }
}

/// `GET /api/programs/{id}`
pub fn get(state: &AppState, program_id: &str) -> Reply {
    let programs = match state.programs.read() {
        Ok(p) => p,
        Err(_) => return Reply::detail(500, "program store unavailable"),
    };
    match programs.get(program_id) {
        Some(p) => Reply::json_body(200, p),
        None => Reply::detail(404, format!("no program '{program_id}'")),
    }
}

/// `POST /api/programs` — 201 with the stored program, id included.
pub fn create(state: &AppState, body: &[u8]) -> Reply {
    let spec: ProgramSpec = match serde_json::from_slice(body) {
        Ok(s) => s,
        Err(e) => return Reply::detail(422, format!("invalid program: {e}")),
    };
    let id = match random_id() {
        Some(id) => id,
        None => return Reply::detail(500, "no entropy available"),
    };
    let program = match spec.into_program(id) {
        Ok(p) => p,
        Err(e) => return Reply::detail(422, e.to_string()),
    };

    let mut programs = match state.programs.write() {
        Ok(p) => p,
        Err(_) => return Reply::detail(500, "program store unavailable"),
    };
    programs.programs.push(program.clone());
    if let Err(e) = persist(state, &programs) {
        // Roll back, or the API reports a program that vanishes on restart.
        programs.programs.pop();
        return Reply::detail(500, e);
    }
    Reply::json_body(201, &program)
}

/// `PUT /api/programs/{id}` — replaces everything but the id.
pub fn update(state: &AppState, program_id: &str, body: &[u8]) -> Reply {
    let spec: ProgramSpec = match serde_json::from_slice(body) {
        Ok(s) => s,
        Err(e) => return Reply::detail(422, format!("invalid program: {e}")),
    };
    let program = match spec.into_program(program_id) {
        Ok(p) => p,
        Err(e) => return Reply::detail(422, e.to_string()),
    };

    let mut programs = match state.programs.write() {
        Ok(p) => p,
        Err(_) => return Reply::detail(500, "program store unavailable"),
    };
    let Some(previous) = programs.get(program_id).cloned() else {
        return Reply::detail(404, format!("no program '{program_id}'"));
    };
    programs.replace(program_id, program.clone());
    if let Err(e) = persist(state, &programs) {
        programs.replace(program_id, previous);
        return Reply::detail(500, e);
    }
    Reply::json_body(200, &program)
}

/// `DELETE /api/programs/{id}` — 204.
pub fn delete(state: &AppState, program_id: &str) -> Reply {
    let mut programs = match state.programs.write() {
        Ok(p) => p,
        Err(_) => return Reply::detail(500, "program store unavailable"),
    };
    let Some(previous) = programs.get(program_id).cloned() else {
        return Reply::detail(404, format!("no program '{program_id}'"));
    };
    programs.remove(program_id);
    if let Err(e) = persist(state, &programs) {
        programs.programs.push(previous);
        return Reply::detail(500, e);
    }
    Reply {
        status: 204,
        body: Vec::new(),
        content_type: "application/json",
        extra: Vec::new(),
    }
}

/// `POST /api/programs/{id}/apply` — run it now.
///
/// A favourite applies its scene; a curve applies wherever it currently sits. A
/// schedule is refused: it has no single scene to apply, only triggers that fire
/// on their own.
pub fn apply_now(state: &AppState, program_id: &str) -> Reply {
    let program = {
        let programs = match state.programs.read() {
            Ok(p) => p,
            Err(_) => return Reply::detail(500, "program store unavailable"),
        };
        match programs.get(program_id) {
            Some(p) => p.clone(),
            None => return Reply::detail(404, format!("no program '{program_id}'")),
        }
    };

    let request = if program.is_curve() {
        match &program.curve {
            Some(curve) => breeze_store::curve_request(curve, breeze_store::now_local()),
            None => return Reply::detail(400, "curve has no points"),
        }
    } else if program.is_schedule() {
        return Reply::detail(
            400,
            "schedule programs fire automatically; nothing to apply",
        );
    } else {
        match &program.favourite {
            Some(scene) => scene.clone(),
            None => return Reply::detail(400, "favourite has no settings"),
        }
    };

    // One result per unit, in the program's order. A unit that will not answer
    // fails the request, matching the reference: this is a button press, and
    // silently doing half of what was asked is worse than saying so.
    let mut results = Vec::new();
    for unit in targets(state, &program) {
        match crate::control::apply(&state.manager, unit, &request) {
            Ok(value) => results.push(value),
            Err(e) => return Reply::detail(503, e),
        }
    }
    Reply::json_body(200, &results)
}

/// `GET /api/programs/status`
pub fn status(state: &AppState) -> Reply {
    let (running, runs, errors, last_run) = match state.scheduler.lock() {
        Ok(s) => (s.running, s.runs, s.errors, s.last_run.clone()),
        Err(_) => (false, 0, 0, None),
    };
    // Exactly the reference's five keys, in its order. A client's diagnostics
    // screen reads these by name.
    Reply::json(
        200,
        &serde_json::json!({
            "running": running,
            "tick_seconds": tick_seconds(state.settings.sched_tick_seconds),
            "runs": runs,
            "errors": errors,
            "last_run": last_run,
        }),
    )
}

/// Units a program targets: its own list, or every configured unit.
fn targets(state: &AppState, program: &Program) -> Vec<u64> {
    if program.unit_ids.is_empty() {
        state.manager.known_units()
    } else {
        program
            .unit_ids
            .iter()
            .filter_map(|s| s.parse().ok())
            .collect()
    }
}

fn persist(state: &AppState, programs: &ProgramsDoc) -> Result<(), String> {
    breeze_store::save(
        &state.settings.programs_path,
        programs,
        breeze_store::Mode::Private,
    )
    .map_err(|e| format!("cannot write the program store: {e}"))
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

/// Start the background thread that fires schedules and drives curves.
pub fn spawn_scheduler(state: Arc<AppState>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        if let Ok(mut s) = state.scheduler.lock() {
            s.running = true;
        }
        let tick = Duration::from_secs(tick_seconds(state.settings.sched_tick_seconds));
        loop {
            std::thread::sleep(tick);
            run_once(&state, breeze_store::now_local());
        }
    })
}

/// One pass over every enabled program.
///
/// Takes `now` so the whole pass shares one clock reading — otherwise a tick
/// straddling a minute boundary could fire a trigger under one stamp and record
/// it under the next, and so fire it twice.
pub fn run_once(state: &AppState, now: chrono::NaiveDateTime) {
    let programs: Vec<Program> = match state.programs.read() {
        Ok(p) => p.programs.clone(),
        Err(_) => {
            if let Ok(mut s) = state.scheduler.lock() {
                s.runs += 1;
                s.errors += 1;
            }
            return;
        }
    };

    for program in programs.iter().filter(|p| p.enabled) {
        if program.is_schedule() {
            run_schedule(state, program, now);
        } else if program.is_curve() {
            run_curve(state, program, now);
        }
    }

    if let Ok(mut s) = state.scheduler.lock() {
        s.runs += 1;
        s.last_run = Some(breeze_store::timers::format_local(now));
    }
}

fn run_schedule(state: &AppState, program: &Program, now: chrono::NaiveDateTime) {
    let due = breeze_store::due_entries(&program.schedule, now);
    if due.is_empty() {
        return;
    }
    let stamp = breeze_store::minute_stamp(now);

    for index in due {
        // Claimed before applying, and only once: the reference records the
        // firing up front, so a trigger that fails does not retry for the rest
        // of the minute. Retrying would mean a schedule firing several times
        // over while a unit is briefly unreachable.
        {
            let mut sched = match state.scheduler.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            let key = (program.id.clone(), index);
            if sched.fired.get(&key) == Some(&stamp) {
                continue;
            }
            sched.fired.insert(key, stamp.clone());
        }

        let settings = &program.schedule[index].settings;
        eprintln!(
            "schedule '{}' firing ({})",
            program.name, program.schedule[index].time
        );
        for unit in targets(state, program) {
            if let Err(e) = crate::control::apply(&state.manager, unit, settings) {
                eprintln!("schedule '{}' -> unit {unit} failed: {e}", program.name);
            }
        }
    }
}

fn run_curve(state: &AppState, program: &Program, now: chrono::NaiveDateTime) {
    let Some(curve) = &program.curve else {
        return;
    };
    let request = breeze_store::curve_request(curve, now);
    // No points means nothing to track. (The apply route deliberately differs —
    // see `breeze_store::curve_request`.)
    let Some(temp) = request.target_temperature else {
        return;
    };

    for unit in targets(state, program) {
        let key = (program.id.clone(), unit.to_string());
        match state.scheduler.lock() {
            Ok(sched) => {
                if sched.curve_applied.get(&key) == Some(&temp) {
                    continue;
                }
            }
            Err(_) => return,
        }

        match crate::control::apply(&state.manager, unit, &request) {
            Ok(_) => {
                // Recorded only on success, so a unit that was unreachable is
                // retried next tick instead of being written off until the
                // curve happens to move.
                if let Ok(mut sched) = state.scheduler.lock() {
                    sched.curve_applied.insert(key, temp);
                }
                eprintln!("curve '{}' -> unit {unit} set {temp:.1}", program.name);
            }
            Err(e) => eprintln!("curve '{}' -> unit {unit} failed: {e}", program.name),
        }
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
    fn ids_are_twelve_hex_characters_like_the_reference() {
        // `secrets.token_hex(6)`.
        let id = random_id().unwrap();
        assert_eq!(id.len(), 12);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(id, random_id().unwrap());
    }

    #[test]
    fn a_trigger_is_claimed_once_per_minute() {
        // The dedup lives in SchedulerState; this checks the shape of the key,
        // which is what makes two entries at the same time independent.
        let mut sched = SchedulerState::default();
        let stamp = breeze_store::minute_stamp(at("2026-08-24T07:30:00"));
        sched.fired.insert(("p1".into(), 0), stamp.clone());

        assert_eq!(sched.fired.get(&("p1".to_string(), 0)), Some(&stamp));
        assert_eq!(
            sched.fired.get(&("p1".to_string(), 1)),
            None,
            "a second entry at the same minute must still fire"
        );
        assert_eq!(
            sched.fired.get(&("p2".to_string(), 0)),
            None,
            "another program's entry 0 is unrelated"
        );
    }

    #[test]
    fn the_stamp_changes_with_the_minute_so_a_trigger_can_fire_again() {
        let a = breeze_store::minute_stamp(at("2026-08-24T07:30:00"));
        let b = breeze_store::minute_stamp(at("2026-08-25T07:30:00"));
        assert_ne!(a, b, "tomorrow's 07:30 is a different firing");
    }

    #[test]
    fn a_curve_is_keyed_per_program_and_unit() {
        let mut sched = SchedulerState::default();
        sched.curve_applied.insert(("p1".into(), "7".into()), 22.0);
        assert_eq!(
            sched
                .curve_applied
                .get(&("p1".to_string(), "7".to_string())),
            Some(&22.0)
        );
        assert_eq!(
            sched
                .curve_applied
                .get(&("p1".to_string(), "8".to_string())),
            None,
            "each unit is tracked separately"
        );
    }

    #[test]
    fn the_tick_has_a_floor() {
        // A misconfigured tick slows the loop down; it must not stop it, and it
        // must not busy-spin either.
        assert_eq!(tick_seconds(30), 30);
        assert_eq!(tick_seconds(0), MIN_TICK_SECONDS);
        assert_eq!(tick_seconds(1), MIN_TICK_SECONDS);
        assert_eq!(tick_seconds(3600), 3600);
    }
}
