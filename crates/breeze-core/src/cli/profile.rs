//! The CLI's own credentials, and how it gets them.
//!
//! `breeze-core control` is an API client like any other: it needs the API key
//! *and* a per-device token, because control sits behind full auth. So the CLI
//! enrols itself once, exactly as a phone does, and keeps the result in its own
//! profile — separate from the server's stores, which it may not even be able to
//! read.
//!
//! The profile is mode 600 and lives beside the user's other configuration:
//! `$XDG_CONFIG_HOME/breeze-core/cli.json`, or `~/.config/breeze-core/cli.json`,
//! or `%APPDATA%\breeze-core\cli.json`. It holds a device token, which is a
//! credential for someone's heating — so it is written no wider than the one
//! account that made it.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Where a fresh profile points if nobody says otherwise.
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8420";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub base_url: String,
    pub api_key: String,
    /// The token this CLI enrolled for itself.
    pub device_token: String,
    /// Which enrolled device this is, so `breeze-core` can be revoked by name
    /// from the panel without guessing.
    #[serde(default)]
    pub token_id: String,
}

pub fn path() -> PathBuf {
    if let Ok(dir) = std::env::var("BREEZE_CLI_PROFILE") {
        return PathBuf::from(dir);
    }
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".config")))
        .or_else(|_| std::env::var("APPDATA").map(PathBuf::from))
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("breeze-core").join("cli.json")
}

pub fn load() -> Option<Profile> {
    let text = std::fs::read_to_string(path()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(profile: &Profile) -> Result<(), String> {
    let file = path();
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {parent:?}: {e}"))?;
    }
    let text = serde_json::to_string_pretty(profile)
        .map_err(|e| format!("cannot serialise the profile: {e}"))?;
    std::fs::write(&file, text).map_err(|e| format!("cannot write {file:?}: {e}"))?;
    restrict(&file);
    Ok(())
}

/// 600. This file holds a device token.
#[cfg(unix)]
fn restrict(file: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_file: &std::path::Path) {
    // Windows inherits the user's profile ACL, which is already per-user.
}

/// The profile, enrolling first if there is not one yet.
///
/// Interactive on purpose. A control command that silently failed with "401"
/// would send somebody hunting through the server's logs for a problem that is
/// really "this machine has never introduced itself".
pub fn ensure() -> Result<Profile, String> {
    if let Some(profile) = load() {
        return Ok(profile);
    }
    println!("No profile yet at {}.", path().display());
    println!("This will enrol this machine as a device, once.\n");
    enrol(None)
}

/// Run the pairing handshake and save the result.
///
/// `base_url` overrides the prompt, for `breeze-core login --base-url …`.
pub fn enrol(base_url: Option<String>) -> Result<Profile, String> {
    let base_url = match base_url {
        Some(url) => url,
        None => {
            let entered = prompt(&format!("Server URL [{DEFAULT_BASE_URL}]: "))?;
            if entered.is_empty() {
                DEFAULT_BASE_URL.to_string()
            } else {
                entered
            }
        }
    };
    let base_url = base_url.trim_end_matches('/').to_string();

    // Offered because the person doing this is usually sitting on the server,
    // where the key is already in a file they can read. Typing a 32-character
    // key by hand invites a typo that presents as a 401.
    let api_key = match key_from_config() {
        Some(key) => {
            println!("Using the api_key from the server's config.json.");
            key
        }
        None => {
            let key = prompt("API key: ")?;
            if key.is_empty() {
                return Err("an API key is required".into());
            }
            key
        }
    };

    enrol_with(base_url, api_key)
}

/// The handshake itself, with the URL and key already known.
///
/// Split out so a scripted run can enrol without asking anybody anything: on
/// the server the key is readable from `config.json` and approval is allowed
/// from the LAN, so nothing is left to prompt for — and prompting there would
/// hang whatever called it.
pub fn enrol_with(base_url: String, api_key: String) -> Result<Profile, String> {
    let base_url = base_url.trim_end_matches('/').to_string();
    let label = format!(
        "breeze-core CLI on {}",
        hostname().unwrap_or_else(|| "this machine".into())
    );
    println!("\nAsking {base_url} to enrol \"{label}\"...");

    let client = crate::cli::client::Client::new(base_url.clone(), api_key.clone(), None);
    let started = client.post_json(
        "/api/auth/enroll/start",
        &serde_json::json!({ "label": label, "auth_version": 1 }),
    )?;
    let session_id = started["session_id"]
        .as_str()
        .ok_or("the server did not return a session id")?
        .to_string();
    let user_code = started["user_code"]
        .as_str()
        .ok_or("the server did not return a code")?
        .to_string();

    // Approval is LAN-only. When the CLI is already on the LAN — which it is,
    // if it is running on the server — it can approve itself, and there is no
    // reason to make somebody walk to a browser to rubber-stamp it.
    println!("Enrolment code: {user_code}");
    match client.post_json(
        "/api/auth/enroll/approve",
        &serde_json::json!({ "code": user_code }),
    ) {
        Ok(_) => println!("Approved from here (this machine is on the local network)."),
        Err(e) => {
            println!("Could not approve from here: {e}");
            println!("\nApprove it from the panel, or on the server run:");
            println!("  breeze-core approve {user_code}");
            print!("Then press enter to continue... ");
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            let _ = std::io::stdin().lock().read_line(&mut line);
        }
    }

    let polled = client.post_json(
        "/api/auth/enroll/poll",
        &serde_json::json!({ "session_id": session_id }),
    )?;
    if polled["status"].as_str() != Some("approved") {
        return Err(format!(
            "the enrolment was not approved (status: {})",
            polled["status"].as_str().unwrap_or("unknown")
        ));
    }
    let device_token = polled["device_token"]
        .as_str()
        .ok_or("the server approved the enrolment but issued no token")?
        .to_string();

    let profile = Profile {
        base_url,
        api_key,
        device_token,
        token_id: polled["token_id"].as_str().unwrap_or_default().to_string(),
    };
    save(&profile)?;
    println!("\nSaved to {}.", path().display());
    Ok(profile)
}

/// The API key out of the server's own config, when this is running on the
/// server and the file is readable.
fn key_from_config() -> Option<String> {
    let path = std::env::var("AC_CONFIG").unwrap_or_else(|_| {
        let dir = std::env::var("AC_CONFIG_DIR").unwrap_or_else(|_| "/etc/breeze-core".into());
        format!("{dir}/config.json")
    });
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let key = value.get("api_key")?.as_str()?;
    (!key.is_empty()).then(|| key.to_string())
}

fn hostname() -> Option<String> {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
}

fn prompt(message: &str) -> Result<String, String> {
    print!("{message}");
    std::io::stdout()
        .flush()
        .map_err(|e| format!("cannot write to the terminal: {e}"))?;
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| format!("cannot read from the terminal: {e}"))?;
    Ok(line.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_path_honours_an_explicit_override() {
        // Which is what lets the tests, and anyone with two servers, avoid the
        // single shared location.
        std::env::set_var("BREEZE_CLI_PROFILE", "/tmp/somewhere/cli.json");
        assert_eq!(path(), PathBuf::from("/tmp/somewhere/cli.json"));
        std::env::remove_var("BREEZE_CLI_PROFILE");
    }

    #[test]
    fn a_profile_round_trips_through_json() {
        let profile = Profile {
            base_url: "http://127.0.0.1:8420".into(),
            api_key: "0123456789abcdef".into(),
            device_token: "aa".repeat(32),
            token_id: "abc123".into(),
        };
        let text = serde_json::to_string(&profile).unwrap();
        let back: Profile = serde_json::from_str(&text).unwrap();
        assert_eq!(back.base_url, profile.base_url);
        assert_eq!(back.device_token, profile.device_token);
        assert_eq!(back.token_id, "abc123");
    }

    #[test]
    fn an_older_profile_without_a_token_id_still_loads() {
        // Forward compatibility in the direction that matters: a profile written
        // before token_id existed must not force a re-enrolment.
        let back: Profile =
            serde_json::from_str(r#"{"base_url":"http://x","api_key":"k","device_token":"t"}"#)
                .unwrap();
        assert_eq!(back.token_id, "");
    }
}
