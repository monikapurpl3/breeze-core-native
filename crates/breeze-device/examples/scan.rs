//! Run a real LAN discovery and print what answers.
//!
//! ```text
//! cargo run -p breeze-device --example scan            # local /24, plus broadcast
//! cargo run -p breeze-device --example scan 10.0.0.0/24
//! ```
//!
//! Exists to answer "why does automatic pairing find nothing", which is not a
//! question a unit test can settle: it depends on the host's firewall and on
//! whether the access point forwards broadcast. The report says which method
//! actually produced the answers.

use std::time::{Duration, Instant};

fn main() {
    let arg = std::env::args().nth(1);
    let listen = Duration::from_secs(
        std::env::var("SCAN_LISTEN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4),
    );

    let started = Instant::now();
    let result = match &arg {
        Some(cidr) => {
            println!("sweeping {cidr} (no broadcast), listening {listen:?}\n");
            breeze_device::scan_subnet(cidr, listen)
        }
        None => {
            match breeze_device::scan::local_ipv4() {
                Some(ip) => println!("this host is {ip}; sweeping its /24 and broadcasting"),
                None => println!("could not work out a local private address; broadcast only"),
            }
            println!("listening {listen:?}\n");
            breeze_device::scan(listen)
        }
    };

    let report = match result {
        Ok(r) => r,
        Err(e) => {
            eprintln!("scan failed: {e}");
            std::process::exit(1);
        }
    };

    println!(
        "sent {} unicast probe(s){}, took {:.1}s",
        report.probes_sent,
        if report.broadcast_sent {
            " plus a broadcast"
        } else {
            ""
        },
        started.elapsed().as_secs_f64()
    );
    if let Some(swept) = &report.swept {
        println!("swept {swept}");
    }
    println!();

    if report.found.is_empty() {
        println!("no units answered.");
    } else {
        println!("{} unit(s):", report.found.len());
        for unit in &report.found {
            println!(
                "  {:<15} id={:<18} port={} {:?} ssid={} type=0x{:02x}",
                unit.ip.to_string(),
                unit.id,
                unit.port,
                unit.version,
                unit.ssid,
                unit.device_type
            );
            if unit.ip != unit.reported_ip {
                println!(
                    "      note: the unit reports {} but answered from {}",
                    unit.reported_ip, unit.ip
                );
            }
        }
    }

    if !report.unparseable.is_empty() {
        println!(
            "\n{} reply/replies not understood:",
            report.unparseable.len()
        );
        for (ip, why) in &report.unparseable {
            println!("  {ip}: {why}");
        }
    }
}
