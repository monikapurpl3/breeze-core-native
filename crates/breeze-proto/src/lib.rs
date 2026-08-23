//! The Midea LAN protocol for air conditioners, in Rust.
//!
//! This replaces the parts of [msmart-ng] that Breeze Core actually uses, so the
//! server can ship as a single ~2 MB binary instead of a 25 MB bundle carrying a
//! Python runtime. It is not a general-purpose port: the commercial-appliance
//! class, the vendor CLI and the cloud client are all out of scope here.
//!
//! [msmart-ng]: https://github.com/mill1000/midea-msmart
//!
//! # The layers
//!
//! Four of them, wrapped one inside the next:
//!
//! ```text
//! ┌ lan  ── 0x8370 session (V3 only): AES-256-CBC + SHA-256
//! │ ┌ packet ── 0x5A5A V2 packet: AES-128-ECB + MD5
//! │ │ ┌ frame ── 0xAA appliance frame + checksum
//! │ │ │ ┌ ac ── the command or state payload + CRC-8
//! ```
//!
//! A V2 device speaks `packet` straight over TCP; a V3 device carries the very
//! same packet inside a `lan` session. Nothing above `ac` knows what an air
//! conditioner is.
//!
//! # Correctness
//!
//! Every layer is codec-only — bytes in, bytes out, no sockets — so all of it is
//! testable without hardware. The command and response vectors come from
//! msmart's own test suite (see `tests/vectors.rs`), which is the closest thing
//! to a specification this protocol has.
//!
//! # Two things that will bite you
//!
//! * **Key sizes differ per layer.** `packet` uses a fixed 16-byte key
//!   (AES-128-ECB); a V3 session uses the device's 32-byte cloud key
//!   (AES-256-CBC). See [`security`].
//! * **A unit ignores its first request after a handshake.** Wait
//!   [`lan::POST_HANDSHAKE_SETTLE_MS`] or every command times out with no error.

pub mod ac;
pub mod crc8;
pub mod discover;
pub mod frame;
pub mod lan;
pub mod packet;
pub mod security;

/// This crate's version, for build stamps and diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
