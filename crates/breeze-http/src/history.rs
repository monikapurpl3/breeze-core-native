//! Recent readings per unit, in memory.
//!
//! A small ring buffer filled opportunistically whenever a state is read — by
//! the state route, the batch route, or the stream's poller. It never polls a
//! unit itself, so it costs nothing: a reading that was going to happen anyway
//! gets remembered on its way past.
//!
//! Deliberately not durable. This is a hint for drawing a graph, not a record,
//! so a restart starting the graph over is the correct behaviour rather than a
//! shortcoming — and it keeps a feature that samples every five seconds from
//! writing to a router's flash for the rest of its life.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::RwLock;

use serde::Serialize;

/// Samples kept per unit. 720 at the stream's 5-second tick is about an hour,
/// which is what a client's graph shows.
pub const DEFAULT_SIZE: usize = 720;

/// One reading, trimmed to what a graph actually plots.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sample {
    /// Unix seconds. Named `t` as the reference names it — a client reads this.
    pub t: i64,
    pub online: bool,
    pub power_state: bool,
    pub operational_mode: String,
    pub target_temperature: f64,
    pub indoor_temperature: Option<f64>,
    pub outdoor_temperature: Option<f64>,
}

/// Per-unit ring buffers.
#[derive(Debug)]
pub struct History {
    by_unit: RwLock<HashMap<String, VecDeque<Sample>>>,
    size: usize,
}

impl History {
    pub fn new(size: usize) -> Self {
        Self {
            by_unit: RwLock::new(HashMap::new()),
            size: size.max(1),
        }
    }

    /// Remember a sample taken from a serialised unit state.
    ///
    /// Takes the wire form rather than a typed state because every caller
    /// already has one, and because an offline unit's record is a different
    /// shape with the same keys.
    pub fn record(&self, state: &serde_json::Value, now: i64) {
        let Some(id) = state.get("id").and_then(|v| v.as_str()) else {
            return;
        };
        // A state with no mode is an offline record from the batch route; still
        // worth keeping, because a gap in the graph is information.
        let sample = Sample {
            t: now,
            online: state
                .get("online")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            power_state: state
                .get("power_state")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            operational_mode: state
                .get("operational_mode")
                .and_then(|v| v.as_str())
                .unwrap_or("UNKNOWN")
                .to_string(),
            target_temperature: state
                .get("target_temperature")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            indoor_temperature: state.get("indoor_temperature").and_then(|v| v.as_f64()),
            outdoor_temperature: state.get("outdoor_temperature").and_then(|v| v.as_f64()),
        };

        if let Ok(mut by_unit) = self.by_unit.write() {
            let buffer = by_unit.entry(id.to_string()).or_default();
            if buffer.len() >= self.size {
                buffer.pop_front();
            }
            buffer.push_back(sample);
        }
    }

    /// Every sample held for a unit, oldest first.
    pub fn samples(&self, unit_id: &str) -> Vec<Sample> {
        self.by_unit
            .read()
            .ok()
            .and_then(|by_unit| by_unit.get(unit_id).map(|b| b.iter().cloned().collect()))
            .unwrap_or_default()
    }

    /// The most recent sample, for `/metrics` to report a last-known value.
    pub fn latest(&self, unit_id: &str) -> Option<Sample> {
        self.by_unit
            .read()
            .ok()
            .and_then(|by_unit| by_unit.get(unit_id).and_then(|b| b.back().cloned()))
    }

    /// How many units have any history, for diagnostics.
    pub fn tracked_units(&self) -> usize {
        self.by_unit.read().map(|b| b.len()).unwrap_or(0)
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new(DEFAULT_SIZE)
    }
}

/// Unix seconds now.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(id: &str, indoor: Option<f64>) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "name": "Kuhinja",
            "online": true,
            "power_state": true,
            "operational_mode": "COOL",
            "target_temperature": 24.0,
            "indoor_temperature": indoor,
            "outdoor_temperature": 28.5,
        })
    }

    #[test]
    fn a_sample_keeps_only_what_a_graph_plots() {
        let history = History::default();
        history.record(&state("7", Some(26.0)), 1_787_500_000);
        let samples = history.samples("7");
        assert_eq!(samples.len(), 1);
        let s = &samples[0];
        assert_eq!(s.t, 1_787_500_000);
        assert_eq!(s.operational_mode, "COOL");
        assert_eq!(s.target_temperature, 24.0);
        assert_eq!(s.indoor_temperature, Some(26.0));
        assert_eq!(s.outdoor_temperature, Some(28.5));
        // The name and address are not plotted and are not kept.
        let json = serde_json::to_string(s).unwrap();
        assert!(!json.contains("Kuhinja"), "{json}");
    }

    #[test]
    fn the_buffer_is_bounded_and_drops_the_oldest() {
        let history = History::new(3);
        for t in 0..10 {
            history.record(&state("7", Some(20.0 + t as f64)), t);
        }
        let samples = history.samples("7");
        assert_eq!(samples.len(), 3, "bounded to its size");
        assert_eq!(samples[0].t, 7, "oldest kept is the 8th");
        assert_eq!(samples[2].t, 9, "newest last");
    }

    #[test]
    fn units_are_kept_apart() {
        let history = History::default();
        history.record(&state("7", Some(26.0)), 1);
        history.record(&state("8", Some(21.0)), 2);
        assert_eq!(history.samples("7").len(), 1);
        assert_eq!(history.samples("8").len(), 1);
        assert_eq!(history.samples("7")[0].indoor_temperature, Some(26.0));
        assert_eq!(history.tracked_units(), 2);
    }

    #[test]
    fn an_unknown_unit_has_no_samples_rather_than_an_error() {
        let history = History::default();
        assert!(history.samples("nope").is_empty());
        assert!(history.latest("nope").is_none());
    }

    #[test]
    fn a_state_with_no_id_is_ignored() {
        // The batch route can produce an error record; without an id there is
        // nothing to file it under, and inventing a key would corrupt a graph.
        let history = History::default();
        history.record(&serde_json::json!({"online": false}), 1);
        assert_eq!(history.tracked_units(), 0);
    }

    #[test]
    fn an_offline_record_is_still_recorded() {
        // A gap matters: a graph that simply stops is indistinguishable from a
        // server that stopped, while an offline sample says which happened.
        let history = History::default();
        history.record(
            &serde_json::json!({"id": "7", "online": false, "error": "unreachable"}),
            5,
        );
        let samples = history.samples("7");
        assert_eq!(samples.len(), 1);
        assert!(!samples[0].online);
        assert_eq!(samples[0].operational_mode, "UNKNOWN");
        assert_eq!(samples[0].indoor_temperature, None);
    }

    #[test]
    fn latest_is_the_newest_sample() {
        let history = History::default();
        history.record(&state("7", Some(20.0)), 1);
        history.record(&state("7", Some(25.0)), 2);
        assert_eq!(history.latest("7").unwrap().indoor_temperature, Some(25.0));
        assert_eq!(history.latest("7").unwrap().t, 2);
    }

    #[test]
    fn a_zero_size_still_keeps_one() {
        // Rather than a buffer that silently records nothing.
        let history = History::new(0);
        history.record(&state("7", Some(20.0)), 1);
        assert_eq!(history.samples("7").len(), 1);
    }
}
