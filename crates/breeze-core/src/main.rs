//! The server binary.
//!
//! Everything is assembled from the store files named by the environment, using
//! the same variables Breeze Core reads, so an existing systemd unit or container
//! runs this unchanged.

use std::sync::Arc;

fn main() {
    let settings = breeze_http::Settings::from_env();
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
    let _runner = breeze_http::timer_routes::spawn_runner(std::sync::Arc::clone(&state));
    let _scheduler = breeze_http::program_routes::spawn_scheduler(std::sync::Arc::clone(&state));
    // Idles until a client subscribes, so a server nobody is watching makes no
    // LAN traffic at all.
    let _poller = breeze_http::stream::spawn_poller(std::sync::Arc::clone(&state));

    if let Err(e) = breeze_http::serve(state) {
        eprintln!("breeze-core: {e}");
        std::process::exit(1);
    }
}
