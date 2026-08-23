//! The REST surface, as the existing clients speak it.
//!
//! Breeze Core's three clients — the Android app, the web panel and the
//! diagnostic CLI — are all HTTP clients of this contract, and none of them are
//! being changed. So the shapes here are fixed: field names, field *types*, and
//! the status codes for each failure. `GET /api/units/{unknown}` is a 404, a bad
//! enum is a 400, an unreachable unit is a 503, and out-of-range input is a 422
//! rejected before it ever reaches an air conditioner.
//!
//! Blocking I/O throughout, one thread per connection. That is not a compromise:
//! a request spends most of its life waiting on a LAN round-trip to a unit, and
//! there are a handful of clients, so an async runtime would cost a megabyte of
//! binary and buy nothing measurable.

pub mod auth_routes;
pub mod config_routes;
pub mod control;
pub mod history;
pub mod metrics;
pub mod panel;
pub mod program_routes;
pub mod respond;
pub mod server;
pub mod state;
pub mod stream;
pub mod system;
pub mod timer_routes;
pub mod units;

pub use respond::Reply;
pub use server::serve;
pub use state::{AppState, Settings, StartupError};
pub use units::{UnitState, UnitSummary};

/// The commit this binary was built from, or `"unknown"`.
///
/// Set by CI via `BREEZE_COMMIT`. Shared by `/api/version` and `/metrics` so a
/// scrape and a client see the same build identity.
pub fn build_commit() -> &'static str {
    option_env!("BREEZE_COMMIT").unwrap_or("unknown")
}
