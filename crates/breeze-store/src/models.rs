//! The four store documents, shaped to match Breeze Core's files exactly.
//!
//! Field order here is not cosmetic: these structs are serialised straight back
//! over the user's own files, so declaration order **is** the byte order. Two
//! things in particular are easy to get wrong and are pinned by round-trip tests
//! against fixtures generated from the Python models:
//!
//! * `Program.id` comes **last**, because Breeze Core's `Program` subclasses
//!   `ProgramSpec` and pydantic appends subclass fields;
//! * every `Option` writes `null` rather than being skipped.

use serde::{Deserialize, Serialize};

use crate::control::ControlRequest;

fn default_port() -> u16 {
    6444
}

fn default_true() -> bool {
    true
}

fn default_auth_version() -> u8 {
    1
}

fn default_curve_mode() -> String {
    "COOL".into()
}

fn default_curve_fan() -> i64 {
    102
}

fn default_kind() -> String {
    "favourite".into()
}

// ---------------------------------------------------------------- config.json

/// One air conditioner as the admin configured it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitConfig {
    pub name: String,
    pub ip: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub id: i64,
    /// V3 credentials; `null` for V1/V2 devices.
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
}

/// `config.json` — admin-managed, mode 640 so the CLIs can read it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub units: Vec<UnitConfig>,
}

// --------------------------------------------------------------- devices.json

/// One enrolled client.
///
/// The credential depends on `auth_version`, and the two are mutually exclusive:
/// v1 stores the SHA-256 of a bearer token, v2 stores an Ed25519 **public** key
/// and therefore no secret at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub token_id: String,
    pub label: String,
    #[serde(default = "default_auth_version")]
    pub auth_version: u8,
    #[serde(default)]
    pub token_hash: Option<String>,
    #[serde(default)]
    pub public_key: Option<String>,
    pub created_at: f64,
    /// `null` means non-expiring.
    #[serde(default)]
    pub expires_at: Option<f64>,
    #[serde(default)]
    pub last_used: Option<f64>,
}

impl DeviceRecord {
    /// Whether this record carries the credential its version requires.
    ///
    /// Breeze Core enforces this at load and so must we: a v2 record with no
    /// public key cannot authenticate anything, and silently accepting it would
    /// leave a device that appears enrolled and never works.
    pub fn has_required_credential(&self) -> bool {
        match self.auth_version {
            1 => self.token_hash.is_some(),
            2 => self.public_key.is_some(),
            _ => false,
        }
    }
}

/// `devices.json` — written by the app at runtime, mode 600.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DevicesDoc {
    #[serde(default)]
    pub devices: Vec<DeviceRecord>,
}

// -------------------------------------------------------------- programs.json

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduleEntry {
    /// 0 = Monday .. 6 = Sunday.
    #[serde(default)]
    pub days: Vec<u8>,
    /// `HH:MM`, 24-hour, server-local.
    pub time: String,
    pub settings: ControlRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    pub time: String,
    pub temperature: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurveConfig {
    #[serde(default = "default_curve_mode")]
    pub operational_mode: String,
    #[serde(default = "default_curve_fan")]
    pub fan_speed: i64,
    #[serde(default)]
    pub points: Vec<CurvePoint>,
}

/// A stored program: favourite, schedule or curve.
///
/// `id` is last on purpose — see the module docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Empty means every configured unit.
    #[serde(default)]
    pub unit_ids: Vec<String>,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub favourite: Option<ControlRequest>,
    #[serde(default)]
    pub schedule: Vec<ScheduleEntry>,
    #[serde(default)]
    pub curve: Option<CurveConfig>,
    pub id: String,
}

/// `programs.json` — mode 600.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ProgramsDoc {
    #[serde(default)]
    pub programs: Vec<Program>,
}

// ---------------------------------------------------------------- timers.json

/// A one-shot timer.
///
/// `created_at` and `fires_at` are naive server-local ISO strings — the same
/// clock the scheduler works in. Clients count down from a server-computed
/// `seconds_remaining` instead of parsing these, because a phone's clock cannot
/// be trusted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timer {
    pub id: String,
    #[serde(default)]
    pub unit_ids: Vec<String>,
    pub minutes: u32,
    pub created_at: String,
    pub fires_at: String,
    pub settings: ControlRequest,
    #[serde(default)]
    pub label: String,
}

/// `timers.json` — mode 600.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TimersDoc {
    #[serde(default)]
    pub timers: Vec<Timer>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_id_serialises_last() {
        // Breeze Core's Program subclasses ProgramSpec, so pydantic appends `id`.
        // Declaring it first here would rewrite every programs.json.
        let p = Program {
            name: "x".into(),
            enabled: true,
            unit_ids: vec![],
            kind: "favourite".into(),
            favourite: None,
            schedule: vec![],
            curve: None,
            id: "p1".into(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let id_at = json.find("\"id\"").expect("id present");
        let name_at = json.find("\"name\"").expect("name present");
        assert!(
            name_at < id_at,
            "id must come after the spec fields: {json}"
        );
        assert!(json.ends_with("\"id\":\"p1\"}"), "got {json}");
    }

    #[test]
    fn defaults_match_breeze_core_when_a_key_is_absent() {
        let u: UnitConfig = serde_json::from_str(r#"{"name":"n","ip":"1.2.3.4","id":7}"#).unwrap();
        assert_eq!(u.port, 6444, "port defaults to the Midea LAN port");
        assert_eq!(u.token, None);

        let d: DeviceRecord = serde_json::from_str(
            r#"{"token_id":"t","label":"l","token_hash":"h","created_at":1.0}"#,
        )
        .unwrap();
        assert_eq!(d.auth_version, 1, "records without a version are v1 bearer");

        let c: CurveConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(c.operational_mode, "COOL");
        assert_eq!(c.fan_speed, 102);
    }

    #[test]
    fn a_version_without_its_credential_is_detectable() {
        let mut d = DeviceRecord {
            token_id: "t".into(),
            label: "l".into(),
            auth_version: 2,
            token_hash: None,
            public_key: None,
            created_at: 1.0,
            expires_at: None,
            last_used: None,
        };
        assert!(!d.has_required_credential(), "v2 needs a public key");
        d.public_key = Some("k".into());
        assert!(d.has_required_credential());

        // A v1 record is matched by its hash, never by a public key.
        let v1 = DeviceRecord {
            auth_version: 1,
            public_key: Some("k".into()),
            token_hash: None,
            ..d.clone()
        };
        assert!(!v1.has_required_credential());
    }

    #[test]
    fn empty_documents_are_the_default() {
        assert_eq!(
            serde_json::to_string(&TimersDoc::default()).unwrap(),
            r#"{"timers":[]}"#
        );
        assert_eq!(
            serde_json::to_string(&ProgramsDoc::default()).unwrap(),
            r#"{"programs":[]}"#
        );
    }
}
