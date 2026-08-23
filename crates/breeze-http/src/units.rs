//! The unit state wire shape, and the routes that produce it.
//!
//! This JSON is the contract three clients already speak, so the field names,
//! their order and their *types* are fixed. The one that bites: `id` is a
//! **string**, because Breeze Core serialises `str(unit.id)` — the ids are
//! 48-bit and clients use them in URLs. Emitting a number here would be valid
//! JSON and would break every client.

use breeze_device::{Device, DeviceManager};
use breeze_proto::ac::response::State;
use serde::Serialize;

/// One unit's state, exactly as `devices/schemas.py:serialize` produces it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UnitState {
    /// String, not a number. See the module docs.
    pub id: String,
    pub name: String,
    pub ip: String,
    pub online: bool,
    pub power_state: bool,
    /// Enum *name*, never its number: `"COOL"`, not `2`.
    pub operational_mode: String,
    pub target_temperature: f64,
    pub indoor_temperature: Option<f64>,
    pub outdoor_temperature: Option<f64>,
    pub fan_speed: u8,
    pub swing_mode: String,
    pub eco: bool,
    pub turbo: bool,
}

/// What a unit looks like before we have spoken to it — or when we cannot.
///
/// Breeze Core reports `online: false` with the last known values rather than
/// failing the whole batch, so one unreachable unit does not blank a panel.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UnitOffline {
    pub id: String,
    pub name: String,
    pub ip: String,
    pub online: bool,
    pub error: String,
}

/// Just identity, for `GET /api/units`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UnitSummary {
    pub id: String,
    pub name: String,
    pub ip: String,
}

/// Unknown enum values are surfaced as `UNKNOWN`, never guessed.
///
/// A unit reporting a mode no firmware documents would otherwise be displayed as
/// whichever variant happened to be first — confidently wrong is worse than
/// visibly unknown.
fn mode_name(state: &State) -> String {
    state
        .mode
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "UNKNOWN".to_string())
}

fn swing_name(state: &State) -> String {
    state
        .swing_mode
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "UNKNOWN".to_string())
}

impl UnitState {
    pub fn from_device(device: &Device, state: &State) -> Self {
        let config = device.config();
        Self {
            id: config.id.to_string(),
            name: config.name.clone(),
            ip: config.ip.to_string(),
            online: true,
            power_state: state.power_on,
            operational_mode: mode_name(state),
            target_temperature: state.target_temperature as f64,
            indoor_temperature: state.indoor_temperature.map(|t| t as f64),
            outdoor_temperature: state.outdoor_temperature.map(|t| t as f64),
            fan_speed: state.fan_speed.0,
            swing_mode: swing_name(state),
            eco: state.eco,
            turbo: state.turbo,
        }
    }
}

/// `GET /api/units` — identity only, no round-trips.
pub fn list_units(manager: &DeviceManager) -> Vec<UnitSummary> {
    manager
        .known_units()
        .into_iter()
        .filter_map(|id| {
            manager
                .with_unit(id, |d| {
                    let c = d.config();
                    Ok(UnitSummary {
                        id: c.id.to_string(),
                        name: c.name.clone(),
                        ip: c.ip.to_string(),
                    })
                })
                .ok()
        })
        .collect()
}

/// One unit's live state, or a structured offline record.
///
/// Never an error: a unit that will not answer is a fact about the unit, not a
/// failure of the request. Breeze Core made the same choice in 2.4.0 so a batch
/// read does not 503 because one air conditioner is unplugged.
pub fn unit_state(manager: &DeviceManager, id: u64) -> Result<serde_json::Value, String> {
    // One lock acquisition for one logical read. Taking it three times -- once
    // for identity, once to refresh, once to serialise -- would let the unit's
    // state change underneath us between them, and would triple the contention
    // on a lock already held for a ~1.8s round-trip.
    manager
        .with_unit(id, |device| {
            let config = device.config().clone();
            let value = match device.refresh() {
                Ok(state) => serde_json::to_value(UnitState::from_device(device, &state)),
                Err(e) => serde_json::to_value(UnitOffline {
                    id: config.id.to_string(),
                    name: config.name,
                    ip: config.ip.to_string(),
                    online: false,
                    error: e.to_string(),
                }),
            };
            // Serialising our own structs cannot fail; an empty object is a
            // better outcome than a panic in a request thread if it ever does.
            Ok(value.unwrap_or_else(|_| serde_json::json!({})))
        })
        .map_err(|e| e.to_string())
}

/// `GET /api/units/state` — every unit in one call.
///
/// Returns an **envelope**, not an array: `{"states": [...], "errors": [...]}`.
/// That shape is deliberate in Breeze Core and load-bearing — a single
/// unreachable air conditioner lands in `errors` while the rest still come back,
/// so one unplugged unit never 503s the whole batch or blanks a panel. Clients
/// read both keys.
pub fn all_states(manager: &DeviceManager) -> serde_json::Value {
    let ids = manager.known_units();

    // One thread per unit. Each unit has its own lock, so these genuinely run in
    // parallel and the batch costs one round-trip instead of N. Serially this
    // measured 5.4s for three units against Python's 1.8s -- the whole point of
    // per-unit locking is to make a batch read feel like a single one.
    let results: Vec<(u64, Result<serde_json::Value, String>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = ids
            .iter()
            .map(|id| {
                let id = *id;
                scope.spawn(move || (id, unit_state(manager, id)))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join().unwrap_or_else(|_| {
                    // A panicked worker must not take the batch down with it.
                    (0, Err("worker panicked".to_string()))
                })
            })
            .collect()
    });

    // Reassemble in configuration order: threads finish in whatever order the
    // air conditioners answer, and a panel that reorders its cards on every
    // refresh is worse than a slow one.
    let mut states = Vec::new();
    let mut errors = Vec::new();
    for id in &ids {
        let Some((_, result)) = results.iter().find(|(rid, _)| rid == id) else {
            continue;
        };
        match result {
            Ok(value) => {
                if value.get("online").and_then(|v| v.as_bool()) == Some(false) {
                    errors.push(value.clone());
                } else {
                    states.push(value.clone());
                }
            }
            Err(e) => errors.push(serde_json::json!({
                "id": id.to_string(),
                "online": false,
                "error": e,
            })),
        }
    }
    serde_json::json!({ "states": states, "errors": errors })
}

#[cfg(test)]
mod tests {
    use super::*;
    use breeze_device::UnitConfig;
    use breeze_proto::ac::types::{FanSpeed, Mode, SwingMode};
    use std::net::{IpAddr, Ipv4Addr};

    fn state() -> State {
        let mut p = vec![0u8; 24];
        p[0] = 0xC0;
        p[1] = 0x01;
        p[2] = ((Mode::Cool as u8) << 5) | 0x08; // 24 C, cool
        p[3] = 102;
        p[7] = SwingMode::Both as u8;
        p[11] = 0xFF; // no indoor sensor
        p[12] = 0xFF; // no outdoor sensor
        State::parse(&p).unwrap()
    }

    fn device() -> Device {
        Device::new(UnitConfig {
            id: 153_931_628_470_980,
            name: "Lijeva Soba".into(),
            ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 73)),
            port: 6444,
            token: None,
            key: None,
        })
    }

    #[test]
    fn the_id_is_a_string_because_clients_put_it_in_urls() {
        let json = serde_json::to_value(UnitState::from_device(&device(), &state())).unwrap();
        assert!(
            json["id"].is_string(),
            "id must be a string, got {}",
            json["id"]
        );
        assert_eq!(json["id"], "153931628470980");
        // A 48-bit id must survive intact -- no float rounding, no truncation.
        assert_eq!(
            json["id"].as_str().unwrap().parse::<u64>().unwrap(),
            153_931_628_470_980
        );
    }

    #[test]
    fn field_names_match_the_python_serializer() {
        let json = serde_json::to_value(UnitState::from_device(&device(), &state())).unwrap();
        let obj = json.as_object().unwrap();
        let expected = [
            "id",
            "name",
            "ip",
            "online",
            "power_state",
            "operational_mode",
            "target_temperature",
            "indoor_temperature",
            "outdoor_temperature",
            "fan_speed",
            "swing_mode",
            "eco",
            "turbo",
        ];
        for key in expected {
            assert!(obj.contains_key(key), "missing {key}");
        }
        assert_eq!(
            obj.len(),
            expected.len(),
            "unexpected extra fields: {obj:?}"
        );
    }

    #[test]
    fn enums_are_names_not_numbers() {
        // Python 3.11 changed IntEnum.__str__ to render a bare int, which bit
        // this project once. The names are the contract.
        let json = serde_json::to_value(UnitState::from_device(&device(), &state())).unwrap();
        assert_eq!(json["operational_mode"], "COOL");
        assert_eq!(json["swing_mode"], "BOTH");
    }

    #[test]
    fn an_absent_sensor_is_null_not_a_number() {
        let json = serde_json::to_value(UnitState::from_device(&device(), &state())).unwrap();
        assert!(json["indoor_temperature"].is_null());
        assert!(json["outdoor_temperature"].is_null());
    }

    #[test]
    fn an_unrecognised_mode_is_reported_as_unknown() {
        let mut p = vec![0u8; 24];
        p[0] = 0xC0;
        p[2] = 0x7 << 5; // undocumented
        p[7] = 0x7; // undocumented
        let s = State::parse(&p).unwrap();
        let json = serde_json::to_value(UnitState::from_device(&device(), &s)).unwrap();
        assert_eq!(json["operational_mode"], "UNKNOWN");
        assert_eq!(json["swing_mode"], "UNKNOWN");
    }

    #[test]
    fn fan_speed_is_a_number_and_keeps_the_auto_sentinel() {
        let json = serde_json::to_value(UnitState::from_device(&device(), &state())).unwrap();
        assert_eq!(json["fan_speed"], 102);
        assert!(json["fan_speed"].is_number());
        assert_eq!(FanSpeed::AUTO.0, 102);
    }

    #[test]
    fn the_batch_route_is_an_envelope_not_an_array() {
        // Breeze Core's shape, and load-bearing: one unreachable unit lands in
        // `errors` while the rest still arrive in `states`, so a single unplugged
        // air conditioner never 503s the batch. A bare array here would make
        // every client's batch read fail.
        let manager = DeviceManager::new(std::iter::empty());
        let value = all_states(&manager);
        assert!(value.is_object(), "must be an envelope, got {value}");
        assert!(value["states"].is_array(), "missing states");
        assert!(value["errors"].is_array(), "missing errors");
    }

    #[test]
    fn an_offline_unit_is_a_record_not_an_error() {
        let json = serde_json::to_value(UnitOffline {
            id: "1".into(),
            name: "Room".into(),
            ip: "192.0.2.1".into(),
            online: false,
            error: "no response after 3 attempts".into(),
        })
        .unwrap();
        assert_eq!(json["online"], false);
        assert!(json["error"].is_string());
        // Identity must still be present so a client can show which unit it is.
        assert_eq!(json["id"], "1");
        assert_eq!(json["name"], "Room");
    }
}
