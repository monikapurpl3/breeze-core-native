//! One air conditioner, and the connection to it.
//!
//! Connections are lazy and cached: the first request after a start pays connect
//! plus handshake plus the settle wait, and everything after that reuses the
//! session until it expires or the unit drops it. Every read and every write is
//! still a live round-trip — there is no push in this protocol — so per-call
//! latency of a few hundred milliseconds is inherent, not a bug to optimise away.
//!
//! What *is* worth avoiding is paying for more round-trips than a request needs,
//! and waiting longer than necessary when a unit ignores one. The first is the
//! manager's job (it merges controls and shares reads); the second is the
//! session's (it resends on the same connection). This type keeps the facts
//! both of those decisions need: when the unit last answered, what it said, and
//! when it last failed.

use breeze_proto::ac::capabilities::Capabilities;
use breeze_proto::ac::command::{self, Setpoint};
use breeze_proto::ac::response::State;
use breeze_proto::ac::types::TemperatureType;
use breeze_proto::{frame, packet};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use crate::error::DeviceError;
use crate::session::{LinkStats, Session, Timing};

/// Connections per operation. The session already resends on the connection it
/// has, so the only thing a further attempt adds is a *fresh* connection: one is
/// enough, and a unit that ignores three sends and a reconnect is not there.
const ATTEMPTS: u32 = 2;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Writes are a couple of hundred bytes into an idle socket; this only bounds
/// a unit that has stopped reading entirely.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long keep-warm leaves a unit alone after it failed. A unit that is off
/// the network costs a connect timeout per attempt, and nobody is waiting.
const WARM_BACKOFF: Duration = Duration::from_secs(300);

/// A unit as configured: identity and credentials, no connection.
#[derive(Debug, Clone)]
pub struct UnitConfig {
    pub id: u64,
    pub name: String,
    pub ip: IpAddr,
    pub port: u16,
    /// V3 handshake token. Absent means this unit cannot be reached.
    pub token: Option<Vec<u8>>,
    /// V3 cloud key, 32 bytes.
    pub key: Option<[u8; 32]>,
}

impl UnitConfig {
    fn credentials(&self) -> Result<(&[u8], &[u8; 32]), DeviceError> {
        match (self.token.as_deref(), self.key.as_ref()) {
            (Some(t), Some(k)) if !t.is_empty() => Ok((t, k)),
            _ => Err(DeviceError::MissingCredentials),
        }
    }
}

/// Counts for a diagnostics screen, cumulative since start.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeviceStats {
    /// Connections opened, the first one included.
    pub connects: u64,
    /// Requests the unit ignored and was asked again.
    pub resends: u64,
    /// Replies thrown away because no request was waiting for them.
    pub discarded: u64,
}

pub struct Device {
    config: UnitConfig,
    session: Option<Session<TcpStream>>,
    /// Fetched on first request and kept: a unit's capabilities cannot change
    /// while it is powered.
    capabilities: Option<Capabilities>,
    /// Last successfully decoded state, kept so a caller can answer "what was it
    /// last doing?" without a round-trip and without inventing values.
    last_state: Option<State>,
    /// When `last_state` arrived. What lets a request that waited behind
    /// another one reuse its answer instead of asking again.
    last_state_at: Option<Instant>,
    /// When the unit last answered anything. The unit's own idle timer counts
    /// from the last request it received, which is within a round-trip of this.
    last_exchange_at: Option<Instant>,
    /// The most recent failure, so requests queued behind it can share it
    /// instead of each spending another full timeout on a unit that is gone.
    last_failure: Option<(Instant, DeviceError)>,
    online: bool,
    timing: Timing,
    stats: DeviceStats,
}

impl Device {
    pub fn new(config: UnitConfig) -> Self {
        Self {
            config,
            session: None,
            capabilities: None,
            last_state: None,
            last_state_at: None,
            last_exchange_at: None,
            last_failure: None,
            online: false,
            timing: Timing::default(),
            stats: DeviceStats::default(),
        }
    }

    /// Override how sessions wait on the unit. Tests only in practice: real
    /// units want the protocol's own timings.
    pub fn with_timing(mut self, timing: Timing) -> Self {
        self.timing = timing;
        self
    }

    pub fn config(&self) -> &UnitConfig {
        &self.config
    }

    /// Change the display name, keeping the live session.
    ///
    /// A rename touches nothing the connection depends on, so replacing the
    /// whole device for it would cost a needless reconnect -- about 1.8s on the
    /// next request, for an edit to a label.
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.config.name = name.into();
    }

    /// Whether a session is currently open.
    ///
    /// Distinct from [`Device::online`], which records whether the last exchange
    /// worked: a unit can be `online` from a minute ago with no session now.
    pub fn is_connected(&self) -> bool {
        self.session.is_some()
    }

    /// The capabilities already cached, if any -- never a fetch.
    ///
    /// Distinct from [`Device::capabilities`], which probes the unit when the
    /// cache is cold. A diagnostics screen reports every unit at once and must
    /// not turn that into one LAN round-trip per unit.
    pub fn cached_capabilities(&self) -> Option<&Capabilities> {
        self.capabilities.as_ref()
    }

    /// Whether the last exchange succeeded. This is a record of the past, not a
    /// probe: it says nothing about whether the unit would answer right now.
    pub fn online(&self) -> bool {
        self.online
    }

    pub fn last_state(&self) -> Option<&State> {
        self.last_state.as_ref()
    }

    /// The last state, if it arrived at or after `since`.
    pub fn state_since(&self, since: Instant) -> Option<&State> {
        match self.last_state_at {
            Some(at) if at >= since => self.last_state.as_ref(),
            _ => None,
        }
    }

    /// The last state, if it is no older than `max_age`.
    pub fn fresh_state(&self, max_age: Duration) -> Option<&State> {
        match self.last_state_at {
            Some(at) if at.elapsed() <= max_age => self.last_state.as_ref(),
            _ => None,
        }
    }

    /// The last failure, if it happened at or after `since` and nothing has
    /// succeeded since.
    pub fn failure_since(&self, since: Instant) -> Option<&DeviceError> {
        match &self.last_failure {
            Some((at, e)) if *at >= since => Some(e),
            _ => None,
        }
    }

    /// How long since the unit last answered anything, or `None` if it never has.
    pub fn idle_for(&self) -> Option<Duration> {
        self.last_exchange_at.map(|at| at.elapsed())
    }

    /// Whether keep-warm should read this unit now: it has been quiet for at
    /// least `max_idle` (or never spoken), and it has not failed recently.
    pub fn needs_warming(&self, max_idle: Duration) -> bool {
        if let Some((at, _)) = &self.last_failure {
            if at.elapsed() < WARM_BACKOFF {
                return false;
            }
        }
        if self.config.credentials().is_err() {
            return false;
        }
        self.idle_for().is_none_or(|idle| idle >= max_idle)
    }

    pub fn stats(&self) -> DeviceStats {
        let live = self
            .session
            .as_ref()
            .map(Session::stats)
            .unwrap_or_default();
        DeviceStats {
            connects: self.stats.connects,
            resends: self.stats.resends + live.resends,
            discarded: self.stats.discarded + live.discarded,
        }
    }

    /// Drop any cached connection. Used when configuration changes underneath us.
    pub fn forget(&mut self) {
        self.drop_session();
        self.online = false;
    }

    /// Read current state from the unit.
    pub fn refresh(&mut self) -> Result<State, DeviceError> {
        let frame = command::get_state(self.next_message_id(), TemperatureType::Indoor);
        self.exchange(&frame, |payload| Ok(State::parse(payload)?))
            .inspect(|state| self.remember(state))
    }

    /// What the unit says it can do, fetched once and cached.
    ///
    /// Cached because it cannot change while the unit is powered: asking again
    /// would be two more round-trips for an answer that is already known, and
    /// `/api/system` reports this for every unit at once.
    ///
    /// Two queries, because the protocol splits the list: a unit that sets the
    /// "more to come" flag answers a second query with the rest. Only the first
    /// failing is an error -- a unit that answers the first and not the second
    /// has told us most of what it can do, and half a capability list beats none.
    pub fn capabilities(&mut self) -> Result<Capabilities, DeviceError> {
        if let Some(cached) = &self.capabilities {
            return Ok(cached.clone());
        }
        let frame = command::get_capabilities(self.next_message_id());
        let mut caps = self.exchange(&frame, |payload| Ok(Capabilities::parse(payload)))?;

        if caps.additional {
            let frame = command::get_more_capabilities(self.next_message_id());
            if let Ok(more) = self.exchange(&frame, |payload| Ok(Capabilities::parse(payload))) {
                caps.merge(&more);
            }
        }
        self.capabilities = Some(caps.clone());
        Ok(caps)
    }

    /// Apply a complete setpoint and return the state the unit reports back.
    ///
    /// The unit echoes its resulting state, which is what makes an optimistic UI
    /// honest: the caller reconciles against what actually happened rather than
    /// what it asked for.
    pub fn apply(&mut self, setpoint: &Setpoint) -> Result<State, DeviceError> {
        let frame = command::set_state(self.next_message_id(), setpoint);
        self.exchange(&frame, |payload| Ok(State::parse(payload)?))
            .inspect(|state| self.remember(state))
    }

    /// Convenience for the common case: read, change one or more fields, write.
    pub fn modify(&mut self, change: impl FnOnce(&mut Setpoint)) -> Result<State, DeviceError> {
        let current = self.refresh()?;
        let mut setpoint = Setpoint::from_state(&current);
        change(&mut setpoint);
        self.apply(&setpoint)
    }

    /// Make the last report look old, as if nobody had asked for a while.
    #[cfg(test)]
    pub(crate) fn age_last_state(&mut self) {
        self.last_state_at = self
            .last_state_at
            .map(|_| Instant::now() - Duration::from_secs(3600));
    }

    fn remember(&mut self, state: &State) {
        self.last_state = Some(state.clone());
        self.last_state_at = Some(Instant::now());
    }

    /// Send a framed command and decode its reply with `decode`, reconnecting
    /// once if the connection turns out to be dead.
    ///
    /// `decode` is separate from the transport because a capability reply is
    /// not a state report: reading one as the other would fail, and retrying
    /// that failure would only take longer to be wrong.
    fn exchange<T>(
        &mut self,
        command_frame: &[u8],
        decode: impl Fn(&[u8]) -> Result<T, DeviceError>,
    ) -> Result<T, DeviceError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let result = self
                .try_exchange(command_frame)
                .and_then(|payload| decode(&payload));
            match result {
                Ok(value) => {
                    self.online = true;
                    self.last_exchange_at = Some(Instant::now());
                    self.last_failure = None;
                    return Ok(value);
                }
                Err(e) => {
                    // Any failure retires the connection, not only the ones
                    // worth retrying: a session that returned something it
                    // could not parse may be mid-packet, and reusing it would
                    // make every later request fail the same way.
                    self.drop_session();
                    if e.is_retryable() && attempt < ATTEMPTS {
                        continue;
                    }
                    self.online = false;
                    self.last_failure = Some((Instant::now(), e.clone()));
                    return Err(e);
                }
            }
        }
    }

    fn try_exchange(&mut self, command_frame: &[u8]) -> Result<Vec<u8>, DeviceError> {
        self.ensure_session()?;
        let session = self.session.as_mut().expect("ensure_session succeeded");
        let reply = session.request(&packet::encode(self.config.id, command_frame))?;
        let inner = packet::decode(&reply)?;
        Ok(frame::parse(&inner, frame::DeviceType::AirConditioner)?.to_vec())
    }

    /// Connect and authenticate if there is no usable session.
    fn ensure_session(&mut self) -> Result<(), DeviceError> {
        if self.session.as_ref().is_some_and(|s| s.is_authenticated()) {
            return Ok(());
        }
        let (token, key) = self.config.credentials()?;
        let addr = SocketAddr::new(self.config.ip, self.config.port);
        let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
        // Nagle would batch our small packets against a device that answers one
        // request at a time; there is nothing to coalesce.
        stream.set_nodelay(true)?;
        self.stats.connects += 1;

        let mut session = Session::new(stream).with_timing(self.timing);
        session.authenticate(token, key)?;
        self.session = Some(session);
        Ok(())
    }

    /// Close the connection, keeping its counts.
    fn drop_session(&mut self) {
        if let Some(s) = self.session.take() {
            let live: LinkStats = s.stats();
            self.stats.resends += live.resends;
            self.stats.discarded += live.discarded;
        }
    }

    /// Message ids only need to vary; nothing correlates replies by them.
    fn next_message_id(&self) -> u8 {
        use std::sync::atomic::{AtomicU8, Ordering};
        static NEXT: AtomicU8 = AtomicU8::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeServer;
    use std::net::Ipv4Addr;

    const KEY: [u8; 32] = [0x42; 32];

    fn config(token: Option<Vec<u8>>, key: Option<[u8; 32]>) -> UnitConfig {
        UnitConfig {
            id: 1,
            name: "Test".into(),
            // TEST-NET-1: guaranteed not to be a real host.
            ip: IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
            port: 6444,
            token,
            key,
        }
    }

    /// Fast timings, so an ignored request costs milliseconds.
    pub(crate) fn quick() -> Timing {
        Timing {
            settle: Duration::ZERO,
            reply_timeout: Duration::from_millis(150),
            sends: 3,
            linger: Duration::from_millis(250),
        }
    }

    fn device_for(server: &FakeServer) -> Device {
        Device::new(UnitConfig {
            id: 7,
            name: "Fake".into(),
            ip: server.addr.ip(),
            port: server.addr.port(),
            token: Some(vec![0xAA; 64]),
            key: Some(KEY),
        })
        .with_timing(quick())
    }

    #[test]
    fn a_unit_without_credentials_fails_before_touching_the_network() {
        let mut d = Device::new(config(None, None));
        let err = d.refresh().unwrap_err();
        assert!(
            matches!(err, DeviceError::MissingCredentials),
            "got {err:?}"
        );
        assert!(!d.online());
    }

    #[test]
    fn an_empty_token_counts_as_missing() {
        // An empty string in config.json is a half-finished pairing, not a
        // credential; treating it as one produces a confusing network error.
        let mut d = Device::new(config(Some(vec![]), Some([0; 32])));
        assert!(matches!(
            d.refresh().unwrap_err(),
            DeviceError::MissingCredentials
        ));
    }

    #[test]
    fn a_fatal_error_does_not_burn_retries() {
        // Missing credentials can never succeed, so exchange must not try again.
        let mut d = Device::new(config(None, None));
        let before = std::time::Instant::now();
        let _ = d.refresh();
        assert!(
            before.elapsed() < Duration::from_millis(500),
            "a fatal error should fail immediately, not retry"
        );
    }

    #[test]
    fn a_fresh_device_reports_neither_online_nor_state() {
        let d = Device::new(config(Some(vec![1; 64]), Some([2; 32])));
        assert!(!d.online());
        assert!(d.last_state().is_none(), "must not invent a state");
    }

    #[test]
    fn forget_clears_the_cached_connection() {
        let mut d = Device::new(config(Some(vec![1; 64]), Some([2; 32])));
        d.online = true;
        d.forget();
        assert!(!d.online());
        assert!(d.session.is_none());
    }

    #[test]
    fn message_ids_vary_between_commands() {
        let d = Device::new(config(Some(vec![1; 64]), Some([2; 32])));
        let a = d.next_message_id();
        let b = d.next_message_id();
        assert_ne!(a, b);
    }

    #[test]
    fn an_ignored_request_is_resent_on_the_same_connection() {
        // What a real unit does now and then. 4.0.2 answered it with a 10 s
        // timeout and a reconnect; the connection was fine all along.
        let server = FakeServer::start(KEY, 7);
        let mut d = device_for(&server);
        d.refresh().unwrap();
        server.behave(|b| b.ignore = 1);

        let started = Instant::now();
        d.refresh().expect("the resend should be answered");
        assert_eq!(server.connections(), 1, "must not have reconnected");
        assert_eq!(d.stats().resends, 1);
        // One reply timeout, not the old ten seconds.
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_late_reply_is_not_mistaken_for_the_next_one() {
        // The hazard resending creates: the first send is answered after all,
        // so two replies are on their way. The second must not become the echo
        // of the next command -- that is precisely a slider springing back.
        let server = FakeServer::start(KEY, 7);
        let mut d = device_for(&server);
        let before = d.refresh().unwrap();
        assert_eq!(before.target_temperature, 24.0);

        // Answer the next request after the client has already resent it.
        server.behave(|b| b.delay_next = Some(Duration::from_millis(200)));
        d.refresh().unwrap();

        let mut sp = Setpoint::from_state(&before);
        sp.target_temperature = 27.5;
        let echoed = d.apply(&sp).unwrap();
        assert_eq!(
            echoed.target_temperature, 27.5,
            "the echo must be the reply to this command, not a leftover read"
        );
        assert!(
            d.stats().discarded >= 1,
            "the duplicate should have been discarded"
        );
    }

    #[test]
    fn a_connection_the_unit_closed_is_replaced_before_it_is_used() {
        // A real unit closes a connection exactly 30 s after the last request.
        let server = FakeServer::start(KEY, 7);
        server.behave(|b| b.idle_close = Some(Duration::from_millis(100)));
        let mut d = device_for(&server);
        d.refresh().unwrap();
        std::thread::sleep(Duration::from_millis(250));

        let started = Instant::now();
        d.refresh().expect("a closed connection should be reopened");
        assert_eq!(server.connections(), 2);
        // Noticed from the FIN, not by waiting for a reply that cannot come.
        assert!(
            started.elapsed() < quick().reply_timeout,
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn state_bookkeeping_tracks_what_arrived_and_when() {
        let server = FakeServer::start(KEY, 7);
        let mut d = device_for(&server);
        let t0 = Instant::now();
        assert!(d.state_since(t0).is_none());
        d.refresh().unwrap();
        assert!(d.state_since(t0).is_some());
        assert!(d.fresh_state(Duration::from_secs(5)).is_some());
        assert!(d.idle_for().unwrap() < Duration::from_secs(1));
        assert!(!d.needs_warming(Duration::from_secs(20)));
        assert!(d.needs_warming(Duration::ZERO));
    }

    #[test]
    fn a_unit_that_just_failed_is_left_alone_by_keep_warm() {
        // Off the network, every attempt costs a connect timeout, and nobody is
        // waiting on the answer. Back off rather than hammer it.
        let server = FakeServer::start(KEY, 7);
        let mut d = device_for(&server);
        server.behave(|b| b.ignore = 100);
        assert!(d.refresh().is_err());
        assert!(!d.needs_warming(Duration::ZERO));
        assert!(d
            .failure_since(Instant::now() - Duration::from_secs(5))
            .is_some());
    }
}
