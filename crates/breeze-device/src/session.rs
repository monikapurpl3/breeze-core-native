//! A live V3 session over any byte stream.
//!
//! Generic over `Read + Write` rather than hard-wired to `TcpStream`, which is
//! what lets the whole handshake-and-request state machine be tested in-process
//! against a fake unit. Given this layer is the one that failed silently against
//! real hardware — the missing post-handshake wait — that matters more here than
//! anywhere else in the port.

use breeze_proto::lan::{self, Decoded, LanError, PacketType};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

use crate::error::DeviceError;

/// Session keys stop being honoured after a while; renew early rather than
/// discovering it mid-command.
const SESSION_LIFETIME: Duration = Duration::from_secs(lan::SESSION_LIFETIME_SECS);

/// Largest packet we will buffer from a unit. Generous for a protocol whose
/// biggest message is a couple of hundred bytes, but bounded: this reads from the
/// network, and an unbounded length field is a memory-exhaustion primitive.
const MAX_PACKET: usize = 8192;

pub struct Session<S> {
    stream: S,
    session_key: Option<[u8; 32]>,
    authenticated_at: Option<Instant>,
    packet_id: u16,
    /// How long to wait after a handshake before sending anything. Configurable
    /// only so tests need not sleep; real use wants the protocol's value.
    settle: Duration,
}

impl<S: Read + Write> Session<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            session_key: None,
            authenticated_at: None,
            packet_id: 1,
            settle: Duration::from_millis(lan::POST_HANDSHAKE_SETTLE_MS),
        }
    }

    /// Override the post-handshake wait. Intended for tests.
    pub fn with_settle(mut self, settle: Duration) -> Self {
        self.settle = settle;
        self
    }

    /// True while the session key is present and still within its lifetime.
    pub fn is_authenticated(&self) -> bool {
        match (self.session_key, self.authenticated_at) {
            (Some(_), Some(at)) => at.elapsed() < SESSION_LIFETIME,
            _ => false,
        }
    }

    /// Perform the token handshake and derive the session key.
    pub fn authenticate(&mut self, token: &[u8], key: &[u8; 32]) -> Result<(), DeviceError> {
        let request = lan::encode_handshake(self.packet_id, token);
        self.stream.write_all(&request)?;
        self.bump_packet_id();

        let packet = self.read_packet()?;
        match lan::decode(&packet, None)? {
            Decoded::Handshake(payload) => {
                self.session_key = Some(lan::derive_session_key(key, &payload)?);
                self.authenticated_at = Some(Instant::now());
            }
            Decoded::Response(_) => {
                return Err(DeviceError::Protocol(LanError::UnexpectedType(
                    PacketType::EncryptedResponse as u8,
                )))
            }
        }

        // The unit ignores whatever arrives immediately after this. Skipping the
        // wait does not fail loudly -- the next request simply never gets a
        // reply, which looks exactly like an unreachable unit.
        if !self.settle.is_zero() {
            std::thread::sleep(self.settle);
        }
        Ok(())
    }

    /// Send a V2 packet and return the V2 packet that comes back.
    pub fn request(&mut self, packet: &[u8]) -> Result<Vec<u8>, DeviceError> {
        let key = self.session_key.ok_or(DeviceError::NotAuthenticated)?;
        // Padding is discarded by the receiver; a fixed filler keeps the wire
        // deterministic, which makes packet captures comparable between runs.
        let request = lan::encode_request(&key, self.packet_id, packet, 0)?;
        self.stream.write_all(&request)?;
        self.bump_packet_id();

        let reply = self.read_packet()?;
        match lan::decode(&reply, Some(&key))? {
            Decoded::Response(v2) => Ok(v2),
            Decoded::Handshake(_) => Err(DeviceError::Protocol(LanError::UnexpectedType(
                PacketType::HandshakeResponse as u8,
            ))),
        }
    }

    /// Read exactly one framed packet, however it is split across reads.
    fn read_packet(&mut self) -> Result<Vec<u8>, DeviceError> {
        let mut buf = vec![0u8; 6];
        self.stream.read_exact(&mut buf)?;
        let total = lan::packet_len(&buf).ok_or(DeviceError::Protocol(LanError::TooShort(6)))?;
        if total > MAX_PACKET {
            return Err(DeviceError::PacketTooLarge(total));
        }
        // A packet shorter than its own header is a malformed length field, not a
        // short read; resize would panic on the subtraction.
        if total < 6 {
            return Err(DeviceError::Protocol(LanError::TooShort(total)));
        }
        buf.resize(total, 0);
        self.stream.read_exact(&mut buf[6..])?;
        Ok(buf)
    }

    fn bump_packet_id(&mut self) {
        // 12 bits, per the protocol.
        self.packet_id = self.packet_id.wrapping_add(1) & 0xFFF;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeUnit;
    use breeze_proto::ac::command;
    use breeze_proto::ac::response::State;
    use breeze_proto::ac::types::TemperatureType;
    use breeze_proto::{frame, packet};

    const KEY: [u8; 32] = [0x42; 32];
    const ID: u64 = 153_931_628_470_980;

    fn session(unit: FakeUnit) -> Session<FakeUnit> {
        Session::new(unit).with_settle(Duration::ZERO)
    }

    #[test]
    fn a_full_handshake_and_state_query_round_trips() {
        let mut s = session(FakeUnit::new(KEY, ID));
        assert!(!s.is_authenticated());
        s.authenticate(&[0xAA; 64], &KEY).unwrap();
        assert!(s.is_authenticated());

        let cmd = command::get_state(1, TemperatureType::Indoor);
        let reply = s.request(&packet::encode(ID, &cmd)).unwrap();
        let inner = packet::decode(&reply).unwrap();
        let payload = frame::parse(&inner, frame::DeviceType::AirConditioner).unwrap();
        let state = State::parse(payload).unwrap();
        // Whatever the fake reports, it must survive the whole stack intact.
        assert_eq!(state.target_temperature, 24.0);
        assert!(state.power_on);
    }

    #[test]
    fn requests_before_authenticating_are_refused_locally() {
        let mut s = session(FakeUnit::new(KEY, ID));
        let err = s.request(&[0u8; 16]).unwrap_err();
        assert!(
            matches!(err, DeviceError::NotAuthenticated),
            "got {err:?} -- must not put bytes on the wire unauthenticated"
        );
    }

    #[test]
    fn the_wrong_key_is_rejected_at_the_handshake() {
        let mut s = session(FakeUnit::new(KEY, ID));
        let err = s.authenticate(&[0xAA; 64], &[0x99; 32]).unwrap_err();
        assert!(
            matches!(err, DeviceError::Protocol(LanError::Rejected)),
            "got {err:?}"
        );
        assert!(
            !s.is_authenticated(),
            "a failed handshake must not look valid"
        );
    }

    #[test]
    fn a_setpoint_reaches_the_unit_and_is_echoed_back() {
        let mut s = session(FakeUnit::new(KEY, ID));
        s.authenticate(&[0xAA; 64], &KEY).unwrap();

        // Read, modify, write -- the only way to change one field.
        let reply = s
            .request(&packet::encode(
                ID,
                &command::get_state(1, TemperatureType::Indoor),
            ))
            .unwrap();
        let payload = packet::decode(&reply).unwrap();
        let state =
            State::parse(frame::parse(&payload, frame::DeviceType::AirConditioner).unwrap())
                .unwrap();

        let mut sp = command::Setpoint::from_state(&state);
        sp.target_temperature = 21.5;
        let reply = s
            .request(&packet::encode(ID, &command::set_state(2, &sp)))
            .unwrap();
        let payload = packet::decode(&reply).unwrap();
        let echoed =
            State::parse(frame::parse(&payload, frame::DeviceType::AirConditioner).unwrap())
                .unwrap();
        assert_eq!(
            echoed.target_temperature, 21.5,
            "unit must reflect the setpoint"
        );
    }

    #[test]
    fn packet_ids_advance_and_wrap_at_twelve_bits() {
        let mut s = session(FakeUnit::new(KEY, ID));
        s.packet_id = 0xFFF;
        s.bump_packet_id();
        assert_eq!(s.packet_id, 0, "must wrap, not overflow past 12 bits");
    }

    #[test]
    fn a_packet_split_across_reads_is_reassembled() {
        // The fake hands back one byte at a time, which is the pathological case
        // for a length-prefixed protocol read from a stream.
        let mut s = session(FakeUnit::new(KEY, ID).dribbling());
        s.authenticate(&[0xAA; 64], &KEY).unwrap();
        let reply = s
            .request(&packet::encode(
                ID,
                &command::get_state(1, TemperatureType::Indoor),
            ))
            .unwrap();
        assert!(packet::decode(&reply).is_ok());
    }

    #[test]
    fn an_absurd_length_field_is_refused_before_allocating() {
        let mut s = session(FakeUnit::new(KEY, ID).with_bogus_length(0xFFFF));
        let err = s.authenticate(&[0xAA; 64], &KEY).unwrap_err();
        assert!(
            matches!(err, DeviceError::PacketTooLarge(_)),
            "got {err:?} -- a 64 KB claim must not be honoured"
        );
    }

    #[test]
    fn a_truncated_reply_is_an_error_not_a_hang() {
        let mut s = session(FakeUnit::new(KEY, ID).truncating());
        let err = s.authenticate(&[0xAA; 64], &KEY).unwrap_err();
        assert!(matches!(err, DeviceError::Io(_)), "got {err:?}");
    }
}
