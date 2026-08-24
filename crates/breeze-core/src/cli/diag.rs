//! `breeze-core diag` — the closest thing this project has to a test suite.
//!
//! Replaces `tools/ac-diag.zsh`, which existed because there was nothing else to
//! put it in. Being a subcommand of the binary means it works on the three BSDs,
//! on Windows and on a router with no zsh; it needs no `curl` or `jq`; and it
//! shares the credential handling with `control` rather than reading
//! `config.json` behind the server's back.
//!
//! Structured as checks with verdicts rather than a wall of output, because the
//! question being asked is always "is anything wrong", and the answer has to
//! survive being pasted into a bug report.

use crate::cli::client::Client;

/// How a single check came out.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Verdict {
    Ok,
    /// Working, but worth knowing about.
    Warn,
    /// Broken.
    Fail,
    /// Not applicable to this server, and not a problem.
    Skip,
}

impl Verdict {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "OK  ",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
            Self::Skip => "skip",
        }
    }
}

pub struct Report {
    checks: Vec<(Verdict, String)>,
}

impl Report {
    fn new() -> Self {
        Self { checks: Vec::new() }
    }

    fn add(&mut self, verdict: Verdict, message: impl Into<String>) {
        let message = message.into();
        println!("  {} {message}", verdict.label());
        self.checks.push((verdict, message));
    }

    fn section(&self, title: &str) {
        println!("\n== {title} ==");
    }

    fn count(&self, verdict: Verdict) -> usize {
        self.checks.iter().filter(|(v, _)| *v == verdict).count()
    }

    /// The exit code: non-zero only for a real failure. A warning is information,
    /// not a reason for a script to stop.
    pub fn exit_code(&self) -> i32 {
        if self.count(Verdict::Fail) > 0 {
            1
        } else {
            0
        }
    }

    fn summarise(&self) {
        println!("\n== summary ==");
        println!(
            "  {} ok, {} warning(s), {} failure(s)",
            self.count(Verdict::Ok),
            self.count(Verdict::Warn),
            self.count(Verdict::Fail)
        );
        println!(
            "  result: {}",
            match (self.count(Verdict::Fail), self.count(Verdict::Warn)) {
                (0, 0) => "OK",
                (0, _) => "OK, with warnings",
                _ => "PROBLEMS FOUND",
            }
        );
    }
}

/// Temperatures outside this are sensor nonsense rather than weather. The same
/// window the app uses to decide a reading is missing.
const SANE_MIN: f64 = -50.0;
const SANE_MAX: f64 = 80.0;

/// A round trip slower than this is worth mentioning: the units here answer in
/// about 700 ms, and a second and a half means a weak signal.
const SLOW_MS: u128 = 1_500;

pub fn run(base_url: Option<String>) -> Result<i32, String> {
    let mut profile = crate::cli::profile::ensure()?;
    if let Some(url) = base_url {
        profile.base_url = url.trim_end_matches('/').to_string();
    }
    let client = Client::from_profile(&profile);
    let mut report = Report::new();

    println!("breeze-core diag -> {}", client.base_url());

    // --- reachable, and what it is -----------------------------------------
    report.section("server");
    match client.get("/api/health") {
        Ok(body) if body["status"] == "ok" => {
            report.add(Verdict::Ok, "the server answers /api/health")
        }
        Ok(other) => report.add(Verdict::Fail, format!("health returned {other}")),
        Err(e) => {
            report.add(Verdict::Fail, format!("cannot reach the server: {e}"));
            report.summarise();
            return Ok(report.exit_code());
        }
    }

    let version = client
        .get("/api/version")
        .unwrap_or(serde_json::Value::Null);
    if let Some(v) = version["version"].as_str() {
        report.add(
            Verdict::Ok,
            format!(
                "version {v} (commit {})",
                version["commit"].as_str().unwrap_or("unknown")
            ),
        );
    }
    let features: Vec<&str> = version["features"]
        .as_array()
        .map(|list| list.iter().filter_map(|f| f.as_str()).collect())
        .unwrap_or_default();
    if features.is_empty() {
        report.add(Verdict::Warn, "the server advertises no features");
    } else {
        report.add(Verdict::Ok, format!("features: {}", features.join(" ")));
    }

    // --- authentication ----------------------------------------------------
    report.section("authentication");
    match client.get("/api/auth/whoami") {
        Ok(who) => report.add(
            Verdict::Ok,
            format!(
                "this machine is enrolled as \"{}\" (auth v{}, id {})",
                who["label"].as_str().unwrap_or("?"),
                who["auth_version"].as_i64().unwrap_or(0),
                who["token_id"].as_str().unwrap_or("?")
            ),
        ),
        Err(e) => report.add(Verdict::Fail, format!("whoami failed: {e}")),
    }

    // A wrong key must be refused. Checking the *negative* matters as much as
    // the positive: a server that accepts anything would pass every other check
    // here while being wide open.
    let wrong = Client::new(
        client.base_url().to_string(),
        "obviously-not-the-key".into(),
        Some(profile.device_token.clone()),
    );
    match wrong.get("/api/units") {
        Err(_) => report.add(Verdict::Ok, "a wrong API key is refused"),
        Ok(_) => report.add(
            Verdict::Fail,
            "a wrong API key was ACCEPTED -- this server is not authenticating",
        ),
    }

    // --- units -------------------------------------------------------------
    report.section("units");
    let units = match client.get("/api/units") {
        Ok(u) => u,
        Err(e) => {
            report.add(Verdict::Fail, format!("cannot list units: {e}"));
            report.summarise();
            return Ok(report.exit_code());
        }
    };
    let list = units.as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        report.add(
            Verdict::Warn,
            "no units are configured -- add one with `breeze-core control` or the panel",
        );
        report.summarise();
        return Ok(report.exit_code());
    }
    report.add(Verdict::Ok, format!("{} unit(s) configured", list.len()));

    // The batch read, which is what a panel opens with.
    let started = std::time::Instant::now();
    match client.get("/api/units/state") {
        Ok(envelope) => {
            let elapsed = started.elapsed().as_millis();
            let states = envelope["states"].as_array().cloned().unwrap_or_default();
            let errors = envelope["errors"].as_array().cloned().unwrap_or_default();
            report.add(
                Verdict::Ok,
                format!(
                    "batch read: {} answered, {} did not, in {elapsed} ms",
                    states.len(),
                    errors.len()
                ),
            );
            if states.len() + errors.len() != list.len() {
                report.add(
                    Verdict::Fail,
                    format!(
                        "the batch covered {} of {} units",
                        states.len() + errors.len(),
                        list.len()
                    ),
                );
            }
            for failed in &errors {
                report.add(
                    Verdict::Warn,
                    format!(
                        "{} did not answer: {}",
                        failed["name"].as_str().unwrap_or("a unit"),
                        failed["error"].as_str().unwrap_or("no reason given")
                    ),
                );
            }
        }
        Err(e) => report.add(Verdict::Fail, format!("batch read failed: {e}")),
    }

    // --- each unit ---------------------------------------------------------
    for unit in &list {
        let name = unit["name"].as_str().unwrap_or("?");
        let id = unit["id"].as_str().unwrap_or("");
        report.section(&format!("unit: {name} ({id})"));

        let started = std::time::Instant::now();
        let state = match client.get(&format!("/api/units/{id}/state")) {
            Ok(s) => s,
            Err(e) => {
                report.add(Verdict::Warn, format!("state read failed: {e}"));
                continue;
            }
        };
        let elapsed = started.elapsed().as_millis();

        if state["online"].as_bool() == Some(false) {
            report.add(Verdict::Warn, "the unit reports itself offline");
            continue;
        }
        report.add(Verdict::Ok, format!("state read in {elapsed} ms"));
        if elapsed > SLOW_MS {
            report.add(
                Verdict::Warn,
                format!("{elapsed} ms is slow -- check the wireless signal to this unit"),
            );
        }

        check_state(&mut report, &state);

        // Capabilities, where the server offers them: a client hides controls
        // based on this, so a wrong answer is a missing button.
        if features.contains(&"unit_capabilities") {
            match client.get(&format!("/api/units/{id}/capabilities")) {
                Ok(caps) => {
                    let modes = caps["operational_modes"]
                        .as_array()
                        .map(|m| m.len())
                        .unwrap_or(0);
                    if modes == 0 {
                        report.add(Verdict::Warn, "the unit reported no operating modes");
                    } else {
                        report.add(
                            Verdict::Ok,
                            format!(
                                "supports {modes} mode(s), {} swing, {} fan speed(s)",
                                caps["swing_modes"].as_array().map(|m| m.len()).unwrap_or(0),
                                caps["fan_speeds"].as_array().map(|m| m.len()).unwrap_or(0)
                            ),
                        );
                    }
                }
                Err(e) => report.add(Verdict::Warn, format!("capabilities failed: {e}")),
            }
        }
    }

    // --- the rest of the surface -------------------------------------------
    report.section("scheduling");
    if features.contains(&"programs") {
        match client.get("/api/programs/status") {
            Ok(status) => {
                let running = status["running"].as_bool() == Some(true);
                report.add(
                    if running { Verdict::Ok } else { Verdict::Fail },
                    format!(
                        "scheduler {} ({} run(s), {} error(s), tick {}s)",
                        if running { "running" } else { "NOT running" },
                        status["runs"].as_i64().unwrap_or(0),
                        status["errors"].as_i64().unwrap_or(0),
                        status["tick_seconds"].as_i64().unwrap_or(0)
                    ),
                );
            }
            Err(e) => report.add(Verdict::Fail, format!("scheduler status failed: {e}")),
        }
        match client.get("/api/programs") {
            Ok(programs) => {
                let all = programs.as_array().cloned().unwrap_or_default();
                let enabled = all
                    .iter()
                    .filter(|p| p["enabled"].as_bool() == Some(true))
                    .count();
                report.add(
                    Verdict::Ok,
                    format!("{} program(s), {enabled} enabled", all.len()),
                );
            }
            Err(e) => report.add(Verdict::Warn, format!("cannot list programs: {e}")),
        }
    } else {
        report.add(Verdict::Skip, "this server has no programs feature");
    }

    if features.contains(&"sleep_timer") {
        match client.get("/api/timers") {
            Ok(timers) => report.add(
                Verdict::Ok,
                format!(
                    "{} timer(s) pending",
                    timers.as_array().map(|t| t.len()).unwrap_or(0)
                ),
            ),
            Err(e) => report.add(Verdict::Warn, format!("cannot list timers: {e}")),
        }
    }

    report.section("input validation");
    // A bad value must be refused before it reaches the firmware. Checked with
    // the *first* unit, and it is a read-only failure by construction.
    let first = list[0]["id"].as_str().unwrap_or("");
    match client.post_json(
        &format!("/api/units/{first}/control"),
        &serde_json::json!({ "target_temperature": 99 }),
    ) {
        Err(_) => report.add(
            Verdict::Ok,
            "an out-of-range temperature is refused before it reaches a unit",
        ),
        Ok(_) => report.add(
            Verdict::Fail,
            "an out-of-range temperature was ACCEPTED and sent to a unit",
        ),
    }

    report.summarise();
    Ok(report.exit_code())
}

/// The sanity of one state document.
fn check_state(report: &mut Report, state: &serde_json::Value) {
    let known_modes = ["AUTO", "COOL", "DRY", "HEAT", "FAN_ONLY"];
    let known_swings = ["OFF", "VERTICAL", "HORIZONTAL", "BOTH"];

    match state["operational_mode"].as_str() {
        Some(mode) if known_modes.contains(&mode) => {
            report.add(Verdict::Ok, format!("mode {mode} is a known value"))
        }
        Some(other) => report.add(
            Verdict::Fail,
            format!("mode '{other}' is not one this API defines"),
        ),
        None => report.add(Verdict::Fail, "no operational_mode in the state"),
    }

    match state["swing_mode"].as_str() {
        Some(swing) if known_swings.contains(&swing) => {
            report.add(Verdict::Ok, format!("swing {swing} is a known value"))
        }
        Some(other) => report.add(
            Verdict::Fail,
            format!("swing '{other}' is not one this API defines"),
        ),
        None => report.add(Verdict::Fail, "no swing_mode in the state"),
    }

    match state["target_temperature"].as_f64() {
        Some(t) if (16.0..=30.0).contains(&t) => {
            report.add(Verdict::Ok, format!("target {t} °C is in range"))
        }
        Some(t) => report.add(
            Verdict::Fail,
            format!("target {t} °C is outside the 16-30 the API allows"),
        ),
        None => report.add(Verdict::Fail, "no target_temperature in the state"),
    }

    // Readings are allowed to be absent -- not every unit has an outdoor probe --
    // but a present reading has to be plausible. A unit reporting 255 °C is the
    // sentinel-instead-of-null case, and worth naming.
    for field in ["indoor_temperature", "outdoor_temperature"] {
        match state[field].as_f64() {
            None => report.add(Verdict::Skip, format!("{field} is not reported")),
            Some(t) if (SANE_MIN..=SANE_MAX).contains(&t) => {
                report.add(Verdict::Ok, format!("{field} {t} °C"))
            }
            Some(t) => report.add(
                Verdict::Warn,
                format!("{field} {t} °C is not a real temperature -- the unit has no such sensor"),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_with_no_failures_exits_zero() {
        let mut report = Report::new();
        report.add(Verdict::Ok, "fine");
        report.add(Verdict::Warn, "slow");
        report.add(Verdict::Skip, "not applicable");
        assert_eq!(report.exit_code(), 0, "a warning is not a failure");
    }

    #[test]
    fn a_single_failure_exits_non_zero() {
        let mut report = Report::new();
        report.add(Verdict::Ok, "fine");
        report.add(Verdict::Fail, "broken");
        assert_eq!(report.exit_code(), 1);
    }

    #[test]
    fn a_good_state_passes_every_check() {
        let mut report = Report::new();
        check_state(
            &mut report,
            &serde_json::json!({
                "operational_mode": "COOL",
                "swing_mode": "BOTH",
                "target_temperature": 24.0,
                "indoor_temperature": 26.5,
                "outdoor_temperature": 28.0,
            }),
        );
        assert_eq!(report.count(Verdict::Fail), 0);
        assert_eq!(report.count(Verdict::Warn), 0);
    }

    #[test]
    fn a_bare_integer_mode_is_caught() {
        // The bug this project has already had: reading an enum with `str()`
        // instead of `.name` yields "2" rather than "COOL" on Python 3.11+.
        let mut report = Report::new();
        check_state(
            &mut report,
            &serde_json::json!({
                "operational_mode": "2",
                "swing_mode": "BOTH",
                "target_temperature": 24.0,
            }),
        );
        assert!(report.count(Verdict::Fail) > 0);
    }

    #[test]
    fn a_missing_outdoor_probe_is_skipped_not_failed() {
        // Most units do not have one, and calling that a fault would make every
        // healthy server look broken.
        let mut report = Report::new();
        check_state(
            &mut report,
            &serde_json::json!({
                "operational_mode": "AUTO",
                "swing_mode": "OFF",
                "target_temperature": 22.0,
                "indoor_temperature": 21.0,
                "outdoor_temperature": null,
            }),
        );
        assert_eq!(report.count(Verdict::Fail), 0);
        assert_eq!(report.count(Verdict::Skip), 1);
    }

    #[test]
    fn a_sentinel_reading_is_a_warning_with_an_explanation() {
        // A unit with no probe sometimes reports 255 rather than null.
        let mut report = Report::new();
        check_state(
            &mut report,
            &serde_json::json!({
                "operational_mode": "COOL",
                "swing_mode": "OFF",
                "target_temperature": 24.0,
                "outdoor_temperature": 255.0,
            }),
        );
        assert_eq!(report.count(Verdict::Warn), 1);
        assert_eq!(report.count(Verdict::Fail), 0, "not a failure, just absent");
        assert!(report
            .checks
            .iter()
            .any(|(_, m)| m.contains("no such sensor")));
    }

    #[test]
    fn an_out_of_range_target_is_a_failure() {
        let mut report = Report::new();
        check_state(
            &mut report,
            &serde_json::json!({
                "operational_mode": "COOL",
                "swing_mode": "OFF",
                "target_temperature": 99.0,
            }),
        );
        assert!(report.count(Verdict::Fail) > 0);
    }

    #[test]
    fn a_state_missing_its_fields_fails_rather_than_passing_quietly() {
        let mut report = Report::new();
        check_state(&mut report, &serde_json::json!({}));
        assert_eq!(
            report.count(Verdict::Fail),
            3,
            "mode, swing and target are all required"
        );
    }
}
