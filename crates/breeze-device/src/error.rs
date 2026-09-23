//! What can go wrong talking to a unit.
//!
//! The distinction that matters to callers is **retryable or not**: a timeout is
//! worth another attempt on a fresh connection, a rejected key never will be.
//! Breeze Core learned this the hard way on its auth layer, where treating every
//! failure as fatal locked users out; the same reasoning applies here.

use breeze_proto::lan::LanError;
use breeze_proto::packet::PacketError;
use breeze_proto::{ac::response::ResponseError, frame::FrameError};

#[derive(Debug)]
pub enum DeviceError {
    /// Socket-level failure: unreachable, refused, timed out, closed.
    Io(std::io::Error),
    /// Session framing or crypto.
    Protocol(LanError),
    /// V2 packet framing or digest.
    Packet(PacketError),
    /// Appliance frame framing or checksum.
    Frame(FrameError),
    /// State report we could not read.
    Response(ResponseError),
    /// A request was attempted before the handshake.
    NotAuthenticated,
    /// A unit claimed a packet size we refuse to allocate.
    PacketTooLarge(usize),
    /// This unit has no V3 credentials in the configuration.
    MissingCredentials,
    /// Ran out of attempts.
    Unreachable { attempts: u32 },
}

/// Cloneable so that one outcome can answer every request that shared it.
///
/// When controls to a unit are merged into one command, or reads share one
/// round-trip, a single failure is the answer to all of them. `std::io::Error`
/// is not `Clone`, so it is rebuilt from its kind and message -- the kind is
/// what [`DeviceError::is_retryable`] reads, and the message is what a caller
/// shows, so nothing a caller can observe is lost.
impl Clone for DeviceError {
    fn clone(&self) -> Self {
        match self {
            Self::Io(e) => Self::Io(std::io::Error::new(e.kind(), e.to_string())),
            Self::Protocol(e) => Self::Protocol(e.clone()),
            Self::Packet(e) => Self::Packet(e.clone()),
            Self::Frame(e) => Self::Frame(e.clone()),
            Self::Response(e) => Self::Response(e.clone()),
            Self::NotAuthenticated => Self::NotAuthenticated,
            Self::PacketTooLarge(n) => Self::PacketTooLarge(*n),
            Self::MissingCredentials => Self::MissingCredentials,
            Self::Unreachable { attempts } => Self::Unreachable {
                attempts: *attempts,
            },
        }
    }
}

impl DeviceError {
    /// Whether another attempt could plausibly succeed.
    ///
    /// Timeouts, resets and truncated reads are transient — a unit drops idle
    /// connections and its session key expires. A rejected key or a malformed
    /// frame will fail identically forever, and retrying only delays the error
    /// the caller needs to see.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Io(e) => matches!(
                e.kind(),
                std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::UnexpectedEof
            ),
            // A stale session produces garbage that fails its signature; a
            // reconnect fixes it.
            Self::Protocol(LanError::BadSignature)
            | Self::Protocol(LanError::Incomplete { .. }) => true,
            Self::NotAuthenticated => true,
            _ => false,
        }
    }
}

impl core::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "network error: {e}"),
            Self::Protocol(e) => write!(f, "session error: {e}"),
            Self::Packet(e) => write!(f, "packet error: {e}"),
            Self::Frame(e) => write!(f, "frame error: {e}"),
            Self::Response(e) => write!(f, "state report error: {e}"),
            Self::NotAuthenticated => write!(f, "not authenticated with the unit"),
            Self::PacketTooLarge(n) => write!(f, "unit claimed a {n}-byte packet; refusing"),
            Self::MissingCredentials => write!(f, "unit has no V3 token and key configured"),
            Self::Unreachable { attempts } => write!(f, "no response after {attempts} attempts"),
        }
    }
}

impl std::error::Error for DeviceError {}

impl From<std::io::Error> for DeviceError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<LanError> for DeviceError {
    fn from(e: LanError) -> Self {
        Self::Protocol(e)
    }
}
impl From<PacketError> for DeviceError {
    fn from(e: PacketError) -> Self {
        Self::Packet(e)
    }
}
impl From<FrameError> for DeviceError {
    fn from(e: FrameError) -> Self {
        Self::Frame(e)
    }
}
impl From<ResponseError> for DeviceError {
    fn from(e: ResponseError) -> Self {
        Self::Response(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    #[test]
    fn a_cloned_error_keeps_its_retry_decision_and_message() {
        // A shared outcome is cloned once per waiting request; if the clone lost
        // the kind, a timeout would stop being retried the moment two requests
        // shared it.
        let e = DeviceError::Io(std::io::Error::new(ErrorKind::TimedOut, "no reply"));
        let c = e.clone();
        assert!(c.is_retryable());
        assert_eq!(c.to_string(), e.to_string());
        assert!(!DeviceError::MissingCredentials.clone().is_retryable());
    }

    #[test]
    fn transient_network_failures_are_retryable() {
        for kind in [
            ErrorKind::TimedOut,
            ErrorKind::ConnectionReset,
            ErrorKind::BrokenPipe,
            ErrorKind::UnexpectedEof,
        ] {
            let e = DeviceError::Io(std::io::Error::new(kind, "x"));
            assert!(e.is_retryable(), "{kind:?} should be retryable");
        }
    }

    #[test]
    fn a_refused_connection_is_not_retryable() {
        // Nothing is listening; hammering it will not change that.
        let e = DeviceError::Io(std::io::Error::new(ErrorKind::ConnectionRefused, "x"));
        assert!(!e.is_retryable());
    }

    #[test]
    fn a_rejected_key_is_never_retryable() {
        // The lesson from Breeze Core's lockout: retrying a fatal auth failure
        // buys nothing and hides the real cause.
        assert!(!DeviceError::Protocol(LanError::Rejected).is_retryable());
        assert!(!DeviceError::MissingCredentials.is_retryable());
    }

    #[test]
    fn a_stale_session_is_retryable_because_reconnecting_fixes_it() {
        assert!(DeviceError::Protocol(LanError::BadSignature).is_retryable());
        assert!(DeviceError::NotAuthenticated.is_retryable());
    }

    #[test]
    fn an_oversized_packet_claim_is_fatal() {
        assert!(!DeviceError::PacketTooLarge(65535).is_retryable());
    }
}
