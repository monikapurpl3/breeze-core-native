//! Talking to real air conditioners: connections, retries and state.
//!
//! [`breeze_proto`] is pure codec and knows nothing about sockets. This crate is
//! where the sockets live, plus the two things that only matter once a real unit
//! is on the other end: connections are **cached** because a handshake is
//! expensive, and requests to one unit are **serialised** because a unit answers
//! one at a time.
//!
//! ```no_run
//! use breeze_device::{DeviceManager, UnitConfig};
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let configs: Vec<UnitConfig> = vec![];
//! let manager = DeviceManager::new(configs);
//! for id in manager.known_units() {
//!     let state = manager.with_unit(id, |d| d.refresh())?;
//!     println!("{id}: {:?} at {} C", state.mode, state.target_temperature);
//! }
//! # Ok(()) }
//! ```

mod device;
mod error;
mod manager;
pub mod scan;
mod session;

#[cfg(test)]
mod testing;

pub use device::{Device, UnitConfig};
pub use error::DeviceError;
pub use manager::DeviceManager;
pub use scan::{scan, scan_subnet, ScanError, ScanReport};
pub use session::Session;
