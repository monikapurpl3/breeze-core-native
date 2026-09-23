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
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    /// Queries and control commands answered, for tests that count round-trips.
    pub queries: usize,
    pub controls: usize,
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
            queries: 0,
            controls: 0,
        }
    }

    /// The setpoint the fake currently holds.
    pub fn target(&self) -> f32 {
        self.setpoint.target_temperature
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
                    0x41 => self.queries += 1,
                    // Control: adopt the setpoint, then report it back.
                    0x40 => {
                        self.controls += 1;
                        self.adopt(payload)
                    }
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

/// The in-process fake answers the instant it is written to, so there is never
/// anything to wait for: a bounded read is a plain read, and "nothing has
/// arrived yet" is simply an empty outbox.
impl crate::session::Link for FakeUnit {
    fn set_read_deadline(&mut self, _timeout: Duration) -> io::Result<()> {
        Ok(())
    }

    fn read_now(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.outbox.is_empty() {
            return Err(io::Error::new(io::ErrorKind::WouldBlock, "nothing waiting"));
        }
        self.read(buf)
    }
}

/// How a [`FakeServer`] misbehaves. Shared with the test, so it can be changed
/// between requests. Every knob is something measured on a real unit.
#[derive(Debug, Default)]
pub struct Behaviour {
    /// Swallow this many requests without processing or answering them -- a
    /// real unit does this now and then, and the connection stays usable.
    pub ignore: usize,
    /// Answer every request after this long. A real unit takes ~0.72 s.
    pub delay: Duration,
    /// Answer the next request after this long instead: set it past the
    /// client's reply timeout and the client resends, so two replies arrive.
    pub delay_next: Option<Duration>,
    /// Hang up after this long without a request. A real unit does at 30 s.
    pub idle_close: Option<Duration>,
}

/// A fake unit behind a real loopback socket, so the device layer runs against
/// it completely unchanged -- real connects, real timeouts, real hang-ups.
///
/// One unit, whatever the number of connections: its setpoint survives a
/// reconnect, as a real unit's does.
pub struct FakeServer {
    pub addr: SocketAddr,
    pub unit: Arc<Mutex<FakeUnit>>,
    pub behaviour: Arc<Mutex<Behaviour>>,
    /// Connections accepted, to tell a reused session from a reconnect.
    pub connections: Arc<Mutex<usize>>,
}

impl FakeServer {
    pub fn start(key: [u8; 32], device_id: u64) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let unit = Arc::new(Mutex::new(FakeUnit::new(key, device_id)));
        let behaviour = Arc::new(Mutex::new(Behaviour::default()));
        let connections = Arc::new(Mutex::new(0usize));
        {
            let (unit, behaviour, connections) = (
                Arc::clone(&unit),
                Arc::clone(&behaviour),
                Arc::clone(&connections),
            );
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    *connections.lock().unwrap() += 1;
                    let (unit, behaviour) = (Arc::clone(&unit), Arc::clone(&behaviour));
                    std::thread::spawn(move || serve(stream, &unit, &behaviour));
                }
            });
        }
        Self {
            addr,
            unit,
            behaviour,
            connections,
        }
    }

    pub fn behave(&self, f: impl FnOnce(&mut Behaviour)) {
        f(&mut self.behaviour.lock().unwrap());
    }

    pub fn controls(&self) -> usize {
        self.unit.lock().unwrap().controls
    }

    pub fn queries(&self) -> usize {
        self.unit.lock().unwrap().queries
    }

    pub fn target(&self) -> f32 {
        self.unit.lock().unwrap().target()
    }

    pub fn connections(&self) -> usize {
        *self.connections.lock().unwrap()
    }
}

/// One connection: read whole packets, answer them as the behaviour says.
fn serve(mut stream: TcpStream, unit: &Mutex<FakeUnit>, behaviour: &Mutex<Behaviour>) {
    let mut rx: Vec<u8> = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let idle = behaviour.lock().unwrap().idle_close;
        let _ = stream.set_read_timeout(idle);
        match stream.read(&mut buf) {
            // The client hung up, or the idle timer ran out: close, as a real
            // unit does, with a clean FIN.
            Ok(0) | Err(_) => return,
            Ok(n) => rx.extend_from_slice(&buf[..n]),
        }
        while let Some(total) = lan::packet_len(&rx) {
            if rx.len() < total {
                break;
            }
            let packet: Vec<u8> = rx.drain(..total).collect();
            let is_request = packet[5] & 0xF == PacketType::EncryptedRequest as u8;
            let wait = {
                let mut b = behaviour.lock().unwrap();
                if is_request && b.ignore > 0 {
                    b.ignore -= 1;
                    continue;
                }
                if is_request {
                    b.delay_next.take().unwrap_or(b.delay)
                } else {
                    Duration::ZERO
                }
            };
            let reply: Vec<u8> = {
                let mut u = unit.lock().unwrap();
                if u.handle(&packet).is_err() {
                    return;
                }
                u.outbox.drain(..).collect()
            };
            if !wait.is_zero() {
                std::thread::sleep(wait);
            }
            if stream.write_all(&reply).is_err() {
                return;
            }
        }
    }
}
