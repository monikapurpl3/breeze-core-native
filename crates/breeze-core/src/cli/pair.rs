//! `breeze-core pair` — find the units on the LAN and write `config.json`.
//!
//! **The one subcommand that is not an API client.** On a fresh install there is
//! no `config.json`, so there is no API key, so there is no way to reach the
//! API — pairing has to work before the service does, which means touching the
//! store directly. Everything else here goes over HTTP on purpose.
//!
//! This is also the verb the package's own postinstall tells people to run, and
//! the reference has had it since 2.x. It keeps that contract exactly: the same
//! flags (`--ip`, `--out`, `--no-prompt`), the same merge-by-device-id
//! behaviour, the same naming prompt, and the same redacted summary. A user
//! following the existing documentation gets what it describes.
//!
//! Re-running it is safe and expected: units are matched by the id they report,
//! names already chosen are kept unless a new one is typed, and credentials
//! already stored are never cleared. So the answer to "one unit was powered off
//! when I did this" is simply to run it again.

use std::io::{BufRead, Write};
use std::time::Duration;

use breeze_proto::discover::Version;
use breeze_store::models::{AppConfig, UnitConfig};
use breeze_store::{Mode, StoreError};

/// Long enough for a unit on wifi to answer, short enough that a person does
/// not assume it has hung. The same window the server's own scan route uses.
const LISTEN: Duration = Duration::from_secs(3);

/// A V3 token is 128 hex characters and its key 64 — the same lengths
/// `POST /api/units` validates, checked here too so a typo is refused at the
/// prompt rather than stored and discovered later by a unit that will not talk.
const TOKEN_HEX_LEN: usize = 128;
const KEY_HEX_LEN: usize = 64;

pub struct Options {
    /// Skip the sweep and probe one address.
    pub ip: Option<String>,
    /// Where to write. Defaults to the same path the server reads.
    pub out: Option<String>,
    /// Ask for names (and for V3 credentials). Off for scripts.
    pub prompt: bool,
}

/// The config path the server would read, resolved the same way it resolves it.
pub fn default_config_path() -> String {
    if let Ok(path) = std::env::var("AC_CONFIG") {
        return path;
    }
    let dir = std::env::var("AC_CONFIG_DIR").unwrap_or_else(|_| "/etc/breeze-core".into());
    format!("{dir}/config.json")
}

pub fn run(options: Options) -> Result<(), String> {
    let path = options.out.unwrap_or_else(default_config_path);

    // A file that exists but is not JSON is a person's configuration, so it is
    // reported rather than silently replaced -- but it does not stop the run,
    // because the thing they are trying to do is recover from exactly that.
    let mut config: AppConfig = match breeze_store::load(&path) {
        Ok(config) => config,
        Err(StoreError::Parse { .. }) => {
            println!("Warning: {path} exists but isn't valid JSON, starting fresh.");
            AppConfig::default()
        }
        Err(e) => return Err(format!("cannot read {path}: {e}")),
    };

    let new_key = config.api_key.is_none();
    if new_key {
        config.api_key = Some(
            breeze_auth::enroll::random_api_key()
                .map_err(|e| format!("cannot mint an API key: {e}"))?,
        );
    }

    let report = match &options.ip {
        // /32 is a sweep of one host: the same unicast probe, no broadcast, so
        // the reply can only have come from the address that was asked about.
        Some(ip) => breeze_device::scan_subnet(&format!("{}/32", ip.trim()), LISTEN),
        None => breeze_device::scan(LISTEN),
    }
    .map_err(|e| format!("discovery failed: {e}"))?;

    if report.found.is_empty() {
        // The reference's wording, because it is the answer to the question the
        // silence raises, and people have read it before.
        println!(
            "No devices found. Make sure all the units are powered on and on the \
             same subnet as this machine, or pass --ip for one you already know \
             (check your router's DHCP leases). You can also just re-run this \
             later for the ones that were off — it won't touch the units you've \
             already paired."
        );
        // Anything that answered but could not be decoded is a different
        // problem, and saying nothing about it would send someone hunting the
        // network for a unit that is right there.
        for (ip, why) in &report.unparseable {
            println!("  something at {ip} answered but could not be read: {why}");
        }
        return Ok(());
    }

    let mut found_count = 0usize;
    for (index, device) in report.found.iter().enumerate() {
        let Ok(id) = i64::try_from(device.id) else {
            println!(
                "Warning: the unit at {} reported an unusable id, skipping.",
                device.ip
            );
            continue;
        };
        let existing = config.find_unit(&id.to_string()).cloned();

        let mut name = existing
            .as_ref()
            .map(|u| u.name.clone())
            .unwrap_or_else(|| format!("AC {}", index + 1));
        if options.prompt {
            if let Some(typed) = ask(&format!("Name for unit at {} [{name}]: ", device.ip))? {
                if !typed.is_empty() {
                    name = typed;
                }
            }
        }

        // Credentials: keep whatever is stored, and only ask when there are
        // none. Discovery does not produce them -- Midea's cloud stopped
        // issuing them to anyone but the account a unit is registered to -- so
        // for a new V3 unit this prompt is the realistic path.
        let (mut token, mut key) = match &existing {
            Some(unit) => (unit.token.clone(), unit.key.clone()),
            None => (None, None),
        };
        if device.version == Version::V3 && token.is_none() {
            if options.prompt {
                println!(
                    "  {} is a V3 unit: it needs a token and key before it can be driven.",
                    device.ip
                );
                println!("  Paste them if you have them, or press enter to add the unit anyway.");
                let typed_token = ask("  token (128 hex chars): ")?.unwrap_or_default();
                if !typed_token.is_empty() {
                    let typed_key = ask("  key   (64 hex chars): ")?.unwrap_or_default();
                    match validate_pair(&typed_token, &typed_key) {
                        Ok(()) => {
                            token = Some(typed_token);
                            key = Some(typed_key);
                        }
                        // Refused, not stored: half a credential authenticates
                        // nothing, and a malformed one fails later with nothing
                        // to point at.
                        Err(e) => println!("  not stored: {e}"),
                    }
                }
            } else {
                println!(
                    "Note: {} is a V3 unit with no stored credentials — add its token and key \
                     with `breeze-core pair` interactively, or POST /api/units.",
                    device.ip
                );
            }
        }

        config.add_or_update_unit(UnitConfig {
            name,
            ip: device.ip.to_string(),
            port: device.port,
            id,
            token,
            key,
        });
        found_count += 1;
    }

    // 0640: owner-writable, readable by the service group, so an admin in that
    // group can run the CLI without sudo. The other stores stay 0600.
    breeze_store::save(&path, &config, Mode::AdminReadable)
        .map_err(|e| format!("cannot write {path}: {e}"))?;

    // Redacted summary. Never the api_key and never a unit's token or key:
    // terminal scrollback and CI logs outlive the moment.
    let summary: Vec<serde_json::Value> = config
        .units
        .iter()
        .map(|u| {
            serde_json::json!({
                "id": u.id.to_string(),
                "name": u.name,
                "ip": u.ip,
                "port": u.port,
                "has_v3_credentials": u.token.is_some() && u.key.is_some(),
            })
        })
        .collect();
    println!(
        "\nThis run found {found_count} unit(s). Config now has {} total at {path}:",
        config.units.len()
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&summary).unwrap_or_else(|_| "[]".into())
    );

    if new_key {
        println!(
            "\nGenerated a new API key and stored it in {path} (mode 640). The web \
             panel and the app ask for it the first time each device connects. Read \
             it from that file when you need it — anyone who has it can control \
             these units over the LAN, so keep it off anywhere public."
        );
    }
    if config
        .units
        .iter()
        .any(|u| u.token.is_some() && u.key.is_some())
    {
        println!(
            "\nAt least one of these is a V3 unit. Keep its token and key somewhere \
             off this box as well: Midea's cloud will not hand them out again."
        );
    }
    Ok(())
}

/// Both or neither, and both the right shape.
fn validate_pair(token: &str, key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("a token needs its key".into());
    }
    if !is_hex(token, TOKEN_HEX_LEN) {
        return Err(format!("the token must be {TOKEN_HEX_LEN} hex characters"));
    }
    if !is_hex(key, KEY_HEX_LEN) {
        return Err(format!("the key must be {KEY_HEX_LEN} hex characters"));
    }
    Ok(())
}

fn is_hex(value: &str, len: usize) -> bool {
    value.len() == len && value.chars().all(|c| c.is_ascii_hexdigit())
}

/// A prompt, returning `None` at end of input so a piped-in script that runs out
/// of answers stops rather than looping on an empty line.
fn ask(message: &str) -> Result<Option<String>, String> {
    print!("{message}");
    std::io::stdout()
        .flush()
        .map_err(|e| format!("cannot write to the terminal: {e}"))?;
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) => Ok(None),
        Ok(_) => Ok(Some(line.trim().to_string())),
        Err(e) => Err(format!("cannot read from the terminal: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_path_follows_the_same_environment_the_server_reads() {
        std::env::set_var("AC_CONFIG", "/somewhere/config.json");
        assert_eq!(default_config_path(), "/somewhere/config.json");
        std::env::remove_var("AC_CONFIG");

        std::env::set_var("AC_CONFIG_DIR", "/opt/bc");
        assert_eq!(default_config_path(), "/opt/bc/config.json");
        std::env::remove_var("AC_CONFIG_DIR");
    }

    #[test]
    fn a_credential_pair_is_validated_before_it_is_stored() {
        let token = "a".repeat(TOKEN_HEX_LEN);
        let key = "b".repeat(KEY_HEX_LEN);
        assert!(validate_pair(&token, &key).is_ok());
        // A token with no key is not half a credential, it is none.
        assert!(validate_pair(&token, "").is_err());
        assert!(validate_pair("abc", &key).is_err());
        // Right length, wrong alphabet -- the case a length check alone misses.
        assert!(validate_pair(&"z".repeat(TOKEN_HEX_LEN), &key).is_err());
    }
}
