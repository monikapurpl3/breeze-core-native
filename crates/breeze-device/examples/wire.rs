//! Watch one unit's connection packet by packet: what arrives, when, and
//! whether anything asked for it.
//!
//! Written to test what 4.0.2's latency and the slider that springs back were
//! made of, against real hardware rather than a theory of it:
//!
//! 1. whether a unit sends packets nobody asked for -- a reader that takes "the
//!    next packet" to be "the reply" would mis-attribute them;
//! 2. whether a request sent right after the previous reply is answered at
//!    all -- the control path is the only one that sends two back to back;
//! 3. with `--write` only: the same for a real setpoint change, restored
//!    about three seconds later;
//! 4. how an idle connection dies (FIN, RST or silence) and after how long.
//!
//! ```text
//! wire <config.json> <unit-name-prefix> [--write] [--idle 10,30,60,120,300]
//! ```
//!
//! Talks to exactly one unit. Reads `config.json` itself so the V3 token and
//! key never leave the host, and never prints either.

use breeze_proto::ac::command::{self, Setpoint};
use breeze_proto::ac::response::State;
use breeze_proto::ac::types::TemperatureType;
use breeze_proto::{frame, lan, packet};
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

fn hex(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

struct Unit {
    id: u64,
    name: String,
    addr: SocketAddr,
    token: Vec<u8>,
    key: [u8; 32],
}

/// What arrived on the socket.
enum Seen {
    Packet(Vec<u8>),
    Eof,
    Error(ErrorKind),
}

struct Conn {
    stream: TcpStream,
    session_key: [u8; 32],
    packet_id: u16,
    buf: Vec<u8>,
    /// When the last request went out, so every packet is timed from it.
    sent_at: Option<Instant>,
    /// Packets received since the last request: 1 is the reply, >1 is news.
    since_send: u32,
    dead: bool,
}

struct Clock(Instant);
impl Clock {
    fn stamp(&self) -> String {
        format!("[{:8.3}]", self.0.elapsed().as_secs_f64())
    }
}

fn msg_id() -> u8 {
    use std::sync::atomic::{AtomicU8, Ordering};
    static N: AtomicU8 = AtomicU8::new(1);
    N.fetch_add(1, Ordering::Relaxed)
}

impl Conn {
    fn open(u: &Unit, clock: &Clock) -> Result<Conn, String> {
        let t = Instant::now();
        let mut stream = TcpStream::connect_timeout(&u.addr, Duration::from_secs(5))
            .map_err(|e| format!("connect: {e}"))?;
        stream.set_nodelay(true).ok();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;
        let connected = t.elapsed();

        stream
            .write_all(&lan::encode_handshake(1, &u.token))
            .map_err(|e| format!("handshake write: {e}"))?;
        let mut head = [0u8; 6];
        stream
            .read_exact(&mut head)
            .map_err(|e| format!("handshake read: {e}"))?;
        let total = lan::packet_len(&head).ok_or("handshake: bad header")?;
        let mut p = head.to_vec();
        p.resize(total, 0);
        stream
            .read_exact(&mut p[6..])
            .map_err(|e| format!("handshake read: {e}"))?;
        let session_key = match lan::decode(&p, None).map_err(|e| format!("{e:?}"))? {
            lan::Decoded::Handshake(payload) => lan::derive_session_key(&u.key, &payload)
                .map_err(|e| format!("session key: {e:?}"))?,
            _ => return Err("handshake: unexpected packet type".into()),
        };
        println!(
            "{} connected in {} ms, handshake done in {} ms",
            clock.stamp(),
            connected.as_millis(),
            t.elapsed().as_millis()
        );
        Ok(Conn {
            stream,
            session_key,
            packet_id: 2,
            buf: Vec::new(),
            sent_at: None,
            since_send: 0,
            dead: false,
        })
    }

    fn send(&mut self, u: &Unit, what: &str, frame_bytes: &[u8], clock: &Clock) -> bool {
        let pkt = packet::encode(u.id, frame_bytes);
        let req = match lan::encode_request(&self.session_key, self.packet_id, &pkt, 0) {
            Ok(r) => r,
            Err(e) => {
                println!("{} !! encode: {e:?}", clock.stamp());
                return false;
            }
        };
        self.packet_id = self.packet_id.wrapping_add(1) & 0xFFF;
        match self.stream.write_all(&req) {
            Ok(()) => {
                self.sent_at = Some(Instant::now());
                self.since_send = 0;
                println!(
                    "{} -> {what}  [pkt id {:03x}]",
                    clock.stamp(),
                    (self.packet_id.wrapping_sub(1)) & 0xFFF
                );
                true
            }
            Err(e) => {
                println!("{} !! write failed: {:?} ({e})", clock.stamp(), e.kind());
                self.dead = true;
                false
            }
        }
    }

    /// Read whatever arrives for `dur`, logging each packet as it lands.
    /// Returns the states decoded, in arrival order.
    fn watch(&mut self, dur: Duration, clock: &Clock) -> Vec<State> {
        let deadline = Instant::now() + dur;
        let mut states = Vec::new();
        let mut tmp = [0u8; 2048];
        while !self.dead {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let wait = (deadline - now).max(Duration::from_millis(1));
            self.stream.set_read_timeout(Some(wait)).ok();
            match self.stream.read(&mut tmp) {
                Ok(0) => self.report(Seen::Eof, clock, &mut states),
                Ok(n) => {
                    self.buf.extend_from_slice(&tmp[..n]);
                    while self.buf.len() >= 6 {
                        let Some(total) = lan::packet_len(&self.buf[..6]) else {
                            println!("{} !! unframeable bytes, dropping buffer", clock.stamp());
                            self.buf.clear();
                            break;
                        };
                        if total < 6 || self.buf.len() < total {
                            break;
                        }
                        let p: Vec<u8> = self.buf.drain(..total).collect();
                        self.report(Seen::Packet(p), clock, &mut states);
                    }
                }
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(e) => self.report(Seen::Error(e.kind()), clock, &mut states),
            }
        }
        states
    }

    fn report(&mut self, seen: Seen, clock: &Clock, states: &mut Vec<State>) {
        match seen {
            Seen::Eof => {
                println!("{} !! peer closed the connection (FIN)", clock.stamp());
                self.dead = true;
            }
            Seen::Error(k) => {
                println!("{} !! read error: {k:?}", clock.stamp());
                self.dead = true;
            }
            Seen::Packet(p) => {
                self.since_send += 1;
                let delay = self
                    .sent_at
                    .map(|t| format!("+{:>5} ms", t.elapsed().as_millis()))
                    .unwrap_or_else(|| "(unprompted)".into());
                let role = match (self.sent_at.is_some(), self.since_send) {
                    (false, _) => "UNPROMPTED",
                    (true, 1) => "reply     ",
                    (true, _) => "EXTRA     ",
                };
                // The packet id sits in the first two decrypted bytes, which
                // `lan::decode` strips. Read it directly: whether the unit
                // echoes ours decides how exactly a stale reply can be refused.
                let reply_id = if p.len() >= 6 + 32 {
                    breeze_proto::security::decrypt_aes_cbc(&self.session_key, &p[6..p.len() - 32])
                        .ok()
                        .filter(|plain| plain.len() >= 2)
                        .map(|plain| {
                            format!("{:03x}", u16::from_be_bytes([plain[0], plain[1]]) & 0xFFF)
                        })
                        .unwrap_or_else(|| "???".into())
                } else {
                    "---".into()
                };
                let delay = format!("{delay} [pkt id {reply_id}]");
                let inner = match lan::decode(&p, Some(&self.session_key)) {
                    Ok(lan::Decoded::Response(v2)) => v2,
                    Ok(_) => {
                        println!(
                            "{} <- {role} {delay}  (handshake-type packet)",
                            clock.stamp()
                        );
                        return;
                    }
                    Err(e) => {
                        println!("{} <- {role} {delay}  undecodable: {e:?}", clock.stamp());
                        return;
                    }
                };
                let f = match packet::decode(&inner) {
                    Ok(f) => f,
                    Err(e) => {
                        println!("{} <- {role} {delay}  bad packet: {e:?}", clock.stamp());
                        return;
                    }
                };
                let ftype = f.get(9).copied().unwrap_or(0);
                let (b0, tail, st) = match frame::parse(&f, frame::DeviceType::AirConditioner) {
                    Ok(b) => {
                        let tail: Vec<String> = b
                            .iter()
                            .skip(b.len().saturating_sub(2))
                            .map(|x| format!("{x:02x}"))
                            .collect();
                        (
                            b.first().copied().unwrap_or(0),
                            tail.join(" "),
                            State::parse(b).ok(),
                        )
                    }
                    Err(e) => {
                        println!(
                            "{} <- {role} {delay}  frame 0x{ftype:02x}  unparseable: {e:?}",
                            clock.stamp()
                        );
                        return;
                    }
                };
                let summary = st
                    .as_ref()
                    .map(|s| {
                        format!(
                            "power {} mode {:?} target {} swing {:?} (raw {:#x}) indoor {:?}",
                            if s.power_on { "on " } else { "off" },
                            s.mode,
                            s.target_temperature,
                            s.swing_mode,
                            s.swing_raw,
                            s.indoor_temperature
                        )
                    })
                    .unwrap_or_default();
                println!(
                    "{} <- {role} {delay}  frame 0x{ftype:02x} body 0x{b0:02x} tail [{tail}]  {summary}",
                    clock.stamp()
                );
                if let Some(s) = st {
                    states.push(s);
                }
            }
        }
    }
}

fn get(conn: &mut Conn, u: &Unit, clock: &Clock) -> bool {
    let id = msg_id();
    conn.send(
        u,
        &format!("GET state        (msg {id:02x})"),
        &command::get_state(id, TemperatureType::Indoor),
        clock,
    )
}

/// Wait for the reply to the last request, up to `limit`. True if one came.
fn await_reply(conn: &mut Conn, limit: Duration, clock: &Clock) -> Vec<State> {
    let start = Instant::now();
    let mut got = Vec::new();
    while conn.since_send == 0 && !conn.dead && start.elapsed() < limit {
        got.extend(conn.watch(Duration::from_millis(20), clock));
    }
    got
}

fn reopen(unit: &Unit, clock: &Clock) -> Conn {
    println!("{} .. reconnecting", clock.stamp());
    let c = Conn::open(unit, clock).unwrap_or_else(|e| {
        println!("!! reconnect failed: {e}");
        std::process::exit(1)
    });
    let mut c = c;
    c.watch(Duration::from_millis(1000), clock);
    c
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: wire <config.json> <unit-name-prefix> [--write] [--idle 10,30,60]");
        std::process::exit(2);
    }
    let write = args.iter().any(|a| a == "--write");
    let idle: Vec<u64> = args
        .iter()
        .position(|a| a == "--idle")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect())
        .unwrap_or_else(|| vec![10, 30, 60, 120, 300]);

    let raw = std::fs::read_to_string(&args[1]).unwrap_or_else(|e| {
        eprintln!("cannot read {}: {e}", args[1]);
        std::process::exit(1)
    });
    let cfg: serde_json::Value = serde_json::from_str(&raw).expect("config.json is not JSON");
    let prefix = args[2].to_lowercase();
    let all = cfg["units"].as_array().cloned().unwrap_or_default();
    let matches: Vec<&serde_json::Value> = all
        .iter()
        .filter(|u| {
            u["name"]
                .as_str()
                .map(|n| n.to_lowercase().starts_with(&prefix))
                .unwrap_or(false)
        })
        .collect();
    if matches.len() != 1 {
        eprintln!(
            "'{}' matches {} units; need exactly one",
            args[2],
            matches.len()
        );
        std::process::exit(1);
    }
    let j = matches[0];
    let ip: IpAddr = j["ip"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .expect("no ip");
    let unit = Unit {
        id: j["id"]
            .as_u64()
            .or_else(|| j["id"].as_str().and_then(|s| s.parse().ok()))
            .expect("no id"),
        name: j["name"].as_str().unwrap_or("?").into(),
        addr: SocketAddr::new(ip, j["port"].as_u64().unwrap_or(6444) as u16),
        token: j["token"].as_str().and_then(hex).expect("no token"),
        key: j["key"]
            .as_str()
            .and_then(hex)
            .and_then(|v| <[u8; 32]>::try_from(v.as_slice()).ok())
            .expect("no key"),
    };
    let clock = Clock(Instant::now());
    println!(
        "unit: {} at {}  (write test: {})",
        unit.name,
        unit.addr,
        if write { "ON" } else { "off" }
    );

    // --- 1. connect, and watch the settle window the protocol demands -------
    let mut conn = Conn::open(&unit, &clock).expect("could not open a session");
    println!(
        "{} .. watching the 1 s post-handshake settle",
        clock.stamp()
    );
    conn.watch(Duration::from_millis(1000), &clock);

    // --- 2. one read, then 3 s for anything that follows it ------------------
    println!("\n== one read, then 3 s for extras");
    get(&mut conn, &unit, &clock);
    conn.watch(Duration::from_secs(3), &clock);

    // `--quick`: one read and its aftermath, nothing else. For comparing what
    // the unit says on a fresh connection with what the server is reporting.
    if args.iter().any(|a| a == "--quick") {
        println!("
{} done (quick)", clock.stamp());
        return;
    }

    // --- 3. back to back: is a request right after a reply answered? --------
    println!("\n== back-to-back reads: a second read N ms after the first reply");
    for gap_ms in [0u64, 50, 150, 300, 600] {
        for trial in 1..=3 {
            if conn.dead {
                conn = reopen(&unit, &clock);
            }
            get(&mut conn, &unit, &clock);
            await_reply(&mut conn, Duration::from_secs(3), &clock);
            if conn.since_send == 0 {
                println!(
                    "{} !! first read unanswered in 3 s (gap {gap_ms}, trial {trial})",
                    clock.stamp()
                );
                conn.watch(Duration::from_secs(3), &clock);
                continue;
            }
            std::thread::sleep(Duration::from_millis(gap_ms));
            get(&mut conn, &unit, &clock);
            await_reply(&mut conn, Duration::from_secs(3), &clock);
            println!(
                "{} .. gap {:>3} ms trial {trial}: second read {}",
                clock.stamp(),
                gap_ms,
                if conn.since_send > 0 {
                    "ANSWERED"
                } else {
                    "NOT ANSWERED within 3 s"
                }
            );
            // Anything late or extra shows up here rather than in the next trial.
            conn.watch(Duration::from_millis(1500), &clock);
        }
    }

    // --- 4. the write test, only when asked for -----------------------------
    if write {
        println!(
            "\n== write test: read, SET +0.5 at once (as 4.0.2's control does), restore ~3 s later"
        );
        if conn.dead {
            conn = reopen(&unit, &clock);
        }
        get(&mut conn, &unit, &clock);
        let got = await_reply(&mut conn, Duration::from_secs(3), &clock);
        let Some(original) = got.last().cloned() else {
            println!("!! could not read the unit; not writing anything");
            std::process::exit(1);
        };
        conn.watch(Duration::from_millis(1500), &clock);

        let o = original.target_temperature;
        let mut nudged = Setpoint::from_state(&original);
        nudged.beep = false;
        nudged.target_temperature = if o <= 29.5 { o + 0.5 } else { o - 0.5 };
        let mut restore = Setpoint::from_state(&original);
        restore.beep = false;

        // Read, then SET the instant the reply lands: 4.0.2's control path.
        get(&mut conn, &unit, &clock);
        await_reply(&mut conn, Duration::from_secs(3), &clock);
        let changed_at = Instant::now();
        let id = msg_id();
        conn.send(
            &unit,
            &format!("SET target {} (msg {id:02x})", nudged.target_temperature),
            &command::set_state(id, &nudged),
            &clock,
        );
        conn.watch(Duration::from_millis(2800), &clock);

        if conn.dead {
            conn = reopen(&unit, &clock);
        }
        let id = msg_id();
        conn.send(
            &unit,
            &format!("SET target {o} (msg {id:02x}) RESTORE"),
            &command::set_state(id, &restore),
            &clock,
        );
        println!(
            "{} .. setpoint was off its value for {:.1} s",
            clock.stamp(),
            changed_at.elapsed().as_secs_f64()
        );
        conn.watch(Duration::from_millis(2500), &clock);

        // Verify, and keep restoring until it reads back right (bounded).
        for attempt in 1..=3 {
            if conn.dead {
                conn = reopen(&unit, &clock);
            }
            get(&mut conn, &unit, &clock);
            let seen = conn.watch(Duration::from_millis(2000), &clock);
            match seen.last() {
                Some(s) if (s.target_temperature - o).abs() < 0.01 => {
                    println!("{} .. verified: target back at {o}", clock.stamp());
                    break;
                }
                other => {
                    println!(
                        "{} !! read back {:?}, expected {o} -- restoring again (attempt {attempt})",
                        clock.stamp(),
                        other.map(|s| s.target_temperature)
                    );
                    let id = msg_id();
                    conn.send(
                        &unit,
                        &format!("SET target {o} (msg {id:02x}) RESTORE"),
                        &command::set_state(id, &restore),
                        &clock,
                    );
                    conn.watch(Duration::from_millis(1500), &clock);
                }
            }
        }
    }

    // --- 5. idle ladder: how long does an unused connection live? -----------
    println!("\n== idle ladder: {idle:?} s");
    for secs in idle {
        if conn.dead {
            conn = reopen(&unit, &clock);
        }
        println!(
            "{} .. idle for {secs} s, logging anything that arrives",
            clock.stamp()
        );
        conn.sent_at = None;
        conn.watch(Duration::from_secs(secs), &clock);
        if conn.dead {
            println!("{} == died while idle, within {secs} s", clock.stamp());
            continue;
        }
        get(&mut conn, &unit, &clock);
        await_reply(&mut conn, Duration::from_secs(5), &clock);
        let verdict = if conn.since_send > 0 {
            "still alive"
        } else if conn.dead {
            "dead, noticed on use"
        } else {
            "SILENT -- no reply in 5 s, no FIN, no RST"
        };
        println!("{} == after {secs} s idle: {verdict}", clock.stamp());
        if conn.since_send == 0 {
            conn.dead = true;
        } else {
            conn.watch(Duration::from_millis(1500), &clock);
        }
    }
    println!("\n{} done", clock.stamp());
}
