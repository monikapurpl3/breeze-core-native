//! `GET /api/system` — the whole deployment in one response.
//!
//! Backs the panel's and the app's "Nerd" screen. One endpoint rather than a
//! dozen because the screen wants the whole picture at once, and a phone on a
//! LAN pays a round-trip per call.
//!
//! Behind the same bar as controlling a unit — API key *and* a device
//! credential. It reports OS, paths, versions and the enrolled device list,
//! which is both what an attacker would want and what the owner wants on a
//! diagnostics screen. Deliberately **not** LAN-gated: the person most likely to
//! need it is the one away from home wondering why their AC will not answer.
//!
//! # No secrets, ever
//!
//! No API key. No device token hashes and no device public keys. No per-unit V3
//! `token`/`key` — `has_v3_credentials` is a boolean. There is a test at the
//! bottom that renders the whole response and fails if any of those appear, and
//! it is the reason this module builds its JSON by hand instead of serialising
//! the stores.
//!
//! # Host facts are best-effort
//!
//! Everything about the machine comes from files that may not exist: this binary
//! runs on Linux, three BSDs, Windows and a router. A fact that cannot be read
//! is `null` rather than absent, and never an error — a diagnostics screen that
//! 500s because `/proc` is missing is worse than one with a gap.

use std::path::Path;

use crate::state::AppState;

/// Existence, size, mode and mtime of one store.
///
/// The mode is a string like `"0o600"` because it is the point: a config that is
/// world-readable is a finding, and this is the screen where somebody notices.
fn file_facts(path: &Path) -> serde_json::Value {
    match std::fs::metadata(path) {
        Ok(meta) => {
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64());
            serde_json::json!({
                "path": path.display().to_string(),
                "exists": true,
                "size_bytes": meta.len(),
                "mode": mode_string(&meta),
                "modified_at": modified,
            })
        }
        Err(_) => serde_json::json!({
            "path": path.display().to_string(),
            "exists": false,
        }),
    }
}

#[cfg(unix)]
fn mode_string(meta: &std::fs::Metadata) -> serde_json::Value {
    use std::os::unix::fs::MetadataExt;
    serde_json::Value::String(format!("0o{:o}", meta.mode() & 0o777))
}

#[cfg(not(unix))]
fn mode_string(_meta: &std::fs::Metadata) -> serde_json::Value {
    // Windows has no POSIX mode. Reporting a fabricated 0o600 would be worse
    // than admitting there is nothing to report.
    serde_json::Value::Null
}

/// This process: pid, and resident size where the kernel will say.
fn process_facts() -> serde_json::Value {
    let mut facts = serde_json::json!({ "pid": std::process::id() });
    if let Some(rss) = resident_bytes() {
        facts["rss_bytes"] = serde_json::json!(rss);
    }
    facts
}

/// Resident set size, from `/proc/self/statm`. Linux only, and free — no
/// dependency for one number.
fn resident_bytes() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = text.split_whitespace().nth(1)?.parse().ok()?;
    // 4 KiB is the page size everywhere this runs; the alternative is a libc
    // call for a number that is a diagnostic hint.
    Some(pages * 4096)
}

/// Operating system name and release.
fn os_facts() -> serde_json::Value {
    // `/etc/os-release` on Linux gives the distribution, which is what someone
    // debugging actually wants ("Fedora 41", not "linux").
    let mut pretty = None;
    let mut id = None;
    let mut version = None;
    if let Ok(text) = std::fs::read_to_string("/etc/os-release") {
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("PRETTY_NAME=") {
                pretty = Some(v.trim_matches('"').to_string());
            } else if let Some(v) = line.strip_prefix("VERSION_ID=") {
                version = Some(v.trim_matches('"').to_string());
            } else if let Some(v) = line.strip_prefix("ID=") {
                id = Some(v.trim_matches('"').to_string());
            }
        }
    }
    // The reference's field NAMES, because the panel reads them by name: it
    // shows blanks for `distro_id` if the server calls it `distribution_id`.
    // `family`/`platform`/`arch` stay as additions -- they cost nothing and a
    // cross-platform build has more of them worth reporting.
    serde_json::json!({
        "system": if std::env::consts::OS == "linux" { "Linux" } else { std::env::consts::OS },
        "hostname": hostname(),
        "pretty_name": pretty.clone(),
        "distro_id": id.clone(),
        "distro_version": version,
        "kernel": kernel_release(),
        "kernel_version": kernel_version(),
        "libc": libc_description(),
        "family": std::env::consts::FAMILY,
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "distribution": pretty,
        "distribution_id": id,
    })
}

/// The kernel's build string -- `uname -v`, not `uname -r`.
///
/// Reported separately from the release because they answer different
/// questions: the release says which kernel, this says which build of it, and a
/// distribution kernel's build date is often the fastest way to tell whether a
/// box has actually rebooted into the update it downloaded.
/// The running executable's modification time, as a unix timestamp.
fn installed_at() -> Option<f64> {
    std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
}

fn kernel_version() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/version")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Which libc this binary is actually using.
///
/// Decided at compile time, and worth reporting for the reason the whole
/// project exists: a static musl build behaves differently from a glibc one,
/// and "which libc" is the first question when a binary refuses to run.
fn libc_description() -> &'static str {
    if cfg!(target_env = "musl") {
        "musl (static)"
    } else if cfg!(target_env = "gnu") {
        "glibc"
    } else if cfg!(target_env = "msvc") {
        "msvc"
    } else {
        "unknown"
    }
}

fn kernel_release() -> Option<String> {
    // /proc/sys/kernel/osrelease on Linux; nothing portable elsewhere, and a
    // uname binding is not worth a dependency here.
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|s| s.trim().to_string())
}

/// Which init system is running, since it decides how the service is managed.
///
/// Marker paths first and `/proc/1/comm` only as a last resort -- which is the
/// order the reference uses, and the order matters more than it looks:
///
///   * `/proc/1/comm` is **unreadable under `ProtectProc=invisible`**, and that
///     directive is in the reference's own hardening drop-in. Having pid 1 as
///     the *only* probe meant a correctly hardened deployment reported a null
///     init system and a blank row in the panel's diagnostics. Found by diffing
///     this endpoint against a live 3.2.0 running under that same drop-in --
///     which answered "systemd" because it looks at /run/systemd/system, a path
///     the sandbox does not touch.
///   * The BSDs have no /proc at all by default, so pid 1 answered nothing
///     there either, on every platform where the answer is simply "rc.d".
fn init_facts() -> serde_json::Value {
    let facts = |name: &str, detail: &str| {
        serde_json::json!({ "name": name, "detail": detail })
    };

    #[cfg(target_os = "macos")]
    return facts("launchd", "launchctl print system/breeze-core");
    #[cfg(target_os = "windows")]
    return facts("windows-sc", "Windows Service Control Manager");
    #[cfg(any(
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    return facts("rc.d", "service breeze_core status");

    #[cfg(all(
        unix,
        not(target_os = "macos"),
        not(target_os = "freebsd"),
        not(target_os = "openbsd"),
        not(target_os = "netbsd"),
        not(target_os = "dragonfly")
    ))]
    {
        // systemd's own documented check: the directory exists iff it booted us.
        if Path::new("/run/systemd/system").is_dir() {
            return facts("systemd", "systemctl status breeze-core");
        }
        if Path::new("/run/openrc").exists() {
            return facts("openrc", "rc-service breeze-core status");
        }
        // procd is OpenWrt, where the init script is the interface.
        if Path::new("/sbin/procd").exists() {
            return facts("procd", "/etc/init.d/breeze-core status");
        }
        if Path::new("/run/runit").is_dir() || Path::new("/etc/runit").is_dir() {
            return facts("runit", "sv status breeze-core");
        }
        if Path::new("/run/s6-rc").exists() {
            return facts("s6", "s6-rc -a list");
        }
        // Last resort, and only that: in a container the app itself may be pid 1
        // and no service manager is involved at all.
        let comm = std::fs::read_to_string("/proc/1/comm")
            .ok()
            .map(|s| s.trim().to_string());
        return match comm {
            Some(name) if !name.is_empty() => {
                serde_json::json!({ "name": name, "detail": "from /proc/1/comm" })
            }
            // "unknown" rather than null, so a client rendering this shows a
            // word instead of a blank.
            _ => facts("unknown", "no init system could be identified"),
        };
    }
}

fn cpu_facts() -> serde_json::Value {
    let mut model = None;
    let mut cores = 0u32;
    if let Ok(text) = std::fs::read_to_string("/proc/cpuinfo") {
        for line in text.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim();
                if key == "model name" && model.is_none() {
                    model = Some(value.trim().to_string());
                }
                if key == "processor" {
                    cores += 1;
                }
            }
        }
    }
    serde_json::json!({
        "model": model,
        // `cores`, not `logical_cores`: the reference's name, and the panel
        // reads it by that name. `available_parallelism` stays as an addition
        // -- it is what the scheduler actually gets, which can be lower than
        // the core count under a cgroup CPU quota.
        "cores": if cores == 0 { serde_json::Value::Null } else { serde_json::json!(cores) },
        "arch": std::env::consts::ARCH,
        "endianness": if cfg!(target_endian = "little") { "little" } else { "big" },
        "available_parallelism": std::thread::available_parallelism()
            .map(|n| n.get())
            .ok(),
    })
}

fn machine_uptime_seconds() -> Option<f64> {
    let text = std::fs::read_to_string("/proc/uptime").ok()?;
    text.split_whitespace().next()?.parse().ok()
}

/// What this build is made of.
///
/// The reference lists Python package versions. There is no runtime here to
/// report, so this reports the crates instead — the honest equivalent, and the
/// thing somebody comparing two builds actually needs.
fn components() -> serde_json::Value {
    serde_json::json!({
        "runtime": "none (statically linked)",
        "language": "rust",
        // Named without a version: pinning one here would drift from Cargo.lock
        // the first time it moved, and a wrong version is worse than none.
        "http": "tiny_http",
        "tls": "none (plain HTTP; terminate TLS in front)",
        "panel_files": crate::panel::asset_count(),
        "panel_bytes": crate::panel::embedded_bytes(),
    })
}

fn network_facts(state: &AppState) -> serde_json::Value {
    serde_json::json!({
        "hostname": hostname(),
        "local_addresses": breeze_device::scan::local_ipv4().map(|ip| vec![ip.to_string()]),
        "bind": state.settings.bind.clone(),
        // Split as well as combined: the reference reports the two halves, and
        // "which address did it actually bind" is the question behind most
        // "I cannot reach it" reports.
        "bind_host": state.settings.bind.rsplit_once(':').map(|(h, _)| h.to_string()),
        "bind_port": state.settings.bind.rsplit_once(':').map(|(_, p)| p.to_string()),
    })
}

fn hostname() -> Option<String> {
    // /etc/hostname is the portable-enough answer; a libc call for one string
    // would mean a dependency and a per-platform shim.
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
}

fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// The whole snapshot.
pub fn snapshot(state: &AppState, connection: serde_json::Value) -> serde_json::Value {
    let started_at = state
        .started_at
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let now = now_seconds();

    let offset = chrono::Local::now().offset().local_minus_utc();

    let mut snap = serde_json::json!({
        "server": {
            "name": "Breeze Core",
            "version": state.version,
            "commit": crate::build_commit(),
            "started_at": started_at,
            // When this build landed on the machine, from the executable's own
            // mtime. The reference reports the same fact about its package
            // directory; for one static binary the binary IS the install, and
            // "how long has this version been here" is worth a row on a
            // diagnostics screen -- it distinguishes "upgraded and restarted"
            // from "restarted".
            "installed_at": installed_at(),
            "uptime_seconds": ((now - started_at) * 10.0).round() / 10.0,
            "timezone": chrono::Local::now().format("%Z").to_string(),
            "utc_offset_seconds": offset,
            "server_time": now,
            "features": crate::server::features(),
            "auth_versions": crate::server::auth_versions(),
        },
        "os": os_facts(),
        "init": init_facts(),
        "cpu": cpu_facts(),
        "machine_uptime_seconds": machine_uptime_seconds(),
        "components": components(),
        "network": network_facts(state),
        "process": process_facts(),
        "connection": connection,
        "paths": {
            "config": state.settings.config_path.display().to_string(),
            "devices": state.settings.devices_path.display().to_string(),
            "programs": state.settings.programs_path.display().to_string(),
            "timers": state.settings.timers_path.display().to_string(),
            // Where the executable itself lives. The reference reports the
            // Python package directory here; the equivalent fact for one static
            // binary is the binary.
            "package": std::env::current_exe()
                .ok()
                .map(|p| p.display().to_string()),
        },
        "settings": {
            "security_headers": state.settings.security_headers,
            "behind_proxy": state.settings.behind_proxy,
            "min_auth_version": state.settings.min_auth_version,
            "worker_threads": state.settings.worker_threads,
            "scheduler_tick_seconds": state.settings.sched_tick_seconds,
            "stream_tick_seconds": state.settings.stream_tick_seconds,
            "timer_tick_seconds": state.settings.timer_tick_seconds,
            "history_size": state.settings.history_size,
            "compression": compression_setting(),
            // Reported even though they are no longer configurable.
            //
            // "Not settable" is not the same as "not a fact": these are still
            // true of the running server, the panel displays them by name, and
            // omitting them left blank rows where the reference showed values.
            // The numbers are the constants 4.x fixed them at.
            "code_ttl_seconds": breeze_auth::enroll::CODE_TTL_SECONDS,
            "token_ttl_days": breeze_auth::enroll::TOKEN_TTL_DAYS,
            "auth_skew_seconds": breeze_auth::signing::DEFAULT_SKEW_SECONDS,
            // Unconditional in 4.x, which is why there is no setting for it.
            "enrollment_lan_only": true,
            // No OpenAPI schema exists to expose, so this can only ever be false.
            "docs_enabled": false,
            // Host filtering belongs to the reverse proxy; the server does none.
            "trusted_hosts": serde_json::Value::Null,
        },
        "storage": {
            "config": file_facts(&state.settings.config_path),
            "devices": file_facts(&state.settings.devices_path),
            "programs": file_facts(&state.settings.programs_path),
            "timers": file_facts(&state.settings.timers_path),
        },
    });

    snap["units"] = units_facts(state);
    snap["devices"] = match state.devices.read() {
        Ok(devices) => devices_facts(&devices, now),
        Err(_) => serde_json::json!([]),
    };
    snap["programs"] = programs_facts(state);
    snap["scheduler"] = scheduler_facts(state);
    snap["stream"] = serde_json::json!({
        "subscribers": state.stream.subscriber_count(),
        "tick_seconds": state.stream.tick_seconds(),
    });
    snap
}

fn compression_setting() -> serde_json::Value {
    serde_json::json!(crate::respond::COMPRESSION_ENABLED)
}

/// Per-unit facts, from configuration and cache only.
///
/// No LAN round-trip: a diagnostics screen must not cost 700 ms per unit while
/// somebody waits. `connected` says whether a session is already open, and a
/// client wanting more asks `/api/units/{id}/capabilities` per unit.
fn units_facts(state: &AppState) -> serde_json::Value {
    let config = match state.config.read() {
        Ok(c) => c,
        Err(_) => return serde_json::json!([]),
    };
    let units: Vec<serde_json::Value> = config
        .units
        .iter()
        .map(|unit| {
            let id = unit.id.to_string();
            let connected = u64::try_from(unit.id)
                .ok()
                .map(|n| state.manager.is_connected(n))
                .unwrap_or(false);
            serde_json::json!({
                "id": id,
                "name": unit.name,
                "ip": unit.ip,
                "port": unit.port,
                // A boolean, never the credential itself.
                "has_v3_credentials": unit.token.is_some() && unit.key.is_some(),
                "connected": connected,
                // Distinct from `connected`: a session can be open to a unit
                // that has stopped answering, and the reference reports both.
                "online": u64::try_from(unit.id)
                    .ok()
                    .map(|n| state.manager.is_online(n))
                    .unwrap_or(false),
                // Whatever was cached by an earlier probe, and null when
                // nothing has probed yet -- never a fresh LAN round-trip, which
                // would cost 700 ms per unit on a diagnostics screen.
                "capabilities": u64::try_from(unit.id)
                    .ok()
                    .and_then(|n| state.manager.cached_capabilities(n))
                    // The same renderer the dedicated endpoint uses, so the two
                    // can never disagree about what a capability looks like.
                    .map(|caps| crate::capabilities::view(&id, &caps))
                    .unwrap_or(serde_json::Value::Null),
                "samples": state.history.samples(&id).len(),
                "last_seen": state.history.latest(&id).map(|s| s.t),
            })
        })
        .collect();
    serde_json::json!(units)
}

/// Enrolled devices: identity and lifecycle, never a credential.
///
/// Takes the document rather than the state so the leak test below can call it
/// directly -- the one thing in this file that must never regress.
fn devices_facts(devices: &breeze_store::DevicesDoc, now: f64) -> serde_json::Value {
    let list: Vec<serde_json::Value> = devices
        .devices
        .iter()
        .map(|d| {
            serde_json::json!({
                "token_id": d.token_id,
                "label": d.label,
                "auth_version": d.auth_version,
                "created_at": d.created_at,
                "last_used": d.last_used,
                "expires_at": d.expires_at,
                "expires_in_seconds": d.expires_at.map(|e| e - now),
                "expired": d.expires_at.is_some_and(|e| e <= now),
            })
        })
        .collect();
    serde_json::json!(list)
}

fn programs_facts(state: &AppState) -> serde_json::Value {
    let programs = match state.programs.read() {
        Ok(p) => p,
        Err(_) => return serde_json::json!({ "total": 0, "by_kind": {} }),
    };
    let mut by_kind = serde_json::Map::new();
    for program in &programs.programs {
        let entry = by_kind
            .entry(program.kind.clone())
            .or_insert(serde_json::json!(0));
        *entry = serde_json::json!(entry.as_u64().unwrap_or(0) + 1);
    }
    serde_json::json!({ "total": programs.programs.len(), "by_kind": by_kind })
}

fn scheduler_facts(state: &AppState) -> serde_json::Value {
    match state.scheduler.lock() {
        Ok(s) => serde_json::json!({
            "running": s.running,
            "tick_seconds": crate::program_routes::tick_seconds(state.settings.sched_tick_seconds),
            "runs": s.runs,
            "errors": s.errors,
            "last_run": s.last_run,
        }),
        Err(_) => serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_facts_report_a_missing_file_rather_than_failing() {
        let facts = file_facts(Path::new("/definitely/not/here.json"));
        assert_eq!(facts["exists"], false);
        assert!(facts["path"].is_string());
        // No size or mode invented for a file that is not there.
        assert!(facts.get("size_bytes").is_none());
    }

    #[test]
    fn file_facts_report_a_real_files_size() {
        // Cargo.toml is beside this crate wherever the tests run.
        let facts = file_facts(Path::new("Cargo.toml"));
        if facts["exists"] == true {
            assert!(facts["size_bytes"].as_u64().unwrap_or(0) > 0);
            assert!(facts["modified_at"].is_number());
        }
    }

    #[test]
    fn the_process_always_reports_a_pid() {
        let facts = process_facts();
        assert!(facts["pid"].as_u64().unwrap_or(0) > 0);
    }

    #[test]
    fn host_facts_never_fail_even_where_proc_is_absent() {
        // This test runs on Windows in development and Linux in CI, which is
        // exactly the point: every one of these must return something.
        let os = os_facts();
        assert!(os["platform"].is_string());
        assert!(os["arch"].is_string());
        // The rest may legitimately be null.
        let _ = init_facts();
        let _ = cpu_facts();
        let _ = machine_uptime_seconds();
        let _ = hostname();
        let components = components();
        assert_eq!(components["language"], "rust");
        assert!(components["panel_files"].as_u64().unwrap_or(0) > 0);
    }

    #[test]
    fn the_cpu_reports_parallelism_even_with_no_proc() {
        // available_parallelism works everywhere, so this field is never null
        // and the screen always has something to show.
        assert!(cpu_facts()["available_parallelism"].is_number());
    }

    #[test]
    fn the_device_list_never_carries_a_credential() {
        // The single most important assertion in this file. `/api/system`
        // reports every enrolled device, and a device record holds either a
        // bearer-token hash or an Ed25519 public key -- neither of which is any
        // client's business, and one of which is a credential.
        let devices = breeze_store::DevicesDoc {
            devices: vec![
                breeze_store::DeviceRecord {
                    token_id: "abc123".into(),
                    label: "Monique".into(),
                    auth_version: 1,
                    token_hash: Some("c".repeat(64)),
                    public_key: None,
                    created_at: 1_787_000_000.0,
                    expires_at: Some(1_790_000_000.0),
                    last_used: Some(1_787_500_000.0),
                },
                breeze_store::DeviceRecord {
                    token_id: "def456".into(),
                    label: "Panel".into(),
                    auth_version: 2,
                    token_hash: None,
                    public_key: Some("ZGVhZGJlZWZkZWFkYmVlZg".into()),
                    created_at: 1_787_000_000.0,
                    expires_at: None,
                    last_used: None,
                },
            ],
        };

        let rendered = serde_json::to_string(&devices_facts(&devices, 1_788_000_000.0)).unwrap();
        assert!(!rendered.contains("token_hash"), "{rendered}");
        assert!(!rendered.contains("public_key"), "{rendered}");
        assert!(!rendered.contains(&"c".repeat(64)), "a token hash leaked");
        assert!(
            !rendered.contains("ZGVhZGJlZWY"),
            "a public key leaked: {rendered}"
        );
        // What it *should* say.
        assert!(rendered.contains("abc123"));
        assert!(rendered.contains("Monique"));
    }

    #[test]
    fn expiry_is_reported_relative_as_well_as_absolute() {
        // A client showing "expires in 30 days" should not have to know what the
        // server thinks the time is.
        let devices = breeze_store::DevicesDoc {
            devices: vec![breeze_store::DeviceRecord {
                token_id: "a".into(),
                label: "".into(),
                auth_version: 2,
                token_hash: None,
                public_key: None,
                created_at: 0.0,
                expires_at: Some(1_000.0),
                last_used: None,
            }],
        };
        let now = 400.0;
        let facts = devices_facts(&devices, now);
        assert_eq!(facts[0]["expires_in_seconds"], 600.0);
        assert_eq!(facts[0]["expired"], false);

        // And past its expiry it says so rather than reporting a negative
        // countdown a client might render as "in -3 days".
        let facts = devices_facts(&devices, 2_000.0);
        assert_eq!(facts[0]["expired"], true);
    }

    #[test]
    fn a_non_expiring_device_has_no_countdown() {
        let devices = breeze_store::DevicesDoc {
            devices: vec![breeze_store::DeviceRecord {
                token_id: "a".into(),
                label: "".into(),
                auth_version: 1,
                token_hash: Some("x".into()),
                public_key: None,
                created_at: 0.0,
                expires_at: None,
                last_used: None,
            }],
        };
        let facts = devices_facts(&devices, 500.0);
        assert!(facts[0]["expires_in_seconds"].is_null());
        assert_eq!(facts[0]["expired"], false);
    }
}
