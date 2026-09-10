//! `GET /api/units/stream` — live state over Server-Sent Events.
//!
//! Midea units have no push, so this does not remove polling: it **centralises**
//! it. One background thread refreshes every unit and fans changes out to all
//! connected clients, instead of each client — app, web panel, widgets — polling
//! on its own. A phone's radio stops waking up for it, and a change made
//! elsewhere (a schedule firing, someone else's button) reaches every stream on
//! the next tick.
//!
//! The poller is lazy: it sleeps while nobody is subscribed and only touches the
//! LAN once at least one client is listening. That matters on a household
//! network — an idle server should be silent, not talking to three air
//! conditioners every five seconds forever.
//!
//! **This is the one route that does not go through [`Reply`].** An SSE response
//! never ends, so it cannot be a buffer that gets handed to `respond()`. The
//! connection is hijacked instead: the request is turned into a raw writer and
//! moved to its own thread, which writes chunks and flushes each one. Two
//! consequences worth knowing:
//!
//! * a stream must **not** occupy a worker from the fixed pool — eight open
//!   streams would otherwise starve the whole API — hence the dedicated thread;
//! * the headers are written by hand here, so the security header set has to be
//!   kept in step with [`Reply::into_http`] deliberately rather than inherited.
//!
//! [`Reply`]: crate::respond::Reply
//! [`Reply::into_http`]: crate::respond::Reply::into_http

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::state::AppState;
use crate::units::UnitState;

/// How many events one slow client may fall behind before it starts losing them.
///
/// Matching the reference's queue size. A client this far behind is stuck, and
/// dropping its events is the only option that does not punish everyone else:
/// the alternative is blocking the poller, which stalls every other stream.
const QUEUE_DEPTH: usize = 200;

/// How long to wait for an event before sending a keepalive comment.
///
/// The keepalive is not decoration. It is the only way this server learns that a
/// client has gone away: the write fails, and the stream ends. Without it a
/// vanished phone would hold a thread until the unit's state happened to change.
const KEEPALIVE: Duration = Duration::from_secs(15);

/// Ceiling on concurrent streams.
///
/// A divergence from the reference, which is async and can afford not to care.
/// Here each stream is an OS thread, so a client bug that reconnects without
/// closing would grow threads without limit. Sixty-four is far above any real
/// household and far below anything that hurts.
pub const MAX_STREAMS: usize = 64;

/// `(event type, payload)`. The type is `state` today; the frame format allows
/// for more without a client change.
pub type Event = (&'static str, serde_json::Value);

struct Subscriber {
    id: u64,
    tx: SyncSender<Event>,
}

/// The central poller and its subscribers.
pub struct StateStream {
    subs: Mutex<Vec<Subscriber>>,
    /// Last state per unit, and the canonical form used to detect a change.
    last: Mutex<HashMap<u64, (String, serde_json::Value)>>,
    /// Signals the idle poller that someone has subscribed.
    wake: Condvar,
    /// Guards nothing but the condvar wait; the subscriber count is read from
    /// `subs` each time round.
    wake_lock: Mutex<()>,
    next_id: AtomicU64,
    tick: Duration,
}

impl StateStream {
    pub fn new(tick_seconds: u64) -> Self {
        Self {
            subs: Mutex::new(Vec::new()),
            last: Mutex::new(HashMap::new()),
            wake: Condvar::new(),
            wake_lock: Mutex::new(()),
            next_id: AtomicU64::new(1),
            // A one-second floor, as the reference clamps it: a zero tick would
            // hammer the units as fast as the LAN allows.
            tick: Duration::from_secs(tick_seconds.max(1)),
        }
    }

    pub fn subscriber_count(&self) -> usize {
        self.subs.lock().map(|s| s.len()).unwrap_or(0)
    }

    pub fn tick_seconds(&self) -> u64 {
        self.tick.as_secs()
    }

    /// Register a listener. Returns `None` when [`MAX_STREAMS`] is already open.
    fn subscribe(&self) -> Option<(u64, Receiver<Event>)> {
        let (tx, rx) = sync_channel(QUEUE_DEPTH);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut subs = self.subs.lock().ok()?;
            if subs.len() >= MAX_STREAMS {
                return None;
            }
            subs.push(Subscriber { id, tx });
        }
        // Nudge the poller, which is asleep if this is the first subscriber.
        let _guard = self.wake_lock.lock();
        self.wake.notify_all();
        Some((id, rx))
    }

    fn unsubscribe(&self, id: u64) {
        if let Ok(mut subs) = self.subs.lock() {
            subs.retain(|s| s.id != id);
        }
    }

    /// Last-known state for every unit, so a client that has just connected does
    /// not stare at an empty panel for a whole tick.
    fn snapshot(&self, order: &[u64]) -> Vec<Event> {
        let last = match self.last.lock() {
            Ok(l) => l,
            Err(_) => return Vec::new(),
        };
        // Configuration order, not map order: a panel that shuffles its cards
        // between reconnects is worse than one that is briefly empty.
        order
            .iter()
            .filter_map(|id| last.get(id).map(|(_, v)| ("state", v.clone())))
            .collect()
    }

    fn broadcast(&self, event: Event) {
        let Ok(subs) = self.subs.lock() else { return };
        for sub in subs.iter() {
            match sub.tx.try_send(event.clone()) {
                Ok(()) => {}
                // Full: this client is stuck. Drop the event rather than block
                // the poller and every other stream behind it.
                Err(TrySendError::Full(_)) => {}
                // Disconnected: its thread has gone. The stream thread removes
                // itself on the way out, so there is nothing to do here.
                Err(TrySendError::Disconnected(_)) => {}
            }
        }
    }
}

/// Start the central poller.
///
/// Idles on a condvar while no one is subscribed, so a server nobody is watching
/// makes no LAN traffic at all.
pub fn spawn_poller(state: Arc<AppState>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || loop {
        if state.stream.subscriber_count() == 0 {
            let Ok(guard) = state.stream.wake_lock.lock() else {
                return;
            };
            // Re-check under the lock, then wait. A subscriber arriving between
            // the check above and this wait would otherwise be missed until the
            // next event -- which, with nobody polling, would never come.
            if state.stream.subscriber_count() == 0 {
                let _unused = state.stream.wake.wait(guard);
            }
            continue;
        }
        poll_once(&state);
        std::thread::sleep(state.stream.tick);
    })
}

/// One refresh of every unit, broadcasting whatever changed.
///
/// Units are contacted `BREEZE_BG_WORKERS` at a time, and the default of 1 is
/// the plain sequential walk every release before 4.0.2 did. Each unit costs a
/// LAN round-trip of roughly a second, so past a handful of them the walk
/// outlasts `AC_STREAM_TICK` and every tick starts later than the last; fanning
/// out fixes that without shortening the tick, which would only add traffic.
pub fn poll_once(state: &AppState) {
    let units = state.manager.known_units();
    let lanes = lane_count(units.len(), state.settings.bg_workers);

    if lanes <= 1 {
        for id in units {
            poll_one(state, id);
        }
        return;
    }

    // Scoped threads, so each lane borrows &AppState directly instead of
    // needing an Arc: the scope cannot outlive this call, which is the
    // guarantee that makes the borrow sound. Lanes claim ids by index modulo
    // lane count -- even, and with no shared queue to lock, which is worth
    // having when every item costs about the same and there is nothing to steal.
    std::thread::scope(|scope| {
        let units = &units;
        for lane in 0..lanes {
            scope.spawn(move || {
                for (i, id) in units.iter().enumerate() {
                    if i % lanes == lane {
                        poll_one(state, *id);
                    }
                }
            });
        }
    });
}

/// How many lanes to actually run, given the units present and what was asked
/// for.
///
/// Clamped at both ends, and the lower clamp is load-bearing rather than
/// defensive: lanes is the modulus in `i % lanes`, so a configured `0` would
/// divide by zero and take the poller thread down with it — leaving a server
/// that answers every request but never streams an update again.
fn lane_count(units: usize, requested: usize) -> usize {
    // Never more lanes than units: eight threads for three units is five
    // threads that exist to do nothing.
    requested.max(1).min(units.max(1))
}

/// Read one unit, record it, and broadcast it if it moved.
///
/// Extracted so the sequential and fanned-out paths cannot drift: both call
/// exactly this, and every shared structure it touches (`last`, `history`, the
/// subscriber list) was already behind its own lock because the HTTP workers
/// reach them too.
fn poll_one(state: &AppState, id: u64) {
    let value = read_unit(state, id);
    // Canonical form for change detection only. `serde_json` sorts object
    // keys, which is exactly the reference's `json.dumps(..., sort_keys=True)`
    // -- the string itself is never sent anywhere.
    let canonical = serde_json::to_string(&value).unwrap_or_default();

    let changed = {
        let Ok(mut last) = state.stream.last.lock() else {
            return;
        };
        match last.get(&id) {
            Some((previous, _)) if *previous == canonical => false,
            _ => {
                last.insert(id, (canonical, value.clone()));
                true
            }
        }
    };
    // Recorded every tick, changed or not: a flat line is data, and this is
    // the only sampler that runs on a clock rather than when somebody asks.
    state.history.record(&value, crate::history::now_unix());

    // Broadcast only on change: a unit whose temperature has not moved is
    // not news, and waking every client every tick is what this feature
    // exists to stop.
    if changed {
        state.stream.broadcast(("state", value));
    }
}

/// Refresh one unit, falling back the way the reference does.
///
/// A unit that will not answer keeps its **last known values** with `online`
/// flipped to false, rather than becoming an error record — clients render this
/// straight into a card, so it has to be a complete state either way. A unit
/// never seen at all gets the reference's synthetic defaults.
fn read_unit(state: &AppState, id: u64) -> serde_json::Value {
    let live = state.manager.with_unit(id, |device| {
        let s = device.refresh()?;
        Ok(serde_json::to_value(UnitState::from_device(device, &s))
            .unwrap_or_else(|_| serde_json::json!({})))
    });
    match live {
        Ok(value) => value,
        // Unreachable, or the manager could not give us the unit at all.
        Err(_) => offline_value(state, id),
    }
}

/// The state to report for a unit we could not read.
fn offline_value(state: &AppState, id: u64) -> serde_json::Value {
    if let Ok(last) = state.stream.last.lock() {
        if let Some((_, previous)) = last.get(&id) {
            let mut value = previous.clone();
            if let Some(obj) = value.as_object_mut() {
                obj.insert("online".into(), serde_json::Value::Bool(false));
            }
            return value;
        }
    }
    // Never seen: the reference's placeholder, field for field, so a client
    // rendering it sees a plausible card rather than a hole.
    let (name, ip) = state
        .config
        .read()
        .ok()
        .and_then(|c| {
            c.units
                .iter()
                .find(|u| u.id == id as i64)
                .map(|u| (u.name.clone(), u.ip.clone()))
        })
        .unwrap_or_default();
    serde_json::json!({
        "id": id.to_string(),
        "name": name,
        "ip": ip,
        "online": false,
        "power_state": false,
        "operational_mode": "AUTO",
        "target_temperature": 22.0,
        "indoor_temperature": serde_json::Value::Null,
        "outdoor_temperature": serde_json::Value::Null,
        "fan_speed": 102,
        "swing_mode": "OFF",
        "eco": false,
        "turbo": false,
    })
}

/// Take over the connection and stream to it until the client goes away.
///
/// Runs on its own thread so it cannot consume one of the fixed worker pool.
pub fn hijack(state: Arc<AppState>, request: tiny_http::Request) {
    std::thread::spawn(move || {
        let order = state.manager.known_units();
        let Some((id, rx)) = state.stream.subscribe() else {
            // At the ceiling. Answer properly rather than dropping the socket,
            // so a client can tell "too many" from "server gone".
            let _ = request.respond(
                crate::respond::Reply::detail(503, "too many open streams")
                    .into_http(state.settings.security_headers),
            );
            return;
        };

        let mut writer = request.into_writer();
        if write_headers(&mut writer, state.settings.security_headers).is_err() {
            state.stream.unsubscribe(id);
            return;
        }

        // An immediate byte, so the client -- and any proxy in between -- can see
        // the stream is live without waiting for the first tick.
        let mut alive = write_chunk(&mut writer, ": connected\n\n").is_ok();

        if alive {
            for (kind, data) in state.stream.snapshot(&order) {
                if write_chunk(&mut writer, &frame(kind, &data)).is_err() {
                    alive = false;
                    break;
                }
            }
        }

        while alive {
            match rx.recv_timeout(KEEPALIVE) {
                Ok((kind, data)) => {
                    alive = write_chunk(&mut writer, &frame(kind, &data)).is_ok();
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // The disconnect detector: this write fails once the client
                    // has gone, which is how the thread and subscription end.
                    alive = write_chunk(&mut writer, ": keepalive\n\n").is_ok();
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        // Always, however we left the loop -- otherwise the poller keeps running
        // for a client that is no longer there.
        state.stream.unsubscribe(id);
        let _ = write_terminator(&mut writer);
    });
}

/// One SSE frame.
pub fn frame(kind: &str, data: &serde_json::Value) -> String {
    // `data:` must be one line; serde_json never emits a bare newline inside a
    // compact document, so this stays a single field.
    format!(
        "event: {kind}\ndata: {}\n\n",
        serde_json::to_string(data).unwrap_or_else(|_| "{}".into())
    )
}

/// The status line and headers, written by hand because this response is never
/// a [`Reply`].
///
/// `Transfer-Encoding: chunked` rather than a length, which is what makes an
/// endless body legal HTTP/1.1.
fn write_headers(writer: &mut dyn Write, security: bool) -> std::io::Result<()> {
    let mut head = String::from(concat!(
        "HTTP/1.1 200 OK\r\n",
        "Content-Type: text/event-stream\r\n",
        "Transfer-Encoding: chunked\r\n",
        "Cache-Control: no-cache, no-transform\r\n",
        // Asks nginx not to buffer, without which a reverse proxy holds every
        // event until its buffer fills and the stream appears dead.
        "X-Accel-Buffering: no\r\n",
        "Connection: keep-alive\r\n",
        // Pre-set so no compressing proxy touches the stream: a brotli
        // event-stream is garbage to a client that cannot decode it, which
        // includes Dart's http package.
        "Content-Encoding: identity\r\n",
    ));
    // Taken from the shared list rather than written out again, so a header
    // added for every other route cannot quietly skip this one.
    for (name, value) in crate::respond::SECURITY_HEADERS.iter().filter(|_| security) {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    writer.write_all(head.as_bytes())?;
    writer.flush()
}

/// One chunk, flushed immediately.
///
/// The flush is the whole point: a buffered SSE frame is an event the client
/// does not have.
fn write_chunk(writer: &mut dyn Write, payload: &str) -> std::io::Result<()> {
    write!(writer, "{:x}\r\n", payload.len())?;
    writer.write_all(payload.as_bytes())?;
    writer.write_all(b"\r\n")?;
    writer.flush()
}

fn write_terminator(writer: &mut dyn Write) -> std::io::Result<()> {
    writer.write_all(b"0\r\n\r\n")?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_one_event_and_one_data_line() {
        let f = frame("state", &serde_json::json!({"id": "7", "online": true}));
        assert!(f.starts_with("event: state\n"));
        assert!(f.ends_with("\n\n"), "a frame is terminated by a blank line");
        let lines: Vec<&str> = f.trim_end().split('\n').collect();
        assert_eq!(lines.len(), 2, "exactly one event line and one data line");
        assert!(lines[1].starts_with("data: {"));
        // No raw newline inside the payload, which would split the data field
        // in two and give the client half an event.
        assert_eq!(
            f.matches('\n').count(),
            3,
            "one after the event line, one after data, one blank"
        );
    }

    #[test]
    fn chunks_carry_a_hex_length_and_flush() {
        let mut out = Vec::new();
        write_chunk(&mut out, ": connected\n\n").unwrap();
        // 13 bytes -> "d".
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "d\r\n: connected\n\n\r\n",
            "length must be hex, not decimal"
        );
    }

    #[test]
    fn the_terminator_ends_the_chunked_body() {
        let mut out = Vec::new();
        write_terminator(&mut out).unwrap();
        assert_eq!(out, b"0\r\n\r\n");
    }

    #[test]
    fn the_headers_say_event_stream_and_do_not_promise_a_length() {
        let mut out = Vec::new();
        write_headers(&mut out, true).unwrap();
        let head = String::from_utf8(out).unwrap();
        assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(head.contains("Content-Type: text/event-stream\r\n"));
        assert!(head.contains("Transfer-Encoding: chunked\r\n"));
        assert!(
            !head.to_lowercase().contains("content-length"),
            "an endless body must not claim a length"
        );
        assert!(
            head.contains("Content-Encoding: identity\r\n"),
            "a compressing proxy would corrupt the stream"
        );
        assert!(head.contains("X-Accel-Buffering: no\r\n"));
        assert!(head.ends_with("\r\n\r\n"), "headers end with a blank line");
    }

    #[test]
    fn the_stream_sends_the_same_security_headers_as_every_other_route() {
        // The status line and headers are written by hand here, so this is the
        // check that a header added to SECURITY_HEADERS reaches the one
        // long-lived route too.
        let mut out = Vec::new();
        write_headers(&mut out, true).unwrap();
        let head = String::from_utf8(out).unwrap();
        assert!(!crate::respond::SECURITY_HEADERS.is_empty());
        for (name, value) in crate::respond::SECURITY_HEADERS {
            assert!(
                head.contains(&format!("{name}: {value}\r\n")),
                "{name} missing from the stream headers"
            );
        }
        // And no stray newline snuck into a header value, which would end the
        // header block early and truncate the rest.
        assert!(
            !head.contains("\r\n\r\n\r\n") && head.matches("\r\n\r\n").count() == 1,
            "exactly one blank line, at the end: {head:?}"
        );
    }

    #[test]
    fn a_slow_client_loses_events_rather_than_blocking_the_poller() {
        let stream = StateStream::new(5);
        let (id, rx) = stream.subscribe().unwrap();
        // Fill the queue past its depth without reading a single event.
        for i in 0..(QUEUE_DEPTH + 50) {
            stream.broadcast(("state", serde_json::json!({"n": i})));
        }
        // Still exactly one subscriber, and the broadcasts all returned.
        assert_eq!(stream.subscriber_count(), 1);
        let received = std::iter::from_fn(|| rx.try_recv().ok()).count();
        assert_eq!(
            received, QUEUE_DEPTH,
            "the queue holds its depth and the rest are dropped"
        );
        stream.unsubscribe(id);
        assert_eq!(stream.subscriber_count(), 0);
    }

    #[test]
    fn lane_count_is_clamped_at_both_ends() {
        // A configured zero must never reach the modulus.
        assert_eq!(lane_count(3, 0), 1, "0 lanes would divide by zero");
        assert_eq!(lane_count(0, 0), 1);
        // The default is a sequential walk.
        assert_eq!(lane_count(3, 1), 1);
        // Never more lanes than there are units to put in them.
        assert_eq!(lane_count(3, 8), 3);
        assert_eq!(lane_count(0, 8), 1, "no units still needs a valid modulus");
        // And an honest request under the unit count is granted as asked.
        assert_eq!(lane_count(10, 2), 2);
        assert_eq!(lane_count(10, 10), 10);
    }

    #[test]
    fn the_lanes_partition_the_units_exactly() {
        // The failure this guards against is silent: a lane assignment that
        // skips an index means one air conditioner simply stops appearing in
        // the stream, with nothing logged and every other unit still updating.
        for total in 0..13usize {
            for requested in 0..11usize {
                let lanes = lane_count(total, requested);
                let mut times_polled = vec![0u32; total];
                for lane in 0..lanes {
                    for (i, polls) in times_polled.iter_mut().enumerate() {
                        if i % lanes == lane {
                            *polls += 1;
                        }
                    }
                }
                assert!(
                    times_polled.iter().all(|&n| n == 1),
                    "total={total} requested={requested} lanes={lanes} gave {times_polled:?}"
                );
            }
        }
    }

    #[test]
    fn subscribing_is_refused_past_the_ceiling() {
        let stream = StateStream::new(5);
        let mut held = Vec::new();
        for _ in 0..MAX_STREAMS {
            held.push(stream.subscribe().expect("under the ceiling"));
        }
        assert_eq!(stream.subscriber_count(), MAX_STREAMS);
        assert!(
            stream.subscribe().is_none(),
            "one past the ceiling must be refused, not queued"
        );
        // Closing one frees a slot.
        let (id, _rx) = held.pop().unwrap();
        stream.unsubscribe(id);
        assert!(stream.subscribe().is_some());
    }

    #[test]
    fn every_subscriber_gets_every_event() {
        let stream = StateStream::new(5);
        let (_, a) = stream.subscribe().unwrap();
        let (_, b) = stream.subscribe().unwrap();
        stream.broadcast(("state", serde_json::json!({"id": "7"})));
        assert_eq!(a.try_recv().unwrap().1["id"], "7");
        assert_eq!(b.try_recv().unwrap().1["id"], "7");
    }

    #[test]
    fn the_tick_has_a_floor() {
        assert_eq!(StateStream::new(5).tick_seconds(), 5);
        assert_eq!(
            StateStream::new(0).tick_seconds(),
            1,
            "a zero tick would poll as fast as the LAN allows"
        );
    }

    #[test]
    fn an_unseen_unit_falls_back_to_a_complete_state() {
        // Clients render this straight into a card, so every field a live state
        // has must be present -- an error record would leave holes.
        let stream = StateStream::new(5);
        let _ = &stream;
        let live = serde_json::to_value(UnitState {
            id: "7".into(),
            name: "Kitchen".into(),
            ip: "192.0.2.1".into(),
            online: true,
            power_state: true,
            operational_mode: "COOL".into(),
            target_temperature: 24.0,
            indoor_temperature: Some(26.0),
            outdoor_temperature: Some(28.0),
            fan_speed: 102,
            swing_mode: "BOTH".into(),
            eco: false,
            turbo: false,
        })
        .unwrap();
        let placeholder = serde_json::json!({
            "id": "7", "name": "", "ip": "", "online": false,
            "power_state": false, "operational_mode": "AUTO",
            "target_temperature": 22.0, "indoor_temperature": null,
            "outdoor_temperature": null, "fan_speed": 102,
            "swing_mode": "OFF", "eco": false, "turbo": false,
        });
        let mut live_keys: Vec<&String> = live.as_object().unwrap().keys().collect();
        let mut placeholder_keys: Vec<&String> = placeholder.as_object().unwrap().keys().collect();
        live_keys.sort();
        placeholder_keys.sort();
        assert_eq!(
            live_keys, placeholder_keys,
            "the placeholder must have exactly the fields a live state has"
        );
    }
}
