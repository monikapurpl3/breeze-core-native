//! A fake air conditioner, for testing the session layer without hardware.
//!
//! It implements the *device* side of the protocol properly — real handshake,
//! real AES-256-CBC session, real V2 packets, real frames — so a test that passes
//! against it has exercised every layer rather than a mock's idea of them. The
//! shortcuts are all in behaviour, not encoding: it keeps one state and answers
//! immediately.
//!
//! Compiled only for tests; nothing here ships.

use breeze_proto::ac::command::Setpoint;
use breeze_proto::ac::types::{FanSpeed, Mode, SwingMode};
use breeze_proto::lan::{self, PacketType};
use breeze_proto::{frame, packet, security};
use sha2::Digest as _;
use std::collections::VecDeque;
use std::io::{self, Read, Write};

/// The plaintext a real unit would pick at random for the handshake. Fixed here
/// so a failing test is reproducible.
const HANDSHAKE_PLAINTEXT: [u8; 32] = [0x5C; 32];

/// How the fake should misbehave, for the error paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Misbehaviour {
    None,
    /// Return one byte per read, to exercise reassembly.
    Dribble,
    /// Claim a packet length far larger than anything real.
    BogusLength(u16),
    /// Announce a length and then hang up early.
    Truncate,
}

pub struct FakeUnit {
    key: [u8; 32],
    device_id: u64,
    session_key: Option<[u8; 32]>,
    outbox: VecDeque<u8>,
    misbehaviour: Misbehaviour,
    /// The state the unit currently holds, mutated by control commands.
    setpoint: Setpoint,
}

impl FakeUnit {
    pub fn new(key: [u8; 32], device_id: u64) -> Self {
        Self {
            key,
            device_id,
            session_key: None,
            outbox: VecDeque::new(),
            misbehaviour: Misbehaviour::None,
            setpoint: Setpoint {
                power_on: true,
                mode: Mode::Cool,
                target_temperature: 24.0,
                fan_speed: FanSpeed::AUTO,
                swing_mode: SwingMode::Off,
                eco: false,
                turbo: false,
                sleep: false,
                fahrenheit: false,
                purifier: false,
                aux_heat: false,
                independent_aux_heat: false,
                follow_me: false,
                freeze_protection: false,
                target_humidity: 40,
                beep: false,
            },
        }
    }

    pub fn dribbling(mut self) -> Self {
        self.misbehaviour = Misbehaviour::Dribble;
        self
    }

    pub fn with_bogus_length(mut self, len: u16) -> Self {
        self.misbehaviour = Misbehaviour::BogusLength(len);
        self
    }

    pub fn truncating(mut self) -> Self {
        self.misbehaviour = Misbehaviour::Truncate;
        self
    }

    /// The state report this unit would send for its current setpoint.
    ///
    /// Only the fields the tests read are populated; the rest stay zero, which is
    /// what a report with nothing else to say looks like anyway.
    fn state_payload(&self) -> Vec<u8> {
        let mut p = vec![0u8; 24];
        p[0] = 0xC0;
        p[1] = if self.setpoint.power_on { 0x01 } else { 0 };
        let integral = self.setpoint.target_temperature.trunc() as i32;
        let half = self.setpoint.target_temperature - integral as f32 > 0.0;
        // Report through the alternate field, as the real units here do.
        p[2] = ((self.setpoint.mode as u8) << 5) | if half { 0x10 } else { 0 };
        p[13] = ((integral - 12) as u8) & 0x1F;
        p[3] = self.setpoint.fan_speed.0;
        p[7] = self.setpoint.swing_mode as u8;
        if self.setpoint.eco {
            p[9] |= 0x10;
        }
        // No sensors fitted in the fake: 0xFF is the documented "absent" value.
        p[11] = 0xFF;
        p[12] = 0xFF;
        p
    }

    /// Handle one complete packet from the client and queue the reply.
    fn handle(&mut self, pkt: &[u8]) -> io::Result<()> {
        let flags = pkt[5];
        match flags & 0xF {
            x if x == PacketType::HandshakeRequest as u8 => {
                let encrypted = security::encrypt_aes_cbc(&self.key, &HANDSHAKE_PLAINTEXT)
                    .expect("32 bytes is block aligned");
                let mut payload = encrypted;
                payload.extend_from_slice(&sha2::Sha256::digest(HANDSHAKE_PLAINTEXT));

                // The client derives plaintext XOR key; so must we.
                let mut session = [0u8; 32];
                for i in 0..32 {
                    session[i] = HANDSHAKE_PLAINTEXT[i] ^ self.key[i];
                }
                self.session_key = Some(session);

                let mut reply = vec![0x83, 0x70];
                let len = match self.misbehaviour {
                    Misbehaviour::BogusLength(l) => l,
                    _ => payload.len() as u16,
                };
                reply.extend_from_slice(&len.to_be_bytes());
                reply.push(0x20);
                reply.push(PacketType::HandshakeResponse as u8);
                reply.extend_from_slice(&[0, 0]); // packet id
                if self.misbehaviour != Misbehaviour::Truncate {
                    reply.extend_from_slice(&payload);
                }
                self.outbox.extend(reply);
                Ok(())
            }
            x if x == PacketType::EncryptedRequest as u8 => {
                let key = self
                    .session_key
                    .ok_or_else(|| io::Error::other("request before handshake"))?;
                // A device reads requests, so it needs the direction-agnostic
                // decoder; `lan::decode` is for clients and refuses these.
                let (_, v2) = lan::decode_encrypted(pkt, &key)
                    .map_err(|e| io::Error::other(e.to_string()))?;
                let inner = packet::decode(&v2).map_err(io::Error::other)?;
                let payload = frame::parse(&inner, frame::DeviceType::AirConditioner)
                    .map_err(io::Error::other)?;

                match payload[0] {
                    // Query: report current state.
                    0x41 => {}
                    // Control: adopt the setpoint, then report it back.
                    0x40 => self.adopt(payload),
                    other => return Err(io::Error::other(format!("unknown opcode 0x{other:02X}"))),
                }

                let report = frame::build(
                    frame::DeviceType::AirConditioner,
                    frame::FrameType::Report,
                    &self.state_payload(),
                );
                let v2 = packet::encode(self.device_id, &report);
                let reply = lan::encode_encrypted(&key, 0, &v2, 0, PacketType::EncryptedResponse)
                    .map_err(io::Error::other)?;
                self.outbox.extend(reply);
                Ok(())
            }
            other => Err(io::Error::other(format!("unexpected type {other}"))),
        }
    }

    /// Apply a control payload to the fake's own state.
    fn adopt(&mut self, payload: &[u8]) {
        self.setpoint.power_on = payload[1] & 0x01 != 0;
        if let Some(m) = Mode::from_wire((payload[2] >> 5) & 0x7) {
            self.setpoint.mode = m;
        }
        let half = payload[2] & 0x10 != 0;
        let primary = payload[2] & 0xF;
        let alternate = payload[18] & 0x1F;
        let whole = if primary != 0 {
            primary as f32 + 16.0
        } else {
            alternate as f32 + 12.0
        };
        self.setpoint.target_temperature = whole + if half { 0.5 } else { 0.0 };
        self.setpoint.fan_speed = FanSpeed(payload[3] & 0x7F);
        if let Some(s) = SwingMode::from_wire(payload[7] & 0xF) {
            self.setpoint.swing_mode = s;
        }
        self.setpoint.eco = payload[9] & 0x80 != 0;
    }
}

impl Write for FakeUnit {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Sessions always write a whole packet in one call, so no buffering here.
        if buf.len() < 6 {
            return Err(io::Error::other("short write"));
        }
        self.handle(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for FakeUnit {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.outbox.is_empty() {
            // A real socket would block; a test wants a definite end.
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "fake unit has nothing more to say",
            ));
        }
        let want = if self.misbehaviour == Misbehaviour::Dribble {
            1
        } else {
            buf.len()
        };
        let n = want.min(buf.len()).min(self.outbox.len());
        for slot in buf.iter_mut().take(n) {
            *slot = self.outbox.pop_front().expect("checked non-empty");
        }
        Ok(n)
    }
}
