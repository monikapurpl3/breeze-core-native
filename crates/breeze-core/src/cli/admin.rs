//! The admin half of pairing: which devices are enrolled, and revoking one.
//!
//! `devices` and `revoke` exist because the reference has them and its
//! documentation names them. They are LAN-only and key-gated at the server, so
//! the credential they need is the API key — which is why both accept
//! `--config`, reading it from the server's own `config.json` the way the
//! reference does. That is what makes the existing shell aliases keep working:
//!
//!   breeze-core approve --config /etc/breeze-core/config.json <CODE>
//!   breeze-core devices --config /etc/breeze-core/config.json

use super::client::Client;
use super::profile;

/// Where a client subcommand gets its credentials from.
///
/// `--config` names the server's config, which holds the API key. Without it the
/// CLI's own profile is used, which also carries a device token — needed by the
/// routes that are more than admin-only.
#[derive(Debug, Default)]
pub struct ClientOpts {
    pub base_url: Option<String>,
    pub config: Option<String>,
}

pub const DEFAULT_BASE_URL: &str = profile::DEFAULT_BASE_URL;

impl ClientOpts {
    /// Build a client, preferring an explicit `--config` over the profile.
    ///
    /// When both exist the key comes from the file and the token from the
    /// profile: a token is per-machine and cannot be read out of a config, and
    /// the admin routes do not need one anyway.
    pub fn client(&self) -> Result<Client, String> {
        let stored = profile::load();
        let base_url = self
            .base_url
            .clone()
            .or_else(|| stored.as_ref().map(|p| p.base_url.clone()))
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

        if let Some(path) = &self.config {
            let key = api_key_from(path)?;
            let token = stored.map(|p| p.device_token);
            return Ok(Client::new(base_url, key, token));
        }

        match stored {
            Some(profile) => Ok(Client::new(
                base_url,
                profile.api_key,
                Some(profile.device_token),
            )),
            // Said plainly, with both ways out: on the server the config is
            // right there, and off it enrolling is the answer.
            None => Err("no API key: pass --config /etc/breeze-core/config.json, \
                 or run `breeze-core login` to enrol this machine"
                .to_string()),
        }
    }

    /// Like `client`, but enrols this machine if it has never been enrolled.
    ///
    /// For the subcommands that touch routes needing a device token as well as
    /// the key. With a `--config` to read the key from, the enrolment is silent:
    /// it starts, self-approves from the LAN, and polls — so `breeze-core diag`
    /// out of a cron job on the server works the first time without a person.
    pub fn client_enrolling(&self) -> Result<Client, String> {
        if profile::load().is_some() {
            return self.client();
        }
        if let Some(path) = &self.config {
            let base_url = self
                .base_url
                .clone()
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
            let profile = profile::enrol_with(base_url, api_key_from(path)?)?;
            return Ok(Client::from_profile(&profile));
        }
        // No profile and no config: ask, which is the right thing when a person
        // is running this on their laptop for the first time.
        let mut profile = profile::ensure()?;
        if let Some(url) = &self.base_url {
            profile.base_url = url.trim_end_matches('/').to_string();
        }
        Ok(Client::from_profile(&profile))
    }
}

/// The `api_key` out of a config file.
fn api_key_from(path: &str) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{path} is not valid JSON: {e}"))?;
    match value.get("api_key").and_then(|v| v.as_str()) {
        Some(key) if !key.is_empty() => Ok(key.to_string()),
        // A config with no key is a config that has never been paired, and the
        // fix is a different command.
        _ => Err(format!(
            "{path} has no api_key — run `breeze-core pair` first"
        )),
    }
}

/// `breeze-core devices` — what is enrolled.
pub fn devices(options: &ClientOpts) -> Result<(), String> {
    let client = options.client()?;
    let listed = client.get("/api/auth/devices")?;
    let list = listed
        .get("devices")
        .and_then(|d| d.as_array())
        .or_else(|| listed.as_array())
        .ok_or("unexpected response")?;

    if list.is_empty() {
        println!("no devices enrolled");
        return Ok(());
    }
    // Width from the data: labels are user-supplied ("Monika's phone") and a
    // fixed column either truncates them or wastes half the terminal.
    let width = list
        .iter()
        .filter_map(|d| d.get("label")?.as_str().map(str::len))
        .max()
        .unwrap_or(5)
        .max(5);
    println!(
        "{:<width$}  {:<12}  LAST USED",
        "LABEL",
        "TOKEN ID",
        width = width
    );
    for device in list {
        let label = device.get("label").and_then(|v| v.as_str()).unwrap_or("?");
        let id = device
            .get("token_id")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let last = device
            .get("last_used")
            .and_then(|v| v.as_f64())
            .map(stamp)
            .unwrap_or_else(|| "never".into());
        println!("{label:<width$}  {id:<12}  {last}", width = width);
    }
    Ok(())
}

/// `breeze-core revoke <token-id>` — take a device's access away.
pub fn revoke(token_id: &str, options: &ClientOpts) -> Result<(), String> {
    let client = options.client()?;
    client.delete(&format!("/api/auth/devices/{token_id}"))?;
    println!("revoked {token_id}");
    Ok(())
}

/// A unix timestamp as something a person can read, in local time.
///
/// Hand-rolled rather than pulling in a date library for one line: this is the
/// only place in the CLI that formats a time, and `chrono` is already in the
/// binary for the scheduler but exposing it here would widen the dependency for
/// nothing.
fn stamp(seconds: f64) -> String {
    let secs = seconds as i64;
    let days = secs / 86_400;
    let time = secs % 86_400;
    // 1970-01-01 plus `days`, by the civil-from-days algorithm.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!(
        "{year:04}-{m:02}-{d:02} {:02}:{:02}",
        time / 3600,
        (time % 3600) / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_without_a_key_says_what_to_run() {
        let dir = std::env::temp_dir().join("bc-admin-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("empty.json");
        std::fs::write(&path, r#"{"units":[]}"#).unwrap();
        let error = api_key_from(path.to_str().unwrap()).unwrap_err();
        assert!(error.contains("breeze-core pair"), "{error}");
    }

    #[test]
    fn a_key_is_read_out_of_a_config() {
        let dir = std::env::temp_dir().join("bc-admin-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("full.json");
        std::fs::write(&path, r#"{"api_key":"abc123","units":[]}"#).unwrap();
        assert_eq!(api_key_from(path.to_str().unwrap()).unwrap(), "abc123");
    }

    #[test]
    fn timestamps_are_formatted_without_a_date_library() {
        // 2026-08-24T00:00:00Z, and one second before it.
        assert_eq!(stamp(1_787_529_600.0), "2026-08-24 00:00");
        assert_eq!(stamp(1_787_529_599.0), "2026-08-23 23:59");
        // The epoch itself, as the boundary case that catches an off-by-one in
        // the civil-from-days arithmetic.
        assert_eq!(stamp(0.0), "1970-01-01 00:00");
    }
}
