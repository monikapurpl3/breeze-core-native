//! Reading and writing Breeze Core's four store files, byte-compatibly.
//!
//! This crate exists because the native server has to be a **drop-in
//! replacement**: it will be pointed at an existing installation's
//! `config.json`, `devices.json`, `programs.json` and `timers.json`, and a user
//! must be able to switch back afterwards. Reading them correctly is the easy
//! half. Writing them back byte-for-byte identically is the half that decides
//! whether a migration is trustworthy, because a server that rewrites every
//! stored file on first start gives you nothing to compare against if something
//! later goes wrong.
//!
//! So the tests here round-trip fixtures generated from Breeze Core's own
//! pydantic models (see `tests/fixtures/generate-fixtures.py`) and assert the
//! bytes are unchanged — not merely that the values survive.
//!
//! Three details carry that guarantee:
//!
//! * **2-space indent, no trailing newline**, matching
//!   `model_dump_json(indent=2)` written with `write_text`;
//! * **`None` serialises as `null`**, never omitted;
//! * **field order is declaration order**, including `Program.id` coming last
//!   because pydantic appends subclass fields.
//!
//! File modes are preserved too: `config.json` is 640 so the admin CLIs can read
//! it, the other three are 600 because they are the app's own runtime state.

pub mod control;
pub mod models;
pub mod programs;
mod store;
pub mod timers;

pub use control::{ControlRequest, ValidationError};
pub use models::{
    AppConfig, CurveConfig, CurvePoint, DeviceRecord, DevicesDoc, Program, ProgramsDoc,
    ScheduleEntry, Timer, TimerKind, TimersDoc, UnitConfig,
};
pub use programs::{
    curve_request, curve_setpoint, due_entries, minute_stamp, round_half, ProgramError,
    ProgramSpec, PROGRAM_KINDS,
};
pub use store::{load, save, to_json, Mode, StoreError};

/// Where the four store files live when `AC_CONFIG_DIR` does not say.
///
/// `/etc/breeze-core` everywhere but Android. There, no app may write to
/// `/etc`, and Termux keeps its whole userland under its own prefix, so a
/// server with the usual default would refuse to start until told where its
/// config is -- and so would every CLI command that reads it.
#[cfg(not(target_os = "android"))]
pub const DEFAULT_CONFIG_DIR: &str = "/etc/breeze-core";
#[cfg(target_os = "android")]
pub const DEFAULT_CONFIG_DIR: &str = "/data/data/com.termux/files/usr/etc/breeze-core";

/// The store directory: `AC_CONFIG_DIR`, or [`DEFAULT_CONFIG_DIR`]. The server
/// and the CLI resolve it through here so they cannot disagree.
pub fn config_dir() -> String {
    std::env::var("AC_CONFIG_DIR").unwrap_or_else(|_| DEFAULT_CONFIG_DIR.into())
}
pub use timers::{
    build_start_timer, build_timer, now_local, TimerError, MAX_MINUTES, MAX_START_DAYS, MAX_UNITS,
    START_GRACE_MINUTES,
};
