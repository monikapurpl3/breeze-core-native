//! Fetch a unit's V3 credentials from the cloud, from the command line.
//!
//! ```text
//! cargo run -p breeze-cloud --example fetch -- <device-id> <account> <region>
//! ```
//!
//! The password is read from the `MIDEA_PASSWORD` environment variable, never
//! from an argument: an argument shows up in `ps` and in shell history, and this
//! is somebody's actual Midea account.
//!
//! Exists to verify this client against the live service. Logging in with
//! msmart's shared accounts should succeed and then be refused at `getToken`
//! with `9999` — which is the same wall msmart hits, and proves the signing,
//! the login flow and the error handling all work.

use breeze_cloud::{nethome, Credentials};

fn main() {
    let mut args = std::env::args().skip(1);
    let device_id: u64 = match args.next().and_then(|v| v.parse().ok()) {
        Some(id) => id,
        None => {
            eprintln!("usage: fetch <device-id> <account> [region]");
            eprintln!("       with the password in MIDEA_PASSWORD");
            std::process::exit(2);
        }
    };
    let account = args.next().unwrap_or_default();
    let region = args.next().unwrap_or_else(|| "DE".into());
    let password = std::env::var("MIDEA_PASSWORD").unwrap_or_default();

    if account.is_empty() || password.is_empty() {
        eprintln!("an account and MIDEA_PASSWORD are both required");
        std::process::exit(2);
    }

    println!("unit {device_id}, region {region}, account {account}");
    println!(
        "udpid little-endian: {}",
        breeze_cloud::udpid(device_id, false)
    );
    println!(
        "udpid big-endian:    {}",
        breeze_cloud::udpid(device_id, true)
    );
    println!();

    let credentials = Credentials::new(account, password, region);

    // Login on its own first, so a refusal can be attributed to the right step.
    print!("logging in... ");
    match nethome::login(&credentials) {
        Ok(_) => println!("OK"),
        Err(e) => {
            println!("failed");
            eprintln!("\n{e}");
            eprintln!("{}", e.advice());
            std::process::exit(1);
        }
    }

    print!("fetching the token... ");
    match nethome::fetch_token(&credentials, device_id) {
        Ok(token) => {
            println!("OK");
            // Lengths only by default: this is a credential, and a terminal
            // scrollback is not where it should live.
            println!("\ntoken: {} characters", token.token.len());
            println!("key:   {} characters", token.key.len());
            if std::env::var("SHOW_CREDENTIALS").as_deref() == Ok("1") {
                println!("\ntoken: {}", token.token);
                println!("key:   {}", token.key);
            } else {
                println!("\n(set SHOW_CREDENTIALS=1 to print them)");
            }
        }
        Err(e) => {
            println!("failed");
            eprintln!("\n{e}");
            eprintln!("{}", e.advice());
            std::process::exit(1);
        }
    }
}
