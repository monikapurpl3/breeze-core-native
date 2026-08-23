//! Check that a real store file survives a round-trip byte-for-byte.
//!
//! Run this against a live installation before trusting a migration. It reports
//! only sizes and a verdict — never the file's contents, which hold the API key
//! and per-unit credentials.
//!
//! ```text
//! verify /etc/breeze-core/config.json
//! ```

use breeze_store::{to_json, AppConfig, DevicesDoc, ProgramsDoc, TimersDoc};

/// Round-trip `text` as `T` and report whether the bytes came back identical.
fn check<T>(kind: &str, text: &str) -> bool
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let parsed: T = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            println!("  FAILED to parse as {kind}: {e}");
            return false;
        }
    };
    let rewritten = match to_json(&parsed) {
        Ok(s) => s,
        Err(e) => {
            println!("  FAILED to serialise: {e}");
            return false;
        }
    };
    if rewritten == text {
        println!(
            "  identical: {} bytes in, {} bytes out",
            text.len(),
            rewritten.len()
        );
        return true;
    }
    // Report the position and nothing else: the surrounding text is secret.
    let at = text
        .bytes()
        .zip(rewritten.bytes())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| text.len().min(rewritten.len()));
    println!(
        "  DIFFERS at byte {at} of {} (rewritten is {} bytes)",
        text.len(),
        rewritten.len()
    );
    println!("  (contents withheld -- this file holds credentials)");
    false
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: verify <store.json> [more.json ...]");
        std::process::exit(2);
    }

    let mut all_ok = true;
    for path in &args {
        println!("{path}");
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                println!("  cannot read: {e}");
                all_ok = false;
                continue;
            }
        };
        // Pick the document type from the filename, since that is what a real
        // deployment uses; the shapes are distinct enough that guessing wrong
        // would fail loudly anyway.
        let name = std::path::Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let ok = match name {
            "config.json" => check::<AppConfig>("config", &text),
            "devices.json" => check::<DevicesDoc>("devices", &text),
            "programs.json" => check::<ProgramsDoc>("programs", &text),
            "timers.json" => check::<TimersDoc>("timers", &text),
            other => {
                println!("  unknown store file {other:?}; expected one of config/devices/programs/timers.json");
                false
            }
        };
        all_ok &= ok;
    }
    if !all_ok {
        std::process::exit(1);
    }
}
