//! One air conditioner, and the connection to it.
//!
//! Connections are lazy and cached: the first request after a start pays connect
//! plus handshake plus the settle wait, and everything after that reuses the
//! session until it expires or the unit drops it. Every read and every write is
//! still a live round-trip — there is no push in this protocol — so per-call
//! latency of a few hundred milliseconds is inherent, not a bug to optimise away.

use breeze_proto::ac::capabilities::Capabilities;
use breeze_proto::ac::command::{self, Setpoint};
use breeze_proto::ac::response::State;
use breeze_proto::ac::types::TemperatureType;
use breeze_proto::{frame, packet};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

use crate::error::DeviceError;
use crate::session::Session;

/// Attempts per operation, matching msmart's retry count. A unit that has dropped
/// an idle connection needs exactly one reconnect, so three is generous.
const ATTEMPTS: u32 = 3;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const IO_TIMEOUT: Duration = Duration::from_secs(10);

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

pub struct Device {
    config: UnitConfig,
    session: Option<Session<TcpStream>>,
    /// Fetched on first request and kept: a unit's capabilities cannot change
    /// while it is powered.
    capabilities: Option<Capabilities>,
    /// Last successfully decoded state, kept so a caller can answer "what was it
    /// last doing?" without a round-trip and without inventing values.
    last_state: Option<State>,
    online: bool,
    /// Overrides the post-handshake wait. Tests only.
    settle: Option<Duration>,
}

impl Device {
    pub fn new(config: UnitConfig) -> Self {
        Self {
            config,
            session: None,
            capabilities: None,
            last_state: None,
            online: false,
            settle: None,
        }
    }

    #[cfg(test)]
    pub fn with_settle(mut self, settle: Duration) -> Self {
        self.settle = Some(settle);
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

    /// Whether the last exchange succeeded. This is a record of the past, not a
    /// probe: it says nothing about whether the unit would answer right now.
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

    pub fn online(&self) -> bool {
        self.online
    }

    pub fn last_state(&self) -> Option<&State> {
        self.last_state.as_ref()
    }

    /// Drop any cached connection. Used when configuration changes underneath us.
    pub fn forget(&mut self) {
        self.session = None;
        self.online = false;
    }

    /// Read current state from the unit.
    pub fn refresh(&mut self) -> Result<State, DeviceError> {
        let frame = command::get_state(self.next_message_id(), TemperatureType::Indoor);
        let state = self.exchange(&frame)?;
        Ok(state)
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
        let mut caps = Capabilities::parse(&self.exchange_raw(&frame)?);

        if caps.additional {
            let frame = command::get_more_capabilities(self.next_message_id());
            if let Ok(payload) = self.exchange_raw(&frame) {
                caps.merge(&Capabilities::parse(&payload));
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
        self.exchange(&frame)
    }

    /// Convenience for the common case: read, change one or more fields, write.
    pub fn modify(&mut self, change: impl FnOnce(&mut Setpoint)) -> Result<State, DeviceError> {
        let current = self.refresh()?;
        let mut setpoint = Setpoint::from_state(&current);
        change(&mut setpoint);
        self.apply(&setpoint)
    }

    /// Send a framed command and return its raw appliance payload.
    ///
    /// Separate from [`Device::exchange`] because a capability reply is not a
    /// state report: decoding it as one would fail, and retrying that failure
    /// three times would just take longer to be wrong.
    fn exchange_raw(&mut self, command_frame: &[u8]) -> Result<Vec<u8>, DeviceError> {
        let mut last: Option<DeviceError> = None;
        for attempt in 1..=ATTEMPTS {
            match self.try_exchange_raw(command_frame) {
                Ok(payload) => {
                    self.online = true;
                    return Ok(payload);
                }
                Err(e) if e.is_retryable() && attempt < ATTEMPTS => {
                    self.session = None;
                    last = Some(e);
                }
                Err(e) => {
                    self.online = false;
                    return Err(e);
                }
            }
        }
        self.online = false;
        Err(last.unwrap_or(DeviceError::Unreachable { attempts: ATTEMPTS }))
    }

    fn try_exchange_raw(&mut self, command_frame: &[u8]) -> Result<Vec<u8>, DeviceError> {
        self.ensure_session()?;
        let session = self.session.as_mut().expect("ensure_session succeeded");
        let reply = session.request(&packet::encode(self.config.id, command_frame))?;
        let inner = packet::decode(&reply)?;
        Ok(frame::parse(&inner, frame::DeviceType::AirConditioner)?.to_vec())
    }

    /// Send a framed command and decode the state report it produces, retrying
    /// on transient failures with a fresh connection.
    fn exchange(&mut self, command_frame: &[u8]) -> Result<State, DeviceError> {
        let mut last: Option<DeviceError> = None;
        for attempt in 1..=ATTEMPTS {
            match self.try_exchange(command_frame) {
                Ok(state) => {
                    self.online = true;
                    self.last_state = Some(state.clone());
                    return Ok(state);
                }
                Err(e) if e.is_retryable() && attempt < ATTEMPTS => {
                    // The cached session is the most likely culprit; a stale one
                    // produces exactly these errors.
                    self.session = None;
                    last = Some(e);
                }
                Err(e) => {
                    self.online = false;
                    return Err(e);
                }
            }
        }
        self.online = false;
        Err(last.unwrap_or(DeviceError::Unreachable { attempts: ATTEMPTS }))
    }

    fn try_exchange(&mut self, command_frame: &[u8]) -> Result<State, DeviceError> {
        self.ensure_session()?;
        let session = self.session.as_mut().expect("ensure_session succeeded");
        let reply = session.request(&packet::encode(self.config.id, command_frame))?;
        let inner = packet::decode(&reply)?;
        let payload = frame::parse(&inner, frame::DeviceType::AirConditioner)?;
        Ok(State::parse(payload)?)
    }

    /// Connect and authenticate if there is no usable session.
    fn ensure_session(&mut self) -> Result<(), DeviceError> {
        if self.session.as_ref().is_some_and(|s| s.is_authenticated()) {
            return Ok(());
        }
        let (token, key) = self.config.credentials()?;
        let addr = SocketAddr::new(self.config.ip, self.config.port);
        let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        // Nagle would batch our small packets against a device that answers one
        // request at a time; there is nothing to coalesce.
        stream.set_nodelay(true)?;

        let mut session = Session::new(stream);
        if let Some(settle) = self.settle {
            session = session.with_settle(settle);
        }
        session.authenticate(token, key)?;
        self.session = Some(session);
        Ok(())
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
    use std::net::Ipv4Addr;

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
        // Missing credentials can never succeed, so exchange must not try thrice.
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
}
