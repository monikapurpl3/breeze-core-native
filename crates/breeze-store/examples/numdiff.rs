//! Diagnose a round-trip difference by comparing only the *numbers* in a file.
//!
//! `verify` withholds contents because the store files hold credentials. When it
//! reports a difference, this narrows it down safely: numeric literals are not
//! secret, so it lists every number whose textual form changes when Rust
//! re-serialises it, and says nothing about any string.
//!
//! Read-only. It never writes anything.

use std::path::Path;

/// Pull numeric literals out of JSON text, in order, skipping anything inside a
/// string so a hex token full of digits is never picked up.
fn numbers(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_string {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            i += 1;
            continue;
        }
        if c.is_ascii_digit()
            || (c == '-' && i + 1 < bytes.len() && (bytes[i + 1] as char).is_ascii_digit())
        {
            let start = i;
            i += 1;
            while i < bytes.len() {
                let d = bytes[i] as char;
                if d.is_ascii_digit() || d == '.' || d == 'e' || d == 'E' || d == '+' || d == '-' {
                    i += 1;
                } else {
                    break;
                }
            }
            out.push(text[start..i].to_string());
            continue;
        }
        i += 1;
    }
    out
}

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: numdiff <store.json>");
            std::process::exit(2);
        }
    };
    let text = match std::fs::read_to_string(Path::new(&path)) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            std::process::exit(1);
        }
    };
    // Parse as untyped JSON: this asks "how does serde render these numbers?",
    // independent of our own structs.
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{path} did not parse: {e}");
            std::process::exit(1);
        }
    };
    let rewritten = serde_json::to_string_pretty(&value).expect("Value always serialises");

    let before = numbers(&text);
    let after = numbers(&rewritten);
    println!("{path}: {} numeric literal(s)", before.len());
    if before.len() != after.len() {
        println!("  count changed: {} -> {}", before.len(), after.len());
    }

    let mut differences = 0;
    for (i, (a, b)) in before.iter().zip(after.iter()).enumerate() {
        if a != b {
            differences += 1;
            println!("  #{i}: python {a}  ->  rust {b}");
        }
    }
    if differences == 0 {
        println!("  every number renders identically");
        println!("  (so the difference is in strings, key order, or whitespace)");
    } else {
        println!("  {differences} number(s) render differently");
    }
    // Also report whether untyped round-tripping alone reproduces the file, which
    // separates "our structs are wrong" from "serde renders differently".
    println!(
        "  untyped round-trip is {}",
        if rewritten == text {
            "byte-identical, so the difference comes from our structs"
        } else {
            "already different, so it is serde's rendering, not our structs"
        }
    );
}
