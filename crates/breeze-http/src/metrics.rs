//! `GET /metrics` — Prometheus text exposition.
//!
//! Reads **only cached values**: the last sample the history buffer saw. A
//! scrape never triggers a LAN round-trip, which matters because Prometheus
//! scrapes every 15 seconds and an air conditioner takes ~700 ms to answer —
//! pointing a monitoring system at this must not turn into pointing it at three
//! air conditioners. A unit therefore appears only once something has read it.
//!
//! Behind the API key, like `/api/version`: indoor temperature tells you whether
//! somebody is home, so a public deployment must not serve it unauthenticated.
//!
//! Hand-written, with no client library. The format is a few lines of text and
//! the dependency would be larger than the code.

use crate::respond::Reply;
use crate::state::AppState;

/// Escape a label *value* per the exposition format.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push(' '),
            other => out.push(other),
        }
    }
    out
}

/// Render a float the way Prometheus expects.
///
/// Rust prints `24` for 24.0_f64; Prometheus accepts that, but the reference
/// emits `24.0` and a diff between the two should not trip over formatting.
fn number(v: f64) -> String {
    if v.fract() == 0.0 && v.is_finite() {
        format!("{v:.1}")
    } else {
        v.to_string()
    }
}

/// One metric family: help, type, then its rows. Emitted only if it has rows —
/// a family with no samples is noise in a scrape.
fn family(out: &mut String, name: &str, kind: &str, help: &str, rows: &[String]) {
    if rows.is_empty() {
        return;
    }
    out.push_str(&format!("# HELP {name} {help}\n"));
    out.push_str(&format!("# TYPE {name} {kind}\n"));
    for row in rows {
        out.push_str(row);
        out.push('\n');
    }
}

pub fn render(state: &AppState) -> Reply {
    let mut out = String::new();

    family(
        &mut out,
        "breeze_build_info",
        "gauge",
        "Build info (constant 1).",
        &[format!(
            "breeze_build_info{{version=\"{}\",commit=\"{}\"}} 1",
            escape(state.version),
            escape(crate::build_commit())
        )],
    );

    let mut online = Vec::new();
    let mut indoor = Vec::new();
    let mut target = Vec::new();
    let mut outdoor = Vec::new();

    let names = unit_names(state);
    for id in state.manager.known_units() {
        let unit_id = id.to_string();
        // Cached only. A unit nothing has read yet is absent rather than zero:
        // a fabricated 0 °C would be indistinguishable from a real reading.
        let Some(sample) = state.history.latest(&unit_id) else {
            continue;
        };
        let name = names
            .iter()
            .find(|(uid, _)| *uid == unit_id)
            .map(|(_, n)| n.as_str())
            .unwrap_or("");
        let labels = format!("unit=\"{}\",name=\"{}\"", escape(&unit_id), escape(name));

        online.push(format!(
            "breeze_unit_online{{{labels}}} {}",
            u8::from(sample.online)
        ));
        target.push(format!(
            "breeze_unit_target_temperature_celsius{{{labels}}} {}",
            number(sample.target_temperature)
        ));
        if let Some(v) = sample.indoor_temperature {
            indoor.push(format!(
                "breeze_unit_indoor_temperature_celsius{{{labels}}} {}",
                number(v)
            ));
        }
        if let Some(v) = sample.outdoor_temperature {
            outdoor.push(format!(
                "breeze_unit_outdoor_temperature_celsius{{{labels}}} {}",
                number(v)
            ));
        }
    }

    family(
        &mut out,
        "breeze_unit_online",
        "gauge",
        "1 if the unit was reachable at last poll.",
        &online,
    );
    family(
        &mut out,
        "breeze_unit_indoor_temperature_celsius",
        "gauge",
        "Last indoor temperature.",
        &indoor,
    );
    family(
        &mut out,
        "breeze_unit_target_temperature_celsius",
        "gauge",
        "Last target temperature.",
        &target,
    );
    family(
        &mut out,
        "breeze_unit_outdoor_temperature_celsius",
        "gauge",
        "Last outdoor temperature.",
        &outdoor,
    );

    let (runs, errors) = match state.scheduler.lock() {
        Ok(s) => (s.runs, s.errors),
        Err(_) => (0, 0),
    };
    family(
        &mut out,
        "breeze_units_total",
        "gauge",
        "Configured units.",
        &[format!(
            "breeze_units_total {}",
            state.manager.known_units().len()
        )],
    );
    family(
        &mut out,
        "breeze_scheduler_runs_total",
        "counter",
        "Scheduler tick actions fired.",
        &[format!("breeze_scheduler_runs_total {runs}")],
    );
    family(
        &mut out,
        "breeze_scheduler_errors_total",
        "counter",
        "Scheduler errors.",
        &[format!("breeze_scheduler_errors_total {errors}")],
    );

    Reply {
        status: 200,
        body: out.into_bytes(),
        content_type: "text/plain; version=0.0.4",
        extra: Vec::new(),
    }
}

/// `(id, name)` for every configured unit.
fn unit_names(state: &AppState) -> Vec<(String, String)> {
    state
        .config
        .read()
        .map(|c| {
            c.units
                .iter()
                .map(|u| (u.id.to_string(), u.name.clone()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_values_are_escaped() {
        // A unit name is user-supplied and lands inside a quoted label. An
        // unescaped quote would produce a line Prometheus rejects, taking the
        // whole scrape down with it.
        assert_eq!(escape("Lijeva Soba"), "Lijeva Soba");
        assert_eq!(escape("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(escape("back\\slash"), "back\\\\slash");
        assert_eq!(escape("two\nlines"), "two lines");
        // Non-ASCII is legal in a label value and must pass through intact.
        assert_eq!(escape("Erkondišn ❄"), "Erkondišn ❄");
    }

    #[test]
    fn whole_numbers_print_with_a_decimal() {
        assert_eq!(number(24.0), "24.0");
        assert_eq!(number(24.5), "24.5");
        assert_eq!(number(-3.0), "-3.0");
        assert_eq!(number(0.0), "0.0");
    }

    #[test]
    fn an_empty_family_is_omitted_entirely() {
        // Rather than a HELP/TYPE pair with no samples under it.
        let mut out = String::new();
        family(&mut out, "x", "gauge", "help", &[]);
        assert!(out.is_empty());

        family(&mut out, "x", "gauge", "help", &["x 1".into()]);
        assert_eq!(out, "# HELP x help\n# TYPE x gauge\nx 1\n");
    }

    #[test]
    fn every_line_is_a_comment_or_a_sample() {
        // The shape Prometheus requires: no blank lines, and every non-comment
        // line ending in a value.
        let mut out = String::new();
        family(
            &mut out,
            "breeze_unit_online",
            "gauge",
            "1 if reachable.",
            &["breeze_unit_online{unit=\"7\",name=\"Kuhinja\"} 1".into()],
        );
        for line in out.lines() {
            assert!(!line.is_empty(), "no blank lines");
            if !line.starts_with('#') {
                let value = line.rsplit(' ').next().unwrap();
                assert!(
                    value.parse::<f64>().is_ok(),
                    "line does not end in a number: {line}"
                );
            }
        }
    }
}
