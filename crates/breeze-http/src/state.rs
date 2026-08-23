//! Everything a request handler needs, assembled once at startup.
//!
//! Mirrors Breeze Core's app factory: one place wires the stores, the device
//! manager and the authenticator together, and nothing reaches for a global.
//! That is what makes the whole surface testable against a fake configuration.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use breeze_auth::{EnrollmentService, NonceCache, Verifier};
use breeze_device::{DeviceManager, UnitConfig as DeviceUnit};
use breeze_store::{AppConfig, DevicesDoc};

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
    /// Whether to trust `X-Forwarded-For`. Only true behind a real proxy:
    /// otherwise any client could claim a private address and approve its own
    /// pairing.
    pub behind_proxy: bool,
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
            worker_threads: std::env::var("BREEZE_WORKERS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8),
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
            Self::NoApiKey(p) => write!(
                f,
                "{} has no api_key — run `breeze-core pair` to set the deployment up",
                p.display()
            ),
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

        let units: Vec<DeviceUnit> = config.units.iter().filter_map(to_device_unit).collect();
        let manager = Arc::new(DeviceManager::new(units));
        let verifier = Verifier::new(settings.min_auth_version);

        Ok(Self {
            settings,
            version,
            config: RwLock::new(config),
            devices: RwLock::new(devices),
            manager,
            verifier,
            nonces: Mutex::new(NonceCache::default()),
            enrollment: Mutex::new(EnrollmentService::default()),
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
fn to_device_unit(unit: &breeze_store::UnitConfig) -> Option<DeviceUnit> {
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
            behind_proxy: false,
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
