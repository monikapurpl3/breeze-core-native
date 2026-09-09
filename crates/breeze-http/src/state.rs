//! Everything a request handler needs, assembled once at startup.
//!
//! Mirrors Breeze Core's app factory: one place wires the stores, the device
//! manager and the authenticator together, and nothing reaches for a global.
//! That is what makes the whole surface testable against a fake configuration.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use breeze_auth::{EnrollmentService, NonceCache, Verifier};
use breeze_device::{DeviceManager, UnitConfig as DeviceUnit};
use breeze_store::{AppConfig, DevicesDoc, ProgramsDoc, TimersDoc};

/// Where the four store files live, and the knobs that change behaviour.
#[derive(Debug, Clone)]
pub struct Settings {
    pub config_path: PathBuf,
    pub devices_path: PathBuf,
    pub programs_path: PathBuf,
    pub timers_path: PathBuf,
    pub bind: String,
    /// Refuse device credentials below this version. 1 keeps every existing
    /// device working, which is the drop-in default.
    pub min_auth_version: u8,
    pub worker_threads: usize,
    /// How many units the background state poller contacts concurrently. 1 is
    /// the sequential walk every release before 4.0.2 did.
    pub bg_workers: usize,
    /// Whether to trust `X-Forwarded-For`. Only true behind a real proxy:
    /// otherwise any client could claim a private address and approve its own
    /// pairing.
    pub behind_proxy: bool,
    /// Whether to send the hardening headers. Off when a reverse proxy already
    /// sets them: duplicated CSP headers are intersected, not deduplicated.
    pub security_headers: bool,
    /// Seconds between due-checks. Finer than a scheduler's tick because a
    /// schedule only has to hit the right minute, while a timer is a promise
    /// about a moment.
    pub timer_tick_seconds: u64,
    /// Seconds between scheduler passes. A schedule only has to land inside the
    /// right minute, so this is coarser than the timer runner.
    pub sched_tick_seconds: u64,
    /// Seconds between central state polls, while at least one client is
    /// streaming. Matches the cadence clients used to poll at themselves.
    pub stream_tick_seconds: u64,
    /// Samples kept per unit for the history endpoint and /metrics.
    pub history_size: usize,
    /// How long a pairing code lives. Seconds.
    pub code_ttl_seconds: u64,
    /// How long a device credential lives. Days; zero or less means never.
    pub token_ttl_days: i64,
    /// How far a signed request's timestamp may be from server time, each way.
    pub auth_skew_seconds: u64,
}

impl Settings {
    /// Read from the environment, using the same variable names as Breeze Core so
    /// an existing unit file or container keeps working untouched.
    pub fn from_env() -> Self {
        let dir = std::env::var("AC_CONFIG_DIR").unwrap_or_else(|_| "/etc/breeze-core".into());
        let in_dir = |name: &str| PathBuf::from(&dir).join(name);
        Self {
            config_path: env_path("AC_CONFIG", in_dir("config.json")),
            devices_path: env_path("AC_DEVICES", in_dir("devices.json")),
            programs_path: env_path("AC_PROGRAMS", in_dir("programs.json")),
            timers_path: env_path("AC_TIMERS", in_dir("timers.json")),
            bind: format!(
                "{}:{}",
                std::env::var("BREEZE_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
                std::env::var("BREEZE_PORT").unwrap_or_else(|_| "8420".into())
            ),
            min_auth_version: std::env::var("AC_MIN_AUTH_VERSION")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1),
            // Both pools are clamped to at least 1 HERE, at the boundary,
            // rather than only where they are used. /api/system reports these
            // fields, and a diagnostic that prints 0 while the server is
            // actually running one lane is telling you about the string in the
            // env file instead of about the server -- which is the whole
            // failure this endpoint exists to avoid.
            worker_threads: std::env::var("BREEZE_WORKERS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(|n: usize| n.max(1))
                .unwrap_or(8),
            // How many units the background state poller contacts at once.
            //
            // Defaults to 1, which is exactly what every release before this
            // one did: poll_once walks the units in order, paying a LAN
            // round-trip each. That is fine for a few units and stops being
            // fine when the walk takes longer than AC_STREAM_TICK, at which
            // point every tick starts late and the stream falls behind the
            // units it is reporting on.
            //
            // Raising it is safe because each unit has its own lock and its own
            // cached connection, so two units are genuinely independent; what
            // it must NOT be read as is "more background threads". The timer
            // runner, the scheduler and the poller are one thread each because
            // each is a singleton role — a second scheduler would fire every
            // program twice.
            bg_workers: std::env::var("BREEZE_BG_WORKERS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(|n: usize| n.max(1))
                .unwrap_or(1),
            timer_tick_seconds: std::env::var("AC_TIMER_TICK")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(15),
            history_size: std::env::var("AC_HISTORY_SIZE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(crate::history::DEFAULT_SIZE),
            // These three were constants in 4.0.0 and are settings again.
            //
            // The reasoning for fixing them was that each was "a switch whose
            // only use was a worse configuration". For the credential lifetime
            // that was simply wrong: on a LAN deployment the alternative to a
            // long life is re-pairing every phone in the house on a timer, so a
            // longer one is the better choice and the reference let you say so.
            // Removing the setting turned a deliberate choice into an
            // unreachable one -- and silently shortened it for anyone who had
            // set it.
            code_ttl_seconds: std::env::var("AC_CODE_TTL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(breeze_auth::enroll::CODE_TTL_SECONDS),
            token_ttl_days: std::env::var("AC_TOKEN_TTL_DAYS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(breeze_auth::enroll::TOKEN_TTL_DAYS),
            auth_skew_seconds: std::env::var("AC_AUTH_SKEW_SECONDS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(breeze_auth::signing::DEFAULT_SKEW_SECONDS),
            stream_tick_seconds: std::env::var("AC_STREAM_TICK")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            sched_tick_seconds: std::env::var("AC_SCHED_TICK")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            security_headers: !matches!(
                std::env::var("AC_SECURITY_HEADERS").as_deref(),
                Ok("0") | Ok("false") | Ok("no")
            ),
            behind_proxy: matches!(
                std::env::var("AC_BEHIND_PROXY").as_deref(),
                Ok("1") | Ok("true") | Ok("yes")
            ),
        }
    }
}

fn env_path(key: &str, fallback: PathBuf) -> PathBuf {
    std::env::var(key).map(PathBuf::from).unwrap_or(fallback)
}

pub struct AppState {
    pub settings: Settings,
    /// The *binary's* version, passed in by main: `env!("CARGO_PKG_VERSION")`
    /// inside this library would report the library's version instead, which
    /// is what `/api/version` accidentally advertised at first.
    pub version: &'static str,
    /// The admin-managed configuration. Behind a lock because a future
    /// config-API write reloads it.
    pub config: RwLock<AppConfig>,
    /// Enrolled clients. Read on every authenticated request, written on
    /// enrolment and revocation.
    pub devices: RwLock<DevicesDoc>,
    pub manager: Arc<DeviceManager>,
    pub verifier: Verifier,
    /// Shared, because replay protection is only meaningful across all threads.
    pub nonces: Mutex<NonceCache>,
    /// Pending pairings. In memory only: they live for a minute, and losing
    /// them on restart just means whoever was mid-pairing starts again.
    pub enrollment: Mutex<EnrollmentService>,
    /// Pending one-shot timers. Durable, unlike enrolment sessions: a timer
    /// is a promise that must survive a restart.
    pub timers: RwLock<TimersDoc>,
    pub runner: Mutex<crate::timer_routes::RunnerStats>,
    /// Stored favourites, schedules and curves.
    pub programs: RwLock<ProgramsDoc>,
    /// The scheduler's counters and its per-trigger firing memory.
    pub scheduler: Mutex<crate::program_routes::SchedulerState>,
    /// The central state poller and its SSE subscribers.
    pub stream: crate::stream::StateStream,
    /// Held for the duration of a LAN scan. One at a time: a scan is hundreds
    /// of packets over several seconds, and two would double the traffic to
    /// answer the same question.
    pub scanning: Mutex<()>,
    /// Recent readings, filled by whatever happens to read a state.
    pub history: crate::history::History,
    pub started_at: std::time::SystemTime,
}

#[derive(Debug)]
pub enum StartupError {
    Store(String),
    /// The config exists but has no API key, so nothing could ever authenticate.
    NoApiKey(PathBuf),
}

impl core::fmt::Display for StartupError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Store(e) => write!(f, "{e}"),
            Self::NoApiKey(p) => {
                write!(
                    f,
                    "{} has no api_key — run `breeze-core pair` to set the deployment up",
                    p.display()
                )?;
                // Point at a legacy config if one is sitting there, because
                // following the advice above would otherwise pair from scratch
                // and look like a paired unit's V3 credentials had been lost.
                // /etc/meow-ac is where releases before 2.5.0 defaulted, so
                // anyone who never set AC_CONFIG has their units in there.
                for legacy in ["/etc/meow-ac/config.json", "/usr/local/etc/meow-ac/config.json"] {
                    let path = std::path::Path::new(legacy);
                    if path != p.as_path() && path.exists() {
                        // The newlines and leading spaces below are the message's
                        // own wrapping, so the continuation lines sit at column
                        // 0 in the source. Do not re-indent them to match this
                        // block: Rust keeps everything between the quotes, and
                        // this message shipped once as a single line with
                        // thirty-space gaps in the middle of it.
                        write!(
                            f,
                            "
  note: {legacy} exists. If that is your existing deployment, point
        AC_CONFIG_DIR at its directory rather than pairing again — pairing
        again cannot recover a V3 unit's credentials."
                        )?;
                        break;
                    }
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for StartupError {}

impl AppState {
    /// Load the stores and build everything from them.
    ///
    /// A missing `config.json` is fatal with the same guidance Breeze Core gives,
    /// because a server with no units and no key can only produce confusing 401s.
    pub fn load(settings: Settings, version: &'static str) -> Result<Self, StartupError> {
        let config: AppConfig = breeze_store::load(&settings.config_path)
            .map_err(|e| StartupError::Store(e.to_string()))?;
        if config.api_key.as_deref().unwrap_or("").is_empty() {
            return Err(StartupError::NoApiKey(settings.config_path.clone()));
        }
        let devices: DevicesDoc = breeze_store::load(&settings.devices_path)
            .map_err(|e| StartupError::Store(e.to_string()))?;

        let timers: TimersDoc = breeze_store::load(&settings.timers_path)
            .map_err(|e| StartupError::Store(e.to_string()))?;

        let programs: ProgramsDoc = breeze_store::load(&settings.programs_path)
            .map_err(|e| StartupError::Store(e.to_string()))?;

        let units: Vec<DeviceUnit> = config.units.iter().filter_map(to_device_unit).collect();
        let manager = Arc::new(DeviceManager::new(units));
        let verifier = Verifier::new(settings.min_auth_version);
        let stream = crate::stream::StateStream::new(settings.stream_tick_seconds);
        let history = crate::history::History::new(settings.history_size);
        // Read before `settings` is moved into the struct below.
        let nonces = NonceCache::new(settings.auth_skew_seconds);
        let enrollment =
            EnrollmentService::new(settings.code_ttl_seconds, settings.token_ttl_days);

        Ok(Self {
            settings,
            version,
            config: RwLock::new(config),
            devices: RwLock::new(devices),
            manager,
            verifier,
            nonces: Mutex::new(nonces),
            enrollment: Mutex::new(enrollment),
            timers: RwLock::new(timers),
            runner: Mutex::new(crate::timer_routes::RunnerStats::default()),
            programs: RwLock::new(programs),
            scheduler: Mutex::new(crate::program_routes::SchedulerState::default()),
            stream,
            scanning: Mutex::new(()),
            history,
            started_at: std::time::SystemTime::now(),
        })
    }

    pub fn api_key(&self) -> Option<String> {
        self.config.read().ok()?.api_key.clone()
    }
}

/// Turn a stored unit into one the device layer can drive.
///
/// A unit with an unparseable address or a malformed key is skipped rather than
/// fatal: one bad entry in `config.json` must not stop the other units working.
pub(crate) fn to_device_unit(unit: &breeze_store::UnitConfig) -> Option<DeviceUnit> {
    let ip = unit.ip.parse().ok()?;
    let key = unit
        .key
        .as_deref()
        .and_then(hex_to_bytes)
        .and_then(|v| <[u8; 32]>::try_from(v.as_slice()).ok());
    Some(DeviceUnit {
        // config.json stores the id as a JSON number, which serde reads as
        // i64; the device layer wants u64. A negative id is nonsense, so a
        // failed conversion skips the unit rather than wrapping around.
        id: u64::try_from(unit.id).ok()?,
        name: unit.name.clone(),
        ip,
        port: unit.port,
        token: unit.token.as_deref().and_then(hex_to_bytes),
        key,
    })
}

fn hex_to_bytes(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_decoding_refuses_malformed_input() {
        assert_eq!(hex_to_bytes("00ff").unwrap(), vec![0, 255]);
        assert!(hex_to_bytes("").is_none(), "empty is not a credential");
        assert!(hex_to_bytes("abc").is_none(), "odd length");
        assert!(hex_to_bytes("zz").is_none(), "not hex");
    }

    #[test]
    fn a_unit_with_a_bad_address_is_skipped_not_fatal() {
        let bad = breeze_store::UnitConfig {
            name: "Broken".into(),
            ip: "not-an-ip".into(),
            port: 6444,
            id: 1,
            token: None,
            key: None,
        };
        assert!(to_device_unit(&bad).is_none());
    }

    #[test]
    fn a_unit_with_a_short_key_still_loads_without_credentials() {
        // Better to have the unit listed and unreachable than to hide it: a
        // client can then show "no credentials" instead of the unit vanishing.
        let unit = breeze_store::UnitConfig {
            name: "Half paired".into(),
            ip: "192.0.2.1".into(),
            port: 6444,
            id: 7,
            token: Some("aa".repeat(64)),
            key: Some("bb".repeat(8)), // 8 bytes, not 32
        };
        let d = to_device_unit(&unit).expect("unit should still be listed");
        assert!(d.key.is_none(), "a malformed key must not be half-accepted");
        assert!(d.token.is_some());
    }

    #[test]
    fn credentials_decode_when_well_formed() {
        let unit = breeze_store::UnitConfig {
            name: "Paired".into(),
            ip: "192.0.2.1".into(),
            port: 6444,
            id: 7,
            token: Some("aa".repeat(64)),
            key: Some("bb".repeat(32)),
        };
        let d = to_device_unit(&unit).unwrap();
        assert_eq!(d.token.unwrap().len(), 64);
        assert_eq!(d.key.unwrap().len(), 32);
    }

    #[test]
    fn settings_default_to_breeze_cores_paths_and_port() {
        // Read with the environment unset in this process's view of the defaults.
        let s = Settings {
            config_path: "/etc/breeze-core/config.json".into(),
            devices_path: "/etc/breeze-core/devices.json".into(),
            programs_path: "/etc/breeze-core/programs.json".into(),
            timers_path: "/etc/breeze-core/timers.json".into(),
            bind: "127.0.0.1:8420".into(),
            min_auth_version: 1,
            worker_threads: 8,
            bg_workers: 1,
            behind_proxy: false,
            timer_tick_seconds: 15,
            sched_tick_seconds: 30,
            stream_tick_seconds: 5,
            history_size: 720,
            code_ttl_seconds: 60,
            token_ttl_days: 3650,
            auth_skew_seconds: 60,
            security_headers: true,
        };
        assert_eq!(
            s.min_auth_version, 1,
            "must not lock out v1 devices by default"
        );
        assert!(s.bind.ends_with(":8420"));
        assert!(
            s.bind.starts_with("127.0.0.1"),
            "must not bind every interface by default"
        );
        assert!(
            !s.behind_proxy,
            "must not trust X-Forwarded-For unless told to"
        );
    }
}
