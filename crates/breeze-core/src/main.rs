//! The binary: a server, and the tools that talk to it.
//!
//! `serve` assembles everything from the store files named by the environment,
//! using the same variables Breeze Core reads, so an existing systemd unit or
//! container runs this unchanged. The other subcommands are API clients — they
//! go through the same HTTP surface and the same authentication a phone does,
//! which is what makes them work identically on the server and off it.

use std::sync::Arc;

mod cli;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match cli::parse(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("breeze-core: {e}");
            std::process::exit(2);
        }
    };

    let result = match command {
        cli::Command::Serve {
            host,
            port,
            behind_proxy,
        } => {
            serve(host, port, behind_proxy);
            Ok(())
        }
        cli::Command::Control(args) => cli::control::run(&args),
        // The only subcommand that is not an API client: there is no key to
        // authenticate with until this has run once.
        cli::Command::Pair { ip, out, prompt } => {
            cli::pair::run(cli::pair::Options { ip, out, prompt })
        }
        cli::Command::Diag { client } => match cli::diag::run(&client) {
            // The exit code is the point of a diagnostic in a script.
            Ok(code) => std::process::exit(code),
            Err(e) => Err(e),
        },
        cli::Command::Login { base_url } => cli::profile::enrol(base_url).map(|_| ()),
        cli::Command::Approve { code, client } => cli::approve(code, &client),
        cli::Command::Devices { client } => cli::admin::devices(&client),
        cli::Command::Revoke { token_id, client } => cli::admin::revoke(&token_id, &client),
        cli::Command::Units { client } => cli::units(&client),
        cli::Command::Version => {
            println!("breeze-core {}", env!("CARGO_PKG_VERSION"));
            println!("commit {}", breeze_http::build_commit());
            Ok(())
        }
        cli::Command::Help => {
            println!("{}", cli::usage());
            Ok(())
        }
    };

    if let Err(e) = result {
        eprintln!("breeze-core: {e}");
        std::process::exit(1);
    }
}

/// Run the server until it stops.
///
/// Flags win over the environment, because a flag is the more specific
/// instruction — and because the reference's systemd unit passes them, which
/// this binary used to ignore entirely.
fn serve(host: Option<String>, port: Option<u16>, behind_proxy: bool) {
    let mut settings = breeze_http::Settings::from_env();
    // One-way: the flag can turn proxy-header trust on, never off. AC_BEHIND_PROXY
    // is set by the packaged unit and the flag by BREEZE_OPTS, and a flag that
    // could silently *disable* it would turn the LAN-only admin check into a
    // check that every proxied request passes.
    if behind_proxy {
        settings.behind_proxy = true;
    }
    if host.is_some() || port.is_some() {
        let (current_host, current_port) = settings
            .bind
            .rsplit_once(':')
            .map(|(h, p)| (h.to_string(), p.to_string()))
            .unwrap_or_else(|| ("127.0.0.1".to_string(), "8420".to_string()));
        settings.bind = format!(
            "{}:{}",
            host.unwrap_or(current_host),
            port.map(|p| p.to_string()).unwrap_or(current_port)
        );
    }

    let state = match breeze_http::AppState::load(settings, env!("CARGO_PKG_VERSION")) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            // Startup failures are almost always a path or a permission, so say
            // which file rather than just failing.
            eprintln!("breeze-core: {e}");
            std::process::exit(1);
        }
    };
    // Each on its own thread, so a disk error while a timer deletes itself cannot
    // take the HTTP server down with it -- nor stop schedules and curves running.
    let _runner = breeze_http::timer_routes::spawn_runner(Arc::clone(&state));
    let _scheduler = breeze_http::program_routes::spawn_scheduler(Arc::clone(&state));
    // Idles until a client subscribes, so a server nobody is watching makes no
    // LAN traffic at all.
    let _poller = breeze_http::stream::spawn_poller(Arc::clone(&state));

    if let Err(e) = breeze_http::serve(state) {
        eprintln!("breeze-core: {e}");
        std::process::exit(1);
    }
}
