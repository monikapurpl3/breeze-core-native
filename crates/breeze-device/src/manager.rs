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
//! the whole map instead would make every unit wait behind the slowest, which on
//! a protocol with ~0.7 s round-trips is the difference between a panel that
//! feels alive and one that does not.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use crate::device::{Device, UnitConfig};
use crate::error::DeviceError;

pub struct DeviceManager {
    units: RwLock<HashMap<u64, Arc<Mutex<Device>>>>,
    /// Configured order, so listings are stable rather than hash order.
    order: RwLock<Vec<u64>>,
}

impl DeviceManager {
    pub fn new(configs: impl IntoIterator<Item = UnitConfig>) -> Self {
        let mut units = HashMap::new();
        let mut order = Vec::new();
        for c in configs {
            order.push(c.id);
            units.insert(c.id, Arc::new(Mutex::new(Device::new(c))));
        }
        Self {
            units: RwLock::new(units),
            order: RwLock::new(order),
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

    /// Borrow a unit's handle. The map lock is released immediately; the caller
    /// then locks the unit itself for as long as it needs.
    pub fn get(&self, id: u64) -> Option<Arc<Mutex<Device>>> {
        self.units
            .read()
            .expect("units lock poisoned")
            .get(&id)
            .cloned()
    }

    /// Run something against one unit, holding only that unit's lock.
    pub fn with_unit<T>(
        &self,
        id: u64,
        f: impl FnOnce(&mut Device) -> Result<T, DeviceError>,
    ) -> Result<T, DeviceError> {
        let handle = self.get(id).ok_or(DeviceError::MissingCredentials)?;
        // A panic while holding a unit's lock should not take the whole server
        // down with it: the connection is rebuilt on the next call anyway.
        let mut device = match handle.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                let mut g = poisoned.into_inner();
                g.forget();
                g
            }
        };
        f(&mut device)
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
        units.insert(id, Arc::new(Mutex::new(Device::new(config))));
    }

    /// Remove a unit entirely.
    pub fn remove(&self, id: u64) -> bool {
        let mut units = self.units.write().expect("units lock poisoned");
        let mut order = self.order.write().expect("order lock poisoned");
        order.retain(|x| *x != id);
        units.remove(&id).is_some()
    }

    /// Whether a unit currently has a live session.
    ///
    /// For diagnostics only. Reading it must not open one: `/api/system` reports
    /// this for every unit at once, and connecting each would cost ~700ms apiece
    /// while somebody watches a spinner.
    pub fn is_connected(&self, id: u64) -> bool {
        match self.get(id) {
            Some(handle) => handle.lock().map(|d| d.is_connected()).unwrap_or(false),
            None => false,
        }
    }

    /// Whether the unit answered the last time anything asked.
    ///
    /// Not the same question as `is_connected`: a session can still be open to
    /// a unit that has stopped replying, which is precisely the state worth
    /// seeing on a diagnostics screen. Reads cached state only.
    pub fn is_online(&self, id: u64) -> bool {
        match self.get(id) {
            Some(handle) => handle.lock().map(|d| d.online()).unwrap_or(false),
            None => false,
        }
    }

    /// A unit's already-probed capabilities, or `None` if nothing has probed.
    ///
    /// Never triggers a probe, for the same reason `is_connected` never opens a
    /// session.
    pub fn cached_capabilities(
        &self,
        id: u64,
    ) -> Option<breeze_proto::ac::capabilities::Capabilities> {
        self.get(id)?
            .lock()
            .ok()
            .and_then(|d| d.cached_capabilities().cloned())
    }

    /// Rename a unit in place, keeping its connection.
    ///
    /// Returns whether the unit was there. Separate from `upsert` because that
    /// drops the session, which a label change has no reason to do.
    pub fn rename(&self, id: u64, name: &str) -> bool {
        match self.get(id) {
            Some(handle) => match handle.lock() {
                Ok(mut device) => {
                    device.set_name(name);
                    true
                }
                Err(_) => false,
            },
            None => false,
        }
    }

    /// Drop a unit's cached connection without forgetting the unit.
    pub fn forget(&self, id: u64) {
        if let Some(handle) = self.get(id) {
            if let Ok(mut d) = handle.lock() {
                d.forget();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};
    use std::thread;
    use std::time::{Duration, Instant};

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

    #[test]
    fn listing_follows_configuration_order_not_hash_order() {
        let m = DeviceManager::new([config(30, "c"), config(10, "a"), config(20, "b")]);
        assert_eq!(m.known_units(), vec![30, 10, 20]);
    }

    #[test]
    fn upsert_adds_once_and_then_replaces_in_place() {
        let m = DeviceManager::new([config(1, "one")]);
        m.upsert(config(2, "two"));
        assert_eq!(m.known_units(), vec![1, 2]);
        // Replacing must not append a duplicate to the order.
        m.upsert(config(2, "two renamed"));
        assert_eq!(m.known_units(), vec![1, 2]);
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
        assert_eq!(m.known_units(), vec![1], "must not have invented unit 99");
    }
}
