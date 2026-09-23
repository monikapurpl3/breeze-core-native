//! The set of configured units, and the locking that keeps them honest.
//!
//! Two levels of lock, and the split is the point:
//!
//! * a short-lived lock on the **map**, held only long enough to look a unit up;
//! * a lock **per unit**, held for the whole round-trip.
//!
//! So two requests to the same air conditioner serialise — they must, since a
//! unit answers one command at a time and a control command is a read-modify-write
//! — while requests to *different* units run concurrently. Holding one lock over
//! the whole map instead would make every unit wait behind the slowest.
//!
//! # Not paying for the same round-trip twice
//!
//! Serialising is necessary; queueing is not. 4.0.2 ran every request to a unit
//! one after another, and a unit answers in ~0.72 s whatever it is asked, so
//! tapping + ten times sent ten commands that finished 32 s after the tapping
//! stopped — each answering with a temperature the user had already tapped
//! past, and each holding a server thread while it waited. Three things stop
//! that here:
//!
//! * **Controls merge.** A control that arrives while another is in flight
//!   folds into the next command instead of queueing its own; when that one
//!   goes out it carries every change that arrived meanwhile, and all of them
//!   are answered with its result. Ten taps become two commands.
//! * **Reads share.** A read that waited behind another read — or behind a
//!   control — takes that answer instead of asking again. It arrived after the
//!   request did, so it is as current as anything a fresh round-trip would say.
//! * **Identity and status never wait on a unit.** The unit list and the
//!   diagnostics screen read copies kept beside the unit, not the unit itself,
//!   so nothing that only needs a name blocks behind a slow air conditioner.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard, RwLock, TryLockError};
use std::time::{Duration, Instant};

use breeze_proto::ac::capabilities::Capabilities;
use breeze_proto::ac::command::Setpoint;
use breeze_proto::ac::response::State;
use breeze_proto::ac::types::{FanSpeed, Mode, SwingMode};

use crate::device::{Device, DeviceStats, UnitConfig};
use crate::error::DeviceError;
use crate::session::Timing;

/// How recent the unit's last reported state must be to build a control on it
/// without reading the unit first.
///
/// A control restates the whole configuration, so it needs a base. Reading one
/// fresh costs a round-trip — half of every control, in 4.0.2 — while the state
/// the unit reported moments ago (from the panel's poll, or the previous
/// command's echo) is already here. The window bounds the one risk: a field
/// someone changed with the IR remote since that report would be put back. The
/// reference had no window at all; ten seconds keeps a burst of taps at one
/// round-trip each and still re-reads a unit nobody has looked at for a while.
const CONTROL_BASE_MAX_AGE: Duration = Duration::from_secs(10);

/// A unit's identity: what a listing needs, and nothing that requires asking it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitInfo {
    pub id: u64,
    pub name: String,
    pub ip: IpAddr,
}

/// What the server last learned about a unit, readable without waiting for it.
#[derive(Debug, Clone, Default)]
pub struct UnitStatus {
    /// Whether the last exchange worked.
    pub online: bool,
    /// Whether a session is open right now.
    pub connected: bool,
    pub capabilities: Option<Capabilities>,
    pub stats: DeviceStats,
}

/// The fields a control request sets. Absent means "leave it as it is".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Change {
    pub power_on: Option<bool>,
    pub mode: Option<Mode>,
    pub target_temperature: Option<f32>,
    pub fan_speed: Option<FanSpeed>,
    pub swing_mode: Option<SwingMode>,
    pub eco: Option<bool>,
    pub turbo: Option<bool>,
    /// Whether the unit chirps. Absent means silent: a schedule firing at
    /// 2 a.m. should not.
    pub beep: Option<bool>,
}

impl Change {
    /// Fold a later request into this one. Whatever the later one sets wins;
    /// whatever it leaves out keeps the earlier value.
    pub fn merge(&mut self, later: Change) {
        fn take<T>(slot: &mut Option<T>, later: Option<T>) {
            if later.is_some() {
                *slot = later;
            }
        }
        take(&mut self.power_on, later.power_on);
        take(&mut self.mode, later.mode);
        take(&mut self.target_temperature, later.target_temperature);
        take(&mut self.fan_speed, later.fan_speed);
        take(&mut self.swing_mode, later.swing_mode);
        take(&mut self.eco, later.eco);
        take(&mut self.turbo, later.turbo);
        take(&mut self.beep, later.beep);
    }

    /// Write the fields this change sets into a full setpoint.
    pub fn apply_to(&self, s: &mut Setpoint) {
        if let Some(v) = self.power_on {
            s.power_on = v;
        }
        if let Some(v) = self.mode {
            s.mode = v;
        }
        if let Some(v) = self.target_temperature {
            s.target_temperature = v;
        }
        if let Some(v) = self.fan_speed {
            s.fan_speed = v;
        }
        if let Some(v) = self.swing_mode {
            s.swing_mode = v;
        }
        if let Some(v) = self.eco {
            s.eco = v;
        }
        if let Some(v) = self.turbo {
            s.turbo = v;
        }
        s.beep = self.beep.unwrap_or(false);
    }
}

/// The result of one command, shared by every request merged into it.
#[derive(Debug, Clone)]
pub struct ControlOutcome {
    /// What the unit reported back. Every merged request answers with this.
    pub state: State,
    /// What the command was built on.
    pub base: State,
    /// Whether `base` was the unit's recent report rather than a fresh read.
    pub base_was_cached: bool,
    /// The setpoint actually sent.
    pub sent: Setpoint,
    /// How many requests this one command answered.
    pub merged: usize,
    /// Whether this request is the one that sent the command. The others were
    /// folded into it and got its result.
    pub sent_by_this_request: bool,
}

/// Controls waiting to go out on one unit.
#[derive(Default)]
struct ControlQueue {
    /// Everything merged since the last command left.
    pending: Change,
    /// How many requests are folded into `pending`.
    pending_count: usize,
    /// The number `pending` will be sent under.
    next_batch: u64,
    /// Outcomes some of whose requests have not collected them yet.
    done: HashMap<u64, Finished>,
}

struct Finished {
    outcome: Result<ControlOutcome, DeviceError>,
    uncollected: usize,
}

struct Slot {
    device: Mutex<Device>,
    info: RwLock<UnitInfo>,
    status: Mutex<UnitStatus>,
    queue: Mutex<ControlQueue>,
}

impl Slot {
    fn new(config: UnitConfig, timing: Timing) -> Self {
        let info = UnitInfo {
            id: config.id,
            name: config.name.clone(),
            ip: config.ip,
        };
        Self {
            device: Mutex::new(Device::new(config).with_timing(timing)),
            info: RwLock::new(info),
            status: Mutex::new(UnitStatus::default()),
            queue: Mutex::new(ControlQueue::default()),
        }
    }

    /// Lock the unit itself. A panic while holding it should not take the whole
    /// server down: the connection is simply rebuilt on the next call.
    fn lock_device(&self) -> MutexGuard<'_, Device> {
        match self.device.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                let mut g = poisoned.into_inner();
                g.forget();
                g
            }
        }
    }

    /// Copy what the device knows into the lock-free status. Called with the
    /// device lock held, after every operation.
    fn publish(&self, device: &Device) {
        let status = UnitStatus {
            online: device.online(),
            connected: device.is_connected(),
            capabilities: device.cached_capabilities().cloned(),
            stats: device.stats(),
        };
        *lock(&self.status) = status;
    }

    /// Take the outcome of batch `n` if it has already been sent.
    fn collect(&self, n: u64) -> Option<Result<ControlOutcome, DeviceError>> {
        let mut q = lock(&self.queue);
        let finished = q.done.get_mut(&n)?;
        let outcome = finished.outcome.clone().map(|mut o| {
            o.sent_by_this_request = false;
            o
        });
        finished.uncollected -= 1;
        if finished.uncollected == 0 {
            q.done.remove(&n);
        }
        Some(outcome)
    }
}

/// Lock something whose contents cannot be left inconsistent by a panic: every
/// value under these locks is replaced whole.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

pub struct DeviceManager {
    units: RwLock<HashMap<u64, Arc<Slot>>>,
    /// Configured order, so listings are stable rather than hash order.
    order: RwLock<Vec<u64>>,
    timing: Timing,
}

impl DeviceManager {
    pub fn new(configs: impl IntoIterator<Item = UnitConfig>) -> Self {
        Self::with_timing(configs, Timing::default())
    }

    /// As [`DeviceManager::new`], with sessions that wait differently. Tests
    /// only in practice.
    pub fn with_timing(configs: impl IntoIterator<Item = UnitConfig>, timing: Timing) -> Self {
        let mut units = HashMap::new();
        let mut order = Vec::new();
        for c in configs {
            order.push(c.id);
            units.insert(c.id, Arc::new(Slot::new(c, timing)));
        }
        Self {
            units: RwLock::new(units),
            order: RwLock::new(order),
            timing,
        }
    }

    /// Unit ids in configuration order.
    pub fn known_units(&self) -> Vec<u64> {
        self.order.read().expect("order lock poisoned").clone()
    }

    pub fn contains(&self, id: u64) -> bool {
        self.units
            .read()
            .expect("units lock poisoned")
            .contains_key(&id)
    }

    fn slot(&self, id: u64) -> Option<Arc<Slot>> {
        self.units
            .read()
            .expect("units lock poisoned")
            .get(&id)
            .cloned()
    }

    /// One unit's identity. Never waits on the unit.
    pub fn info(&self, id: u64) -> Option<UnitInfo> {
        let slot = self.slot(id)?;
        let info = slot.info.read().unwrap_or_else(|p| p.into_inner()).clone();
        Some(info)
    }

    /// Every unit's identity, in configuration order. Never waits on a unit:
    /// in 4.0.2 this took each unit's lock, and the app's first request on
    /// opening waited up to 11.7 s behind a control queue to learn three names.
    pub fn list(&self) -> Vec<UnitInfo> {
        self.known_units()
            .into_iter()
            .filter_map(|id| self.info(id))
            .collect()
    }

    /// What the server last learned about a unit. Never waits on the unit.
    pub fn status(&self, id: u64) -> Option<UnitStatus> {
        Some(lock(&self.slot(id)?.status).clone())
    }

    /// Run something against one unit, holding only that unit's lock.
    pub fn with_unit<T>(
        &self,
        id: u64,
        f: impl FnOnce(&mut Device) -> Result<T, DeviceError>,
    ) -> Result<T, DeviceError> {
        let slot = self.slot(id).ok_or(DeviceError::MissingCredentials)?;
        let mut device = slot.lock_device();
        let result = f(&mut device);
        slot.publish(&device);
        result
    }

    /// Read a unit's state, sharing a round-trip with any request that got
    /// there first.
    ///
    /// If, while this request waited for the unit, somebody else read it — or
    /// changed it, which also reports the state — that answer arrived after this
    /// request did, so it is as current as a fresh read and costs nothing. The
    /// same goes for a failure: a unit that has just failed to answer one
    /// request is not asked again by everyone queued behind it.
    pub fn read_state(&self, id: u64) -> Result<State, DeviceError> {
        let arrived = Instant::now();
        let slot = self.slot(id).ok_or(DeviceError::MissingCredentials)?;
        let mut device = slot.lock_device();
        if let Some(state) = device.state_since(arrived) {
            return Ok(state.clone());
        }
        if let Some(e) = device.failure_since(arrived) {
            return Err(e.clone());
        }
        let result = device.refresh();
        slot.publish(&device);
        result
    }

    /// What the unit says it can do. Probed once, then cached.
    pub fn capabilities(&self, id: u64) -> Result<Capabilities, DeviceError> {
        self.with_unit(id, |d| d.capabilities())
    }

    /// Change some of a unit's settings.
    ///
    /// Merged with any other change that arrives while the unit is busy: the
    /// next command carries all of them, and every request it covers gets its
    /// result. Order is kept -- a later request's value for a field wins.
    pub fn control(&self, id: u64, change: Change) -> Result<ControlOutcome, DeviceError> {
        let slot = self.slot(id).ok_or(DeviceError::MissingCredentials)?;

        // Join the batch that goes out next. Under the queue lock, so this and
        // the moment a batch is taken are strictly ordered: a request is either
        // in the batch being taken, or it is numbered for the one after.
        let batch = {
            let mut q = lock(&slot.queue);
            q.pending.merge(change);
            q.pending_count += 1;
            q.next_batch
        };

        let mut device = slot.lock_device();

        // Sent while we waited for the unit? Then the answer is already here.
        if let Some(outcome) = slot.collect(batch) {
            return outcome;
        }

        // Otherwise this request is the first of its batch to reach the unit,
        // and sends everything merged so far as one command.
        let (change, merged) = {
            let mut q = lock(&slot.queue);
            if q.next_batch != batch {
                // Taken by a request that panicked before it could record an
                // outcome. The change was lost; say so rather than guess.
                return Err(DeviceError::Io(std::io::Error::other(
                    "the command was lost before it was sent",
                )));
            }
            q.next_batch += 1;
            let count = std::mem::replace(&mut q.pending_count, 0);
            (std::mem::take(&mut q.pending), count)
        };

        let outcome = send_change(&mut device, &change, merged);
        slot.publish(&device);
        if merged > 1 {
            lock(&slot.queue).done.insert(
                batch,
                Finished {
                    outcome: outcome.clone(),
                    uncollected: merged - 1,
                },
            );
        }
        outcome
    }

    /// Read every unit that has been quiet for `max_idle` or longer, so its
    /// connection outlives the unit's idle timer. Returns how many were read.
    ///
    /// A unit closes a connection 30 s after its last request, and the next
    /// one then pays a reconnect: ~1 s of handshake and the settle the protocol
    /// demands. Skips any unit that is busy -- whoever is using it is keeping it
    /// warm -- and any that failed recently.
    pub fn keep_warm(&self, max_idle: Duration) -> usize {
        let mut read = 0;
        for id in self.known_units() {
            let Some(slot) = self.slot(id) else { continue };
            let mut device = match slot.device.try_lock() {
                Ok(g) => g,
                Err(TryLockError::WouldBlock) => continue,
                Err(TryLockError::Poisoned(_)) => continue,
            };
            if !device.needs_warming(max_idle) {
                continue;
            }
            let _ = device.refresh();
            slot.publish(&device);
            read += 1;
        }
        read
    }

    /// Add a unit, or replace one whose configuration changed.
    ///
    /// Replacing drops the cached connection, because the address or credentials
    /// may have moved and a live session for the old ones is worse than none.
    pub fn upsert(&self, config: UnitConfig) {
        let id = config.id;
        let mut units = self.units.write().expect("units lock poisoned");
        let mut order = self.order.write().expect("order lock poisoned");
        if !units.contains_key(&id) {
            order.push(id);
        }
        units.insert(id, Arc::new(Slot::new(config, self.timing)));
    }

    /// Remove a unit entirely.
    pub fn remove(&self, id: u64) -> bool {
        let mut units = self.units.write().expect("units lock poisoned");
        let mut order = self.order.write().expect("order lock poisoned");
        order.retain(|x| *x != id);
        units.remove(&id).is_some()
    }

    /// Whether a unit currently has a live session. Never waits on the unit.
    pub fn is_connected(&self, id: u64) -> bool {
        self.status(id).is_some_and(|s| s.connected)
    }

    /// Whether the unit answered the last time anything asked.
    ///
    /// Not the same question as `is_connected`: a session can still be open to
    /// a unit that has stopped replying, which is precisely the state worth
    /// seeing on a diagnostics screen. Never waits on the unit.
    pub fn is_online(&self, id: u64) -> bool {
        self.status(id).is_some_and(|s| s.online)
    }

    /// A unit's already-probed capabilities, or `None` if nothing has probed.
    /// Never triggers a probe, and never waits on the unit.
    pub fn cached_capabilities(&self, id: u64) -> Option<Capabilities> {
        self.status(id)?.capabilities
    }

    /// Rename a unit in place, keeping its connection.
    ///
    /// Returns whether the unit was there. Never waits on the unit: the name
    /// clients see lives beside it, so a label edit does not queue behind a
    /// slow air conditioner.
    pub fn rename(&self, id: u64, name: &str) -> bool {
        let Some(slot) = self.slot(id) else {
            return false;
        };
        slot.info.write().unwrap_or_else(|p| p.into_inner()).name = name.to_string();
        // The device's own copy is only for its configuration record; update it
        // when it is free rather than waiting for it.
        if let Ok(mut d) = slot.device.try_lock() {
            d.set_name(name);
        }
        true
    }

    /// Drop a unit's cached connection without forgetting the unit.
    pub fn forget(&self, id: u64) {
        if let Some(slot) = self.slot(id) {
            let mut d = slot.lock_device();
            d.forget();
            slot.publish(&d);
        }
    }
}

/// Build one command from `change` and send it.
fn send_change(
    device: &mut Device,
    change: &Change,
    merged: usize,
) -> Result<ControlOutcome, DeviceError> {
    let (base, base_was_cached) = match device.fresh_state(CONTROL_BASE_MAX_AGE) {
        Some(s) => (s.clone(), true),
        None => (device.refresh()?, false),
    };
    let mut sent = Setpoint::from_state(&base);
    change.apply_to(&mut sent);
    let state = device.apply(&sent)?;
    Ok(ControlOutcome {
        state,
        base,
        base_was_cached,
        sent,
        merged,
        sent_by_this_request: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeServer;
    use std::net::Ipv4Addr;
    use std::thread;

    const KEY: [u8; 32] = [0x42; 32];

    fn config(id: u64, name: &str) -> UnitConfig {
        UnitConfig {
            id,
            name: name.into(),
            // TEST-NET-1, so nothing here can accidentally reach a real device.
            ip: IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
            port: 6444,
            token: None,
            key: None,
        }
    }

    fn quick() -> Timing {
        Timing {
            settle: Duration::ZERO,
            reply_timeout: Duration::from_millis(150),
            sends: 3,
            linger: Duration::from_millis(100),
        }
    }

    /// A manager with one unit behind a fake server answering after `delay`.
    fn fake(delay: Duration) -> (FakeServer, Arc<DeviceManager>) {
        let server = FakeServer::start(KEY, 7);
        server.behave(|b| b.delay = delay);
        let unit = UnitConfig {
            id: 7,
            name: "Fake".into(),
            ip: server.addr.ip(),
            port: server.addr.port(),
            token: Some(vec![0xAA; 64]),
            key: Some(KEY),
        };
        (
            server,
            Arc::new(DeviceManager::with_timing([unit], quick())),
        )
    }

    fn target(t: f32) -> Change {
        Change {
            target_temperature: Some(t),
            ..Change::default()
        }
    }

    #[test]
    fn listing_follows_configuration_order_not_hash_order() {
        let m = DeviceManager::new([config(30, "c"), config(10, "a"), config(20, "b")]);
        assert_eq!(m.known_units(), vec![30, 10, 20]);
        let names: Vec<String> = m.list().into_iter().map(|u| u.name).collect();
        assert_eq!(names, ["c", "a", "b"]);
    }

    #[test]
    fn upsert_adds_once_and_then_replaces_in_place() {
        let m = DeviceManager::new([config(1, "one")]);
        m.upsert(config(2, "two"));
        assert_eq!(m.known_units(), vec![1, 2]);
        // Replacing must not append a duplicate to the order.
        m.upsert(config(2, "two renamed"));
        assert_eq!(m.known_units(), vec![1, 2]);
        assert_eq!(m.info(2).unwrap().name, "two renamed");
        m.with_unit(2, |d| {
            assert_eq!(d.config().name, "two renamed");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn remove_drops_it_from_both_the_map_and_the_order() {
        let m = DeviceManager::new([config(1, "a"), config(2, "b")]);
        assert!(m.remove(1));
        assert!(!m.remove(1), "removing twice must report nothing removed");
        assert_eq!(m.known_units(), vec![2]);
        assert!(!m.contains(1));
    }

    #[test]
    fn requests_to_one_unit_serialise() {
        let m = Arc::new(DeviceManager::new([config(1, "a")]));
        let hits = Arc::new(Mutex::new(Vec::new()));
        let mut handles = Vec::new();
        for i in 0..4 {
            let m = Arc::clone(&m);
            let hits = Arc::clone(&hits);
            handles.push(thread::spawn(move || {
                m.with_unit(1, |_| {
                    // Record entry and exit; overlapping windows would interleave.
                    hits.lock().unwrap().push((i, true));
                    thread::sleep(Duration::from_millis(20));
                    hits.lock().unwrap().push((i, false));
                    Ok(())
                })
                .unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let hits = hits.lock().unwrap();
        // Every enter must be followed by its own exit.
        for pair in hits.chunks(2) {
            assert_eq!(
                pair[0].0, pair[1].0,
                "interleaved access to one unit: {hits:?}"
            );
            assert!(pair[0].1 && !pair[1].1);
        }
    }

    #[test]
    fn different_units_do_not_block_each_other() {
        let m = Arc::new(DeviceManager::new([config(1, "a"), config(2, "b")]));
        let start = Instant::now();
        let handles: Vec<_> = [1u64, 2]
            .into_iter()
            .map(|id| {
                let m = Arc::clone(&m);
                thread::spawn(move || {
                    m.with_unit(id, |_| {
                        thread::sleep(Duration::from_millis(150));
                        Ok(())
                    })
                    .unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        // Serialised would be ~300ms. Allow plenty of slack for slow CI.
        assert!(
            start.elapsed() < Duration::from_millis(280),
            "units appear to share a lock: took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn a_panic_holding_a_unit_lock_does_not_poison_the_manager() {
        let m = Arc::new(DeviceManager::new([config(1, "a")]));
        let m2 = Arc::clone(&m);
        let _ = thread::spawn(move || {
            let _ = m2.with_unit(1, |_| -> Result<(), DeviceError> {
                panic!("something went wrong mid-request");
            });
        })
        .join();
        // The unit must still be usable afterwards.
        m.with_unit(1, |d| {
            assert!(!d.online(), "a poisoned unit should have been reset");
            Ok(())
        })
        .expect("manager must survive a panicking request");
    }

    #[test]
    fn an_unknown_unit_is_reported_rather_than_created() {
        let m = DeviceManager::new([config(1, "a")]);
        assert!(m.with_unit(99, |_| Ok(())).is_err());
        assert!(m.control(99, target(22.0)).is_err());
        assert!(m.read_state(99).is_err());
        assert_eq!(m.known_units(), vec![1], "must not have invented unit 99");
    }

    #[test]
    fn a_later_change_wins_field_by_field() {
        let mut c = Change {
            target_temperature: Some(22.0),
            mode: Some(Mode::Heat),
            ..Change::default()
        };
        c.merge(Change {
            target_temperature: Some(23.5),
            eco: Some(true),
            ..Change::default()
        });
        assert_eq!(c.target_temperature, Some(23.5), "the later value wins");
        assert_eq!(
            c.mode,
            Some(Mode::Heat),
            "an absent field keeps the earlier one"
        );
        assert_eq!(c.eco, Some(true));
    }

    #[test]
    fn a_burst_of_controls_is_merged_into_a_few_commands() {
        // The failure from the log: someone taps + ten times in three seconds.
        // 4.0.2 sent ten commands one after another; the last finished half a
        // minute later. Here the ones that arrive while the unit is busy fold
        // into the next command.
        let (server, m) = fake(Duration::from_millis(80));
        m.read_state(7).unwrap();
        let queries_before = server.queries();

        let handles: Vec<_> = (1..=10)
            .map(|i| {
                let m = Arc::clone(&m);
                thread::sleep(Duration::from_millis(10));
                thread::spawn(move || m.control(7, target(24.0 + i as f32 * 0.5)).unwrap())
            })
            .collect();
        let outcomes: Vec<ControlOutcome> =
            handles.into_iter().map(|h| h.join().unwrap()).collect();

        assert!(
            server.controls() <= 3,
            "ten taps should need at most three commands, sent {}",
            server.controls()
        );
        assert_eq!(
            server.target(),
            29.0,
            "the last tap is what the unit ends up at"
        );
        assert_eq!(
            server.queries(),
            queries_before,
            "a fresh report was at hand: no control should have read the unit first"
        );
        let last = outcomes
            .iter()
            .map(|o| o.state.target_temperature)
            .fold(0.0, f32::max);
        assert_eq!(last, 29.0);
        assert!(
            outcomes.iter().any(|o| o.merged > 1),
            "some command must have carried more than one tap"
        );
    }

    #[test]
    fn every_request_in_a_merged_command_gets_its_result() {
        let (server, m) = fake(Duration::from_millis(100));
        m.read_state(7).unwrap();
        // Hold the unit busy with one command, and queue three behind it.
        let first = {
            let m = Arc::clone(&m);
            thread::spawn(move || m.control(7, target(20.0)).unwrap())
        };
        thread::sleep(Duration::from_millis(30));
        // Spaced out, so they reach the queue in this order: the last one to
        // arrive wins, and threads started back to back arrive in any order.
        let rest: Vec<_> = [21.0, 22.0, 23.0]
            .into_iter()
            .map(|t| {
                let m = Arc::clone(&m);
                let h = thread::spawn(move || m.control(7, target(t)).unwrap());
                thread::sleep(Duration::from_millis(10));
                h
            })
            .collect();
        assert_eq!(first.join().unwrap().state.target_temperature, 20.0);
        let mut senders = 0;
        for h in rest {
            let o = h.join().unwrap();
            assert_eq!(
                o.state.target_temperature, 23.0,
                "all three share the merged result"
            );
            assert_eq!(o.merged, 3);
            senders += usize::from(o.sent_by_this_request);
        }
        assert_eq!(senders, 1, "exactly one of the three sent the command");
        assert_eq!(server.controls(), 2);
    }

    #[test]
    fn a_failed_command_fails_every_request_it_carried() {
        // Merged requests share an outcome, so a failure must reach all of
        // them -- none may report success for a change the unit never took.
        let (server, m) = fake(Duration::from_millis(60));
        m.read_state(7).unwrap();
        let first = {
            let m = Arc::clone(&m);
            thread::spawn(move || m.control(7, target(20.0)))
        };
        thread::sleep(Duration::from_millis(20));
        // Everything from here on goes unanswered.
        server.behave(|b| b.ignore = 1000);
        let rest: Vec<_> = [21.0, 22.0]
            .into_iter()
            .map(|t| {
                let m = Arc::clone(&m);
                thread::spawn(move || m.control(7, target(t)))
            })
            .collect();
        assert!(first.join().unwrap().is_ok());
        for h in rest {
            assert!(
                h.join().unwrap().is_err(),
                "a lost command must not report success"
            );
        }
    }

    #[test]
    fn a_stale_report_is_re_read_before_building_on_it() {
        let (server, m) = fake(Duration::ZERO);
        m.read_state(7).unwrap();
        // Age the report past the window: someone may have used the remote.
        m.with_unit(7, |d| {
            d.age_last_state();
            Ok(())
        })
        .unwrap();
        let before = server.queries();
        let o = m.control(7, target(22.0)).unwrap();
        assert!(
            !o.base_was_cached,
            "no report on hand: the unit must be read first"
        );
        assert_eq!(server.queries(), before + 1);
    }

    #[test]
    fn reads_queued_behind_one_share_its_answer() {
        // Opening the app starts a batch read and the live stream at once, and
        // in 4.0.2 that read every unit twice.
        let (server, m) = fake(Duration::from_millis(100));
        m.read_state(7).unwrap();
        let before = server.queries();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let m = Arc::clone(&m);
                thread::spawn(move || m.read_state(7).unwrap())
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert!(
            server.queries() - before <= 2,
            "four concurrent reads should cost at most two round-trips, cost {}",
            server.queries() - before
        );
    }

    #[test]
    fn a_read_behind_a_control_takes_the_control_s_answer() {
        let (server, m) = fake(Duration::from_millis(100));
        m.read_state(7).unwrap();
        let before = server.queries();
        let c = {
            let m = Arc::clone(&m);
            thread::spawn(move || m.control(7, target(26.0)).unwrap())
        };
        thread::sleep(Duration::from_millis(20));
        let s = m.read_state(7).unwrap();
        c.join().unwrap();
        assert_eq!(
            s.target_temperature, 26.0,
            "the read must see the change it waited behind"
        );
        assert_eq!(
            server.queries(),
            before,
            "and cost no round-trip of its own"
        );
    }

    #[test]
    fn listing_and_status_never_wait_for_a_busy_unit() {
        // In 4.0.2 the unit list took each unit's lock, so the app's first
        // request on opening waited behind whatever that unit was doing.
        let (_server, m) = fake(Duration::from_millis(400));
        let busy = {
            let m = Arc::clone(&m);
            thread::spawn(move || m.read_state(7))
        };
        thread::sleep(Duration::from_millis(50));
        let started = Instant::now();
        assert_eq!(m.list().len(), 1);
        let _ = m.status(7);
        let _ = m.is_online(7);
        assert!(m.rename(7, "Renamed"));
        assert_eq!(m.info(7).unwrap().name, "Renamed");
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "waited {:?} on a busy unit",
            started.elapsed()
        );
        busy.join().unwrap().unwrap();
    }

    #[test]
    fn keep_warm_reads_only_units_that_have_gone_quiet() {
        let (server, m) = fake(Duration::ZERO);
        m.read_state(7).unwrap();
        let before = server.queries();
        assert_eq!(
            m.keep_warm(Duration::from_secs(20)),
            0,
            "just used: leave it"
        );
        thread::sleep(Duration::from_millis(60));
        assert_eq!(m.keep_warm(Duration::from_millis(50)), 1);
        assert_eq!(server.queries(), before + 1);
        assert_eq!(server.connections(), 1, "kept on the same connection");
    }

    #[test]
    fn keep_warm_never_queues_behind_real_work() {
        let (_server, m) = fake(Duration::from_millis(300));
        let busy = {
            let m = Arc::clone(&m);
            thread::spawn(move || m.read_state(7))
        };
        thread::sleep(Duration::from_millis(50));
        let started = Instant::now();
        assert_eq!(m.keep_warm(Duration::ZERO), 0);
        assert!(started.elapsed() < Duration::from_millis(100));
        busy.join().unwrap().unwrap();
    }
}
