//! A live V3 session over any byte stream.
//!
//! Generic over [`Link`] rather than hard-wired to `TcpStream`, which is what
//! lets the whole handshake-and-request state machine be tested in-process
//! against a fake unit. Given this layer is the one that failed silently against
//! real hardware — the missing post-handshake wait — that matters more here than
//! anywhere else in the port.
//!
//! # Waiting on a unit
//!
//! Measured against real units (the `wire` example), a reply takes a steady
//! ~0.72 s, and now and then a unit simply ignores a request: no reply ever
//! comes, and the *same connection* answers the next one normally. So an
//! unanswered request is resent on the connection it was sent on after
//! [`Timing::reply_timeout`], as msmart does, rather than treated as a dead
//! connection. 4.0.2 waited ten seconds and then reconnected, which turned every
//! ignored request into a 12.5 s control.
//!
//! Resending has one hazard, and it is the one that makes a slider spring back:
//! if the first send *is* answered after all, two replies are now on their way,
//! and the second must not be read as the reply to whatever is asked next.
//! Nothing in a reply identifies its request — the unit numbers its replies
//! with its own counter, not ours — so the defence is time, not matching:
//! after a resent request completes, the session keeps reading for
//! [`Timing::linger`] and discards what arrives, and before every request it
//! discards anything already waiting.

use breeze_proto::lan::{self, Decoded, LanError, PacketType};
use std::io::{self, ErrorKind, Read, Write};
use std::time::{Duration, Instant};

use crate::error::DeviceError;

/// Session keys stop being honoured after a while; renew early rather than
/// discovering it mid-command.
const SESSION_LIFETIME: Duration = Duration::from_secs(lan::SESSION_LIFETIME_SECS);

/// Largest packet we will buffer from a unit. Generous for a protocol whose
/// biggest message is a couple of hundred bytes, but bounded: this reads from the
/// network, and an unbounded length field is a memory-exhaustion primitive.
const MAX_PACKET: usize = 8192;

/// How a session waits on a unit.
///
/// Real use wants [`Timing::default`]. Tests shrink it, so that an ignored
/// request costs milliseconds rather than seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// Pause after the handshake before the unit will listen.
    pub settle: Duration,
    /// How long to wait for a reply before sending the request again.
    pub reply_timeout: Duration,
    /// Sends per request on one connection before giving up on it.
    pub sends: u32,
    /// After a request needed resending, how long to keep reading for the
    /// duplicate reply the earlier send may still produce.
    pub linger: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            settle: Duration::from_millis(lan::POST_HANDSHAKE_SETTLE_MS),
            // msmart's value. A healthy reply takes ~0.72 s, so this is nearly
            // three replies' worth of patience before asking again.
            reply_timeout: Duration::from_secs(2),
            sends: 3,
            // A little more than one reply's worth: long enough for the
            // earlier send's answer to arrive if it is coming at all.
            linger: Duration::from_secs(1),
        }
    }
}

/// A byte stream a session can run over.
///
/// A session needs two things a plain `Read + Write` does not promise: a
/// bounded wait, which is what turns "the unit ignored that" into a resend
/// instead of a hang, and a look without waiting at all, which is how stale
/// bytes are cleared before a request goes out.
pub trait Link: Read + Write {
    /// Bound the reads that follow. Never called with zero.
    fn set_read_deadline(&mut self, timeout: Duration) -> io::Result<()>;
    /// Read what has already arrived: `WouldBlock` when nothing has, `Ok(0)`
    /// only when the peer has closed.
    fn read_now(&mut self, buf: &mut [u8]) -> io::Result<usize>;
}

impl Link for std::net::TcpStream {
    fn set_read_deadline(&mut self, timeout: Duration) -> io::Result<()> {
        // `set_read_timeout(Some(ZERO))` is an error; the floor keeps a deadline
        // that has all but passed from being one.
        self.set_read_timeout(Some(timeout.max(Duration::from_millis(1))))
    }

    fn read_now(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.set_nonblocking(true)?;
        let result = self.read(buf);
        // Restored even when the read failed: a socket left non-blocking would
        // make every later timed read return at once, which looks exactly like
        // a unit that never answers.
        self.set_nonblocking(false)?;
        result
    }
}

/// Counts worth seeing on a diagnostics screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkStats {
    /// Requests sent again because the unit did not answer in time.
    pub resends: u64,
    /// Replies discarded because nothing was waiting for them.
    pub discarded: u64,
}

pub struct Session<S> {
    stream: S,
    session_key: Option<[u8; 32]>,
    authenticated_at: Option<Instant>,
    packet_id: u16,
    timing: Timing,
    /// Bytes received and not yet consumed as a packet. Kept across reads, so a
    /// timeout that lands in the middle of a packet loses none of it.
    rx: Vec<u8>,
    stats: LinkStats,
}

impl<S: Link> Session<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            session_key: None,
            authenticated_at: None,
            packet_id: 1,
            timing: Timing::default(),
            rx: Vec::new(),
            stats: LinkStats::default(),
        }
    }

    /// Override how the session waits. Intended for tests.
    pub fn with_timing(mut self, timing: Timing) -> Self {
        self.timing = timing;
        self
    }

    /// Override the post-handshake wait. Intended for tests.
    pub fn with_settle(mut self, settle: Duration) -> Self {
        self.timing.settle = settle;
        self
    }

    pub fn stats(&self) -> LinkStats {
        self.stats
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
        // A handshake the unit missed is resent like any request -- msmart
        // does the same -- and starts from a clean buffer.
        self.rx.clear();
        let packet = self.send_and_wait(|s| {
            let request = lan::encode_handshake(s.packet_id, token);
            s.bump_packet_id();
            Ok(request)
        })?;
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
        if !self.timing.settle.is_zero() {
            std::thread::sleep(self.timing.settle);
        }
        Ok(())
    }

    /// Send a V2 packet and return the V2 packet that comes back.
    pub fn request(&mut self, packet: &[u8]) -> Result<Vec<u8>, DeviceError> {
        let key = self.session_key.ok_or(DeviceError::NotAuthenticated)?;
        // Anything already waiting is an answer to a question nobody is asking
        // any more. Taken as the reply to this one, it would leave every later
        // reply one behind.
        self.drain()?;
        let reply = self.send_and_wait(|s| {
            // A fresh packet id per send, resends included, as msmart does.
            let request = lan::encode_request(&key, s.packet_id, packet, 0)?;
            s.bump_packet_id();
            Ok(request)
        })?;
        match lan::decode(&reply, Some(&key))? {
            Decoded::Response(v2) => Ok(v2),
            Decoded::Handshake(_) => Err(DeviceError::Protocol(LanError::UnexpectedType(
                PacketType::HandshakeResponse as u8,
            ))),
        }
    }

    /// Send, wait up to `reply_timeout`, and resend on silence -- at most
    /// `sends` times on this connection. Returns the first packet to arrive.
    fn send_and_wait(
        &mut self,
        mut encode: impl FnMut(&mut Self) -> Result<Vec<u8>, DeviceError>,
    ) -> Result<Vec<u8>, DeviceError> {
        let mut sent = 0;
        loop {
            let bytes = encode(self)?;
            self.stream.write_all(&bytes)?;
            sent += 1;
            match self.read_packet(self.timing.reply_timeout) {
                Ok(packet) => {
                    if sent > 1 {
                        self.stats.resends += u64::from(sent - 1);
                        self.absorb_late_replies();
                    }
                    return Ok(packet);
                }
                Err(e) if is_timeout(&e) && sent < self.timing.sends => continue,
                Err(e) => return Err(e),
            }
        }
    }

    /// After a resent request: an earlier send may still be answered, and that
    /// answer must not become the reply to whatever is sent next. Wait out the
    /// linger window and discard what arrives. Best effort by design -- the
    /// reply is already in hand, so nothing here can fail the request.
    fn absorb_late_replies(&mut self) {
        let deadline = Instant::now() + self.timing.linger;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            match self.read_packet(left) {
                Ok(_) => self.stats.discarded += 1,
                // Timeout: nothing more came. Anything else -- the unit hanging
                // up, say -- is found by the next request's drain.
                Err(_) => return,
            }
        }
    }

    /// Discard whatever has arrived that no request is waiting for, and notice
    /// a unit that has hung up before writing into the dead connection.
    fn drain(&mut self) -> Result<(), DeviceError> {
        let mut buf = [0u8; 1024];
        loop {
            match self.stream.read_now(&mut buf) {
                // In practice the unit's idle timer: it closes a connection
                // exactly 30 s after the last request.
                Ok(0) => return Err(closed()),
                Ok(n) => self.rx.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        while self.take_packet()?.is_some() {
            self.stats.discarded += 1;
        }
        // A fragment left over is the front of a stale packet still arriving.
        // Finish it and throw it away; if it never completes, the framing can
        // no longer be trusted and the connection should be replaced.
        if !self.rx.is_empty() {
            self.read_packet(self.timing.reply_timeout)?;
            self.stats.discarded += 1;
        }
        Ok(())
    }

    /// Read exactly one framed packet, however it is split across reads, or
    /// fail with a timeout once `timeout` has passed.
    fn read_packet(&mut self, timeout: Duration) -> Result<Vec<u8>, DeviceError> {
        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; 1024];
        loop {
            if let Some(packet) = self.take_packet()? {
                return Ok(packet);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(timed_out());
            }
            self.stream.set_read_deadline(left)?;
            match self.stream.read(&mut buf) {
                Ok(0) => return Err(closed()),
                Ok(n) => self.rx.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    return Err(timed_out())
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Split one complete packet off the front of the receive buffer.
    fn take_packet(&mut self) -> Result<Option<Vec<u8>>, DeviceError> {
        // Even a fragment must start like a packet, or the framing is lost and
        // nothing after it can be trusted.
        if self.rx.len() >= 2 && self.rx[..2] != [0x83, 0x70] {
            return Err(self.lost_framing());
        }
        let Some(total) = lan::packet_len(&self.rx) else {
            return Ok(None);
        };
        if total > MAX_PACKET {
            self.rx.clear();
            return Err(DeviceError::PacketTooLarge(total));
        }
        if self.rx.len() < total {
            return Ok(None);
        }
        Ok(Some(self.rx.drain(..total).collect()))
    }

    fn lost_framing(&mut self) -> DeviceError {
        let head = [self.rx[0], self.rx[1]];
        self.rx.clear();
        DeviceError::Protocol(LanError::NotAPacket(head))
    }

    fn bump_packet_id(&mut self) {
        // 12 bits, per the protocol.
        self.packet_id = self.packet_id.wrapping_add(1) & 0xFFF;
    }
}

/// No reply in time. Retryable: the next attempt uses a fresh connection.
fn timed_out() -> DeviceError {
    DeviceError::Io(io::Error::new(
        ErrorKind::TimedOut,
        "the unit did not reply",
    ))
}

/// The unit hung up. Retryable, for the same reason.
fn closed() -> DeviceError {
    DeviceError::Io(io::Error::new(
        ErrorKind::UnexpectedEof,
        "the unit closed the connection",
    ))
}

fn is_timeout(e: &DeviceError) -> bool {
    matches!(e, DeviceError::Io(io) if io.kind() == ErrorKind::TimedOut)
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

    fn state_of(reply: &[u8]) -> State {
        let inner = packet::decode(reply).unwrap();
        State::parse(frame::parse(&inner, frame::DeviceType::AirConditioner).unwrap()).unwrap()
    }

    #[test]
    fn a_full_handshake_and_state_query_round_trips() {
        let mut s = session(FakeUnit::new(KEY, ID));
        assert!(!s.is_authenticated());
        s.authenticate(&[0xAA; 64], &KEY).unwrap();
        assert!(s.is_authenticated());

        let cmd = command::get_state(1, TemperatureType::Indoor);
        let state = state_of(&s.request(&packet::encode(ID, &cmd)).unwrap());
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

        let get = command::get_state(1, TemperatureType::Indoor);
        let state = state_of(&s.request(&packet::encode(ID, &get)).unwrap());

        let mut sp = command::Setpoint::from_state(&state);
        sp.target_temperature = 21.5;
        let reply = s
            .request(&packet::encode(ID, &command::set_state(2, &sp)))
            .unwrap();
        assert_eq!(
            state_of(&reply).target_temperature,
            21.5,
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
        let get = command::get_state(1, TemperatureType::Indoor);
        let reply = s.request(&packet::encode(ID, &get)).unwrap();
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

    #[test]
    fn bytes_that_are_not_a_packet_are_refused_rather_than_read_as_a_length() {
        // A stream that has lost its framing would otherwise have its next two
        // bytes read as a length, and every packet after that misread.
        let mut s = session(FakeUnit::new(KEY, ID));
        s.rx.extend_from_slice(&[0x12, 0x34, 0x00, 0x10, 0x20, 0x03]);
        let err = s.take_packet().unwrap_err();
        assert!(
            matches!(err, DeviceError::Protocol(LanError::NotAPacket(_))),
            "got {err:?}"
        );
        assert!(s.rx.is_empty(), "the corrupt bytes must be dropped");
    }
}
