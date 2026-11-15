//! The state log: every unit's state, appended to a CSV file on a clock.
//!
//! Off unless `BREEZE_STATE_LOG` names a file. Then one row per unit is written
//! every `BREEZE_STATE_LOG_INTERVAL` (five minutes unless it says otherwise),
//! whether or not any client is open -- the point is a record of what the units
//! did while nobody was looking, which nothing else here keeps: the history
//! endpoint holds the last few hundred readings in memory and loses them at a
//! restart.
//!
//! Three choices worth knowing before changing any of this:
//!
//! * **A bare file name lands beside the store files.** That directory is the
//!   one place every service manager here already lets the service write --
//!   the systemd unit is `ProtectSystem=strict` with only it in
//!   `ReadWritePaths` -- so `BREEZE_STATE_LOG=states.csv` works on every
//!   package without touching the unit. Anywhere else is the person's to allow.
//! * **The file is opened for each write and closed again.** Nothing is held
//!   open, so logrotate (or a person) can move or delete it at any moment and
//!   the next row starts a fresh file, header first.
//! * **An offline unit's row says so and stops there.** The stream keeps a
//!   vanished unit's last known values so a card has something to show; a log
//!   that repeated them would record temperatures nobody measured.
//!
//! Rows are sampled through the stream poller's own [`crate::stream::poll_one`],
//! so a logged reading also lands in the history and reaches any open stream,
//! and a unit is never read twice for the same moment.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::state::{AppState, KeepWarm};

/// How often, unless `BREEZE_STATE_LOG_INTERVAL` says otherwise.
pub const DEFAULT_EVERY: Duration = Duration::from_secs(5 * 60);

/// The shortest interval allowed. Every row is a LAN round-trip to a unit, and
/// a unit busy answering the log is a unit slower to answer a person.
pub const MIN_EVERY: Duration = Duration::from_secs(30);

/// The name `BREEZE_STATE_LOG=on` uses, in the store directory.
pub const DEFAULT_NAME: &str = "states.csv";

/// The first line of every file this writes.
pub const HEADER: &str = "time,epoch,unit_id,name,online,power,mode,target,fan_speed,swing,eco,\
                          turbo,indoor,outdoor\n";

/// Where to log, from `BREEZE_STATE_LOG`. `None`, the default, is off.
pub fn path_from_env(config_dir: &str) -> Option<PathBuf> {
    resolve(&std::env::var("BREEZE_STATE_LOG").ok()?, config_dir)
}

/// `on` and its synonyms mean [`DEFAULT_NAME`] in the store directory; `off`
/// and an empty value mean off; a relative path is taken from the store
/// directory; an absolute one is used as it is.
pub fn resolve(raw: &str, config_dir: &str) -> Option<PathBuf> {
    let raw = raw.trim();
    let word = raw.to_ascii_lowercase();
    if raw.is_empty() || matches!(word.as_str(), "0" | "off" | "no" | "false" | "none") {
        return None;
    }
    let path = if matches!(word.as_str(), "1" | "on" | "yes" | "true") {
        PathBuf::from(DEFAULT_NAME)
    } else {
        PathBuf::from(raw)
    };
    Some(if path.is_absolute() {
        path
    } else {
        Path::new(config_dir).join(path)
    })
}

/// How often, from `BREEZE_STATE_LOG_INTERVAL`.
pub fn interval_from_env() -> Duration {
    match std::env::var("BREEZE_STATE_LOG_INTERVAL") {
        Err(_) => DEFAULT_EVERY,
        Ok(raw) => match parse_interval(&raw) {
            Some(every) if every < MIN_EVERY => {
                eprintln!(
                    "breeze-core: BREEZE_STATE_LOG_INTERVAL={raw:?} is shorter than {}s; using {}s.",
                    MIN_EVERY.as_secs(),
                    MIN_EVERY.as_secs()
                );
                MIN_EVERY
            }
            Some(every) => every,
            None => {
                // Said out loud rather than guessed at, as BREEZE_KEEP_WARM is.
                eprintln!(
                    "breeze-core: BREEZE_STATE_LOG_INTERVAL={raw:?} is not a duration; using {}. \
                     Give seconds, or a number with s, m or h.",
                    describe(DEFAULT_EVERY)
                );
                DEFAULT_EVERY
            }
        },
    }
}

/// `300`, `90s`, `5m`, `1h` -- the same grammar as `BREEZE_KEEP_WARM`, minus
/// its words, which mean nothing for an interval.
pub fn parse_interval(raw: &str) -> Option<Duration> {
    match KeepWarm::parse(raw)? {
        KeepWarm::For(every) => Some(every),
        KeepWarm::Off | KeepWarm::Always => None,
    }
}

/// An interval as a person would write it back.
pub fn describe(every: Duration) -> String {
    KeepWarm::For(every).describe()
}

/// Start the log, if `BREEZE_STATE_LOG` asked for one.
pub fn spawn(state: Arc<AppState>) -> Option<std::thread::JoinHandle<()>> {
    let path = state.settings.state_log.clone()?;
    let every = state.settings.state_log_every;
    eprintln!(
        "breeze-core: logging every unit's state to {} every {}",
        path.display(),
        describe(every)
    );
    Some(std::thread::spawn(move || {
        // Said once when writing starts failing and once when it recovers,
        // not every five minutes in between.
        let mut failing: Option<String> = None;
        loop {
            sleep_until_next(every);
            let rows = sample(&state);
            match append(&path, &rows) {
                Ok(()) => {
                    if failing.take().is_some() {
                        eprintln!(
                            "breeze-core: state log: writing to {} again",
                            path.display()
                        );
                    }
                }
                Err(e) => {
                    let message = e.to_string();
                    if failing.as_deref() != Some(message.as_str()) {
                        eprintln!(
                            "breeze-core: state log: cannot write {}: {message}",
                            path.display()
                        );
                        failing = Some(message);
                    }
                }
            }
        }
    }))
}

/// Wait for the next whole multiple of `every` on the wall clock, so rows land
/// on :00, :05, :10 rather than wherever the server happened to start.
fn sleep_until_next(every: Duration) {
    let secs = every.as_secs().max(1);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let into = now.as_secs() % secs;
    // At least one second of `secs - into` remains, so subtracting the part
    // of the current second already gone cannot underflow.
    let wait =
        Duration::from_secs(secs - into) - Duration::from_nanos(u64::from(now.subsec_nanos()));
    std::thread::sleep(wait);
}

/// One row per unit, read now.
fn sample(state: &AppState) -> String {
    let time = breeze_store::now_local()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let epoch = crate::history::now_unix();
    state
        .manager
        .known_units()
        .into_iter()
        .map(|id| row(&crate::stream::poll_one(state, id), &time, epoch))
        .collect()
}

/// Append `rows`, with the header first if the file is new or empty.
fn append(path: &Path, rows: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o640);
    }
    let mut file = options.open(path)?;
    let mut out = String::new();
    if file.metadata()?.len() == 0 {
        out.push_str(HEADER);
    }
    out.push_str(rows);
    // One write per tick, so a reader never sees half of one.
    file.write_all(out.as_bytes())
}

/// One unit's row, from the state the API would send for it.
pub fn row(state: &serde_json::Value, time: &str, epoch: i64) -> String {
    let text = |key: &str| state.get(key).and_then(|v| v.as_str()).unwrap_or("");
    let flag = |key: &str| match state.get(key).and_then(|v| v.as_bool()) {
        Some(true) => "true".to_string(),
        Some(false) => "false".to_string(),
        None => String::new(),
    };
    let degrees = |key: &str| {
        state
            .get(key)
            .and_then(|v| v.as_f64())
            .map(|t| format!("{t:.1}"))
            .unwrap_or_default()
    };
    let online = state.get("online").and_then(|v| v.as_bool()) == Some(true);

    let mut fields = vec![
        time.to_string(),
        epoch.to_string(),
        text("id").to_string(),
        field(&spreadsheet_safe(text("name"))),
        online.to_string(),
    ];
    if online {
        fields.extend([
            flag("power_state"),
            text("operational_mode").to_string(),
            degrees("target_temperature"),
            state
                .get("fan_speed")
                .and_then(|v| v.as_u64())
                .map(|f| f.to_string())
                .unwrap_or_default(),
            text("swing_mode").to_string(),
            flag("eco"),
            flag("turbo"),
            degrees("indoor_temperature"),
            degrees("outdoor_temperature"),
        ]);
    } else {
        fields.extend(std::iter::repeat_n(String::new(), 9));
    }
    fields.join(",") + "\n"
}

/// Quote a field when it needs it, by doubling its quotes (RFC 4180).
fn field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// A unit's name, kept from being read as a formula. A spreadsheet runs a cell
/// that starts with `=`, `+`, `-` or `@`, and a name is whatever someone typed.
fn spreadsheet_safe(name: &str) -> String {
    if name.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{name}")
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn living_room() -> serde_json::Value {
        serde_json::json!({
            "id": "152832116426242",
            "name": "Living room",
            "ip": "192.168.1.20",
            "online": true,
            "power_state": true,
            "operational_mode": "COOL",
            "target_temperature": 23.5,
            "indoor_temperature": 25.1,
            "outdoor_temperature": 31.0,
            "fan_speed": 102,
            "swing_mode": "VERTICAL",
            "eco": false,
            "turbo": true,
        })
    }

    #[test]
    fn a_row_has_every_column_of_the_header_in_order() {
        let line = row(&living_room(), "2026-11-13 16:05:00", 1_794_585_900);
        assert_eq!(
            line,
            "2026-11-13 16:05:00,1794585900,152832116426242,Living room,true,true,COOL,\
             23.5,102,VERTICAL,false,true,25.1,31.0\n"
        );
        assert_eq!(
            line.split(',').count(),
            HEADER.split(',').count(),
            "one field per column"
        );
    }

    #[test]
    fn whole_degrees_keep_their_decimal_so_a_column_reads_alike() {
        let mut state = living_room();
        state["target_temperature"] = serde_json::json!(24.0);
        assert!(row(&state, "t", 0).contains(",24.0,"));
    }

    #[test]
    fn a_missing_sensor_is_an_empty_field_not_a_zero() {
        let mut state = living_room();
        state["outdoor_temperature"] = serde_json::Value::Null;
        assert!(row(&state, "t", 0).ends_with(",25.1,\n"));
    }

    #[test]
    fn an_offline_unit_records_only_that_it_was_offline() {
        // What the stream hands back for an unreachable unit: the last known
        // values, flagged offline. None of them were measured just now.
        let mut state = living_room();
        state["online"] = serde_json::json!(false);
        assert_eq!(
            row(&state, "t", 7),
            "t,7,152832116426242,Living room,false,,,,,,,,,\n"
        );
    }

    #[test]
    fn names_are_quoted_when_they_need_it_and_never_run_as_formulas() {
        let mut state = living_room();
        state["name"] = serde_json::json!("Kids, \"upstairs\"");
        assert!(row(&state, "t", 0).contains(",\"Kids, \"\"upstairs\"\"\",true,"));
        state["name"] = serde_json::json!("=HYPERLINK(\"x\")");
        assert!(row(&state, "t", 0).contains(",\"'=HYPERLINK(\"\"x\"\")\",true,"));
        state["name"] = serde_json::json!("-cellar");
        assert!(row(&state, "t", 0).contains(",'-cellar,true,"));
    }

    #[test]
    fn negative_temperatures_are_numbers_not_guarded_text() {
        let mut state = living_room();
        state["outdoor_temperature"] = serde_json::json!(-4.5);
        assert!(row(&state, "t", 0).ends_with(",25.1,-4.5\n"));
    }

    #[test]
    fn where_the_file_goes() {
        let dir = if cfg!(windows) {
            "C:\\breeze"
        } else {
            "/etc/breeze-core"
        };
        assert_eq!(resolve("", dir), None);
        assert_eq!(resolve("off", dir), None);
        assert_eq!(resolve(" 0 ", dir), None);
        assert_eq!(resolve("on", dir), Some(Path::new(dir).join("states.csv")));
        assert_eq!(resolve("YES", dir), Some(Path::new(dir).join("states.csv")));
        assert_eq!(
            resolve("log.csv", dir),
            Some(Path::new(dir).join("log.csv"))
        );
        let absolute = if cfg!(windows) {
            "D:\\logs\\ac.csv"
        } else {
            "/var/log/ac.csv"
        };
        assert_eq!(resolve(absolute, dir), Some(PathBuf::from(absolute)));
    }

    #[test]
    fn intervals_read_like_keep_warm_and_refuse_its_words() {
        assert_eq!(parse_interval("300"), Some(Duration::from_secs(300)));
        assert_eq!(parse_interval("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_interval("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_interval("always"), None);
        assert_eq!(parse_interval("off"), None);
        assert_eq!(
            parse_interval("0"),
            None,
            "no interval at all is not an interval"
        );
        assert_eq!(parse_interval("soon"), None);
    }

    #[test]
    fn the_header_is_written_once_however_many_ticks_follow() {
        let dir = std::env::temp_dir().join(format!("breeze-state-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("states.csv");
        let _ = std::fs::remove_file(&path);
        append(&path, "a\n").unwrap();
        append(&path, "b\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("{HEADER}a\nb\n")
        );
        // Moved away by logrotate: the next tick starts a fresh file, header first.
        std::fs::remove_file(&path).unwrap();
        append(&path, "c\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("{HEADER}c\n")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
