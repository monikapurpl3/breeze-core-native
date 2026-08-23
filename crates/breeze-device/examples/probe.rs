//! Read every configured unit through this crate and print its state.
//!
//! The Phase 2 acceptance test: run this on the server and diff its output
//! against `breeze-core diag`, unit for unit. It reads `config.json` itself, so
//! the V3 credentials never move off the host and are never printed.
//!
//! ```text
//! cargo zigbuild --release --target x86_64-unknown-linux-musl --example probe
//! ```

use breeze_device::{DeviceManager, UnitConfig};
use std::net::IpAddr;

fn hex_to_bytes(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/etc/breeze-core/config.json".into());
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            std::process::exit(1);
        }
    };
    let cfg: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{path} is not valid JSON: {e}");
            std::process::exit(1);
        }
    };

    let mut configs = Vec::new();
    for u in cfg["units"].as_array().cloned().unwrap_or_default() {
        let ip: Option<IpAddr> = u["ip"].as_str().and_then(|s| s.parse().ok());
        let id = u["id"]
            .as_u64()
            .or_else(|| u["id"].as_str().and_then(|s| s.parse().ok()));
        let (Some(ip), Some(id)) = (ip, id) else {
            eprintln!("skipping a unit with no usable ip/id");
            continue;
        };
        let key = u["key"]
            .as_str()
            .and_then(hex_to_bytes)
            .and_then(|v| <[u8; 32]>::try_from(v.as_slice()).ok());
        configs.push(UnitConfig {
            id,
            name: u["name"].as_str().unwrap_or("(unnamed)").to_string(),
            ip,
            port: u["port"].as_u64().unwrap_or(6444) as u16,
            token: u["token"].as_str().and_then(hex_to_bytes),
            key,
        });
    }

    println!("{} unit(s) in {path}\n", configs.len());
    let manager = DeviceManager::new(configs);

    let mut ok = 0;
    let mut failed = 0;
    for id in manager.known_units() {
        let name = manager
            .with_unit(id, |d| Ok(d.config().name.clone()))
            .unwrap_or_else(|_| "?".into());
        let started = std::time::Instant::now();
        match manager.with_unit(id, |d| d.refresh()) {
            Ok(s) => {
                ok += 1;
                let mode = s.mode.map(|m| m.as_str()).unwrap_or("?");
                let swing = s.swing_mode.map(|m| m.as_str()).unwrap_or("?");
                println!("{name}  (id={id})");
                println!(
                    "  online=true power={} mode={mode} target={:.1}C",
                    s.power_on, s.target_temperature
                );
                println!(
                    "  indoor={} outdoor={} fan={} swing={swing} eco={} turbo={}",
                    s.indoor_temperature
                        .map(|t| format!("{t:.1}C"))
                        .unwrap_or_else(|| "none".into()),
                    s.outdoor_temperature
                        .map(|t| format!("{t:.1}C"))
                        .unwrap_or_else(|| "none".into()),
                    s.fan_speed.0,
                    s.eco,
                    s.turbo
                );
                println!("  ({} ms)\n", started.elapsed().as_millis());
            }
            Err(e) => {
                failed += 1;
                println!(
                    "{name}  (id={id})\n  FAILED: {e}  (retryable={})\n",
                    e.is_retryable()
                );
            }
        }
    }
    println!("{ok} unit(s) read, {failed} failed");
    if ok == 0 {
        std::process::exit(1);
    }
}
