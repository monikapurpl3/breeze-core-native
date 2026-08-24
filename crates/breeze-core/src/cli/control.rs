//! `breeze-core control 'NAME' TYPE TEMPERATURE FLAP FAN EXTRA TIMER`
//!
//! ```text
//! breeze-core control 'kuhinja'     cool 25.5 both       auto turbo 15m
//! breeze-core control 'lijeva soba' heat 30   horizontal low  eco
//! breeze-core control 'dnevna soba' dry  26   vertical   high      25m
//! breeze-core control 'kuhinja'     off
//! ```
//!
//! Positional, because that is what makes it worth typing: the order is the
//! order somebody thinks in, and anything they do not care about is `none`.
//! Every slot after the name is optional, and `none` is legal in any of them —
//! so `control kuhinja none none none none none 30m` is a perfectly good way to
//! say "switch off in half an hour, change nothing else".
//!
//! A field left out or set to `none` is not sent at all. The API applies only
//! the fields present, so nothing else about the unit is disturbed.
//!
//! The name is matched case-insensitively, and by prefix when that is
//! unambiguous, because `'lijeva soba'` is tedious to type exactly and `lij` is
//! not.

use crate::cli::client::Client;

/// What the seven positions mean, once parsed.
#[derive(Debug, Default, PartialEq)]
pub struct Command {
    /// `off` switches the unit off; any mode switches it on.
    pub power: Option<bool>,
    pub mode: Option<&'static str>,
    pub target_temperature: Option<f64>,
    pub swing_mode: Option<&'static str>,
    pub fan_speed: Option<i64>,
    pub eco: Option<bool>,
    pub turbo: Option<bool>,
    /// Minutes after which to switch the unit off.
    pub timer_minutes: Option<u32>,
}

impl Command {
    /// The control request body, with only the fields that were asked for.
    pub fn body(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        if let Some(v) = self.power {
            map.insert("power_state".into(), serde_json::json!(v));
        }
        if let Some(v) = self.mode {
            map.insert("operational_mode".into(), serde_json::json!(v));
        }
        if let Some(v) = self.target_temperature {
            map.insert("target_temperature".into(), serde_json::json!(v));
        }
        if let Some(v) = self.swing_mode {
            map.insert("swing_mode".into(), serde_json::json!(v));
        }
        if let Some(v) = self.fan_speed {
            map.insert("fan_speed".into(), serde_json::json!(v));
        }
        if let Some(v) = self.eco {
            map.insert("eco".into(), serde_json::json!(v));
        }
        if let Some(v) = self.turbo {
            map.insert("turbo".into(), serde_json::json!(v));
        }
        serde_json::Value::Object(map)
    }

    /// Whether there is anything to send to the unit, timer aside.
    pub fn touches_the_unit(&self) -> bool {
        !self.body().as_object().is_some_and(|m| m.is_empty())
    }
}

/// `none`, `-`, and an empty string all mean "leave this alone".
fn skipped(word: &str) -> bool {
    matches!(word, "none" | "-" | "" | "same" | "keep")
}

/// Parse the positional arguments after the name.
pub fn parse(words: &[String]) -> Result<Command, String> {
    if words.len() > 6 {
        return Err(format!(
            "too many arguments: expected at most TYPE TEMPERATURE FLAP FAN EXTRA TIMER, got {}",
            words.len()
        ));
    }
    let mut command = Command::default();

    // Each slot is parsed by position, but a timer is also accepted anywhere:
    // "15m" is unmistakable, and insisting it come seventh when the middle
    // slots are `none` is a rule with no purpose.
    let mut positional: Vec<&str> = Vec::new();
    for word in words {
        let lower = word.to_ascii_lowercase();
        match parse_timer(&lower) {
            Some(minutes) => {
                if command.timer_minutes.is_some() {
                    return Err("two timers were given".into());
                }
                command.timer_minutes = Some(minutes);
            }
            None => positional.push(word.as_str()),
        }
    }

    for (index, word) in positional.iter().enumerate() {
        let lower = word.to_ascii_lowercase();
        if skipped(&lower) {
            continue;
        }
        match index {
            0 => apply_type(&mut command, &lower)?,
            1 => command.target_temperature = Some(parse_temperature(&lower)?),
            2 => command.swing_mode = Some(parse_flap(&lower)?),
            3 => command.fan_speed = Some(parse_fan(&lower)?),
            4 => apply_extra(&mut command, &lower)?,
            _ => return Err(format!("'{word}' is one argument too many")),
        }
    }
    Ok(command)
}

fn apply_type(command: &mut Command, word: &str) -> Result<(), String> {
    match word {
        "off" => {
            command.power = Some(false);
            Ok(())
        }
        "on" => {
            // Useful on its own: "switch it back on, whatever it was doing".
            command.power = Some(true);
            Ok(())
        }
        other => {
            let mode = match other {
                "cool" | "cooling" => "COOL",
                "heat" | "heating" => "HEAT",
                "dry" | "dehumidify" => "DRY",
                "auto" => "AUTO",
                "fan" | "fan_only" | "fanonly" | "ventilate" => "FAN_ONLY",
                _ => {
                    return Err(format!(
                        "'{other}' is not a mode. Use cool, heat, dry, auto, fan, on, off or none"
                    ))
                }
            };
            command.mode = Some(mode);
            // A mode implies the unit should be running. Setting a mode on a
            // unit that is off and leaving it off is never what was meant.
            command.power = Some(true);
            Ok(())
        }
    }
}

fn parse_temperature(word: &str) -> Result<f64, String> {
    let value: f64 = word
        .trim_end_matches(['c', 'C', '°'])
        .parse()
        .map_err(|_| format!("'{word}' is not a temperature"))?;
    if !(16.0..=30.0).contains(&value) {
        return Err(format!("{value} is outside the 16-30 °C the API accepts"));
    }
    if (value * 2.0).fract() != 0.0 {
        return Err(format!("{value} is not a whole or half degree"));
    }
    Ok(value)
}

fn parse_flap(word: &str) -> Result<&'static str, String> {
    match word {
        "both" | "all" => Ok("BOTH"),
        "horizontal" | "h" | "lr" | "leftright" => Ok("HORIZONTAL"),
        "vertical" | "v" | "ud" | "updown" => Ok("VERTICAL"),
        "off" | "still" | "fixed" => Ok("OFF"),
        _ => Err(format!(
            "'{word}' is not a flap setting. Use both, horizontal, vertical, off or none"
        )),
    }
}

fn parse_fan(word: &str) -> Result<i64, String> {
    match word {
        "silent" | "quiet" => Ok(20),
        "low" | "l" => Ok(40),
        "medium" | "med" | "m" => Ok(60),
        "high" | "h" => Ok(80),
        "max" | "full" => Ok(100),
        "auto" | "a" => Ok(102),
        // A bare percentage, for a unit that takes custom speeds.
        number => {
            let value: i64 = number
                .trim_end_matches('%')
                .parse()
                .map_err(|_| format!("'{number}' is not a fan speed. Use silent, low, medium, high, max, auto, none, or a percentage"))?;
            if ![20, 40, 60, 80, 100, 102].contains(&value) {
                return Err(format!(
                    "{value} is not one of the speeds the API accepts (20, 40, 60, 80, 100, 102)"
                ));
            }
            Ok(value)
        }
    }
}

fn apply_extra(command: &mut Command, word: &str) -> Result<(), String> {
    // Both at once is accepted as `turbo+eco`, because the API has two
    // independent flags and refusing the combination would be this CLI
    // inventing a rule the server does not have.
    for part in word.split(['+', ',']) {
        match part.trim() {
            "" => {}
            "turbo" | "boost" => command.turbo = Some(true),
            "eco" | "saving" => command.eco = Some(true),
            "noturbo" | "no-turbo" => command.turbo = Some(false),
            "noeco" | "no-eco" => command.eco = Some(false),
            other => {
                return Err(format!(
                    "'{other}' is not an extra. Use turbo, eco, no-turbo, no-eco or none"
                ))
            }
        }
    }
    Ok(())
}

/// `15m`, `90min`, `2h`, `45` — anything that is plainly a duration.
fn parse_timer(word: &str) -> Option<u32> {
    let (digits, unit) = word.split_at(word.find(|c: char| !c.is_ascii_digit())?);
    if digits.is_empty() {
        return None;
    }
    let value: u32 = digits.parse().ok()?;
    match unit {
        "m" | "min" | "mins" | "minute" | "minutes" => Some(value),
        "h" | "hr" | "hrs" | "hour" | "hours" => Some(value * 60),
        _ => None,
    }
}

/// Find a unit by name, case-insensitively.
///
/// Exact match wins; then a unique case-insensitive match; then a unique prefix.
/// Ambiguity is an error that lists the candidates rather than picking one —
/// switching the wrong room's heating on is not a recoverable guess.
pub fn resolve_unit(units: &serde_json::Value, wanted: &str) -> Result<(String, String), String> {
    let list = units
        .as_array()
        .ok_or("the server did not return a list of units")?;
    let named: Vec<(String, String)> = list
        .iter()
        .filter_map(|u| {
            Some((
                u.get("id")?.as_str()?.to_string(),
                u.get("name")?.as_str()?.to_string(),
            ))
        })
        .collect();

    if named.is_empty() {
        return Err("this server has no units configured".into());
    }

    let target = wanted.trim().to_lowercase();
    // An id, for scripts and for a unit whose name is ambiguous.
    if let Some(found) = named.iter().find(|(id, _)| *id == wanted.trim()) {
        return Ok(found.clone());
    }
    let exact: Vec<&(String, String)> = named
        .iter()
        .filter(|(_, name)| name.to_lowercase() == target)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0].clone());
    }
    if exact.len() > 1 {
        return Err(format!(
            "'{wanted}' matches {} units by name; use an id instead",
            exact.len()
        ));
    }

    let prefixed: Vec<&(String, String)> = named
        .iter()
        .filter(|(_, name)| name.to_lowercase().starts_with(&target))
        .collect();
    match prefixed.len() {
        1 => Ok(prefixed[0].clone()),
        0 => Err(format!(
            "no unit matches '{wanted}'. Known units: {}",
            named
                .iter()
                .map(|(_, n)| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
        _ => Err(format!(
            "'{wanted}' is ambiguous -- it could be {}",
            prefixed
                .iter()
                .map(|(_, n)| n.as_str())
                .collect::<Vec<_>>()
                .join(" or ")
        )),
    }
}

/// Run the command.
pub fn run(args: &[String]) -> Result<(), String> {
    let Some(name) = args.first() else {
        return Err(usage());
    };
    if name == "--help" || name == "-h" {
        println!("{}", usage());
        return Ok(());
    }
    let command = parse(&args[1..])?;
    if !command.touches_the_unit() && command.timer_minutes.is_none() {
        return Err(format!(
            "nothing to do -- every setting was 'none'.\n\n{}",
            usage()
        ));
    }

    let profile = crate::cli::profile::ensure()?;
    let client = Client::from_profile(&profile);

    let units = client.get("/api/units")?;
    let (id, resolved) = resolve_unit(&units, name)?;

    if command.touches_the_unit() {
        let state = client.post_json(&format!("/api/units/{id}/control"), &command.body())?;
        println!("{resolved}: {}", describe(&state));
    }

    if let Some(minutes) = command.timer_minutes {
        // A timer is a separate promise, so it is a separate call. Default
        // action: switch off -- which is what a duration after a control
        // command means to anybody who has used a sleep timer.
        let timer = client.post_json(
            "/api/timers",
            &serde_json::json!({
                "unit_ids": [id],
                "minutes": minutes,
                "label": "breeze-core control",
            }),
        )?;
        let at = timer["fires_at"].as_str().unwrap_or("?");
        println!("{resolved}: switching off in {minutes} minutes, at {at}");
    }
    Ok(())
}

/// One line of what the unit now reports.
fn describe(state: &serde_json::Value) -> String {
    let field = |key: &str| state.get(key).cloned().unwrap_or(serde_json::Value::Null);
    let temperature = |value: &serde_json::Value| match value.as_f64() {
        Some(v) => format!("{v:.1}"),
        None => "--".to_string(),
    };

    if field("power_state").as_bool() == Some(false) {
        return format!(
            "off (indoor {} °C)",
            temperature(&field("indoor_temperature"))
        );
    }
    let mut parts = vec![
        field("operational_mode")
            .as_str()
            .unwrap_or("?")
            .to_string(),
        format!("{} °C", temperature(&field("target_temperature"))),
    ];
    if let Some(swing) = field("swing_mode").as_str() {
        if swing != "OFF" {
            parts.push(format!("swing {}", swing.to_lowercase()));
        }
    }
    if let Some(fan) = field("fan_speed").as_i64() {
        parts.push(format!(
            "fan {}",
            match fan {
                20 => "silent".to_string(),
                40 => "low".to_string(),
                60 => "medium".to_string(),
                80 => "high".to_string(),
                100 => "max".to_string(),
                102 => "auto".to_string(),
                other => format!("{other}%"),
            }
        ));
    }
    if field("eco").as_bool() == Some(true) {
        parts.push("eco".into());
    }
    if field("turbo").as_bool() == Some(true) {
        parts.push("turbo".into());
    }
    parts.push(format!(
        "indoor {} °C",
        temperature(&field("indoor_temperature"))
    ));
    parts.join(", ")
}

pub fn usage() -> String {
    "usage: breeze-core control 'NAME' [TYPE] [TEMPERATURE] [FLAP] [FAN] [EXTRA] [TIMER]

  NAME         a unit's name, case-insensitive, or a unique prefix, or its id
  TYPE         cool | heat | dry | auto | fan | on | off
  TEMPERATURE  16-30, in half degrees
  FLAP         both | horizontal | vertical | off
  FAN          silent | low | medium | high | max | auto
  EXTRA        turbo | eco | turbo+eco | no-turbo | no-eco
  TIMER        15m | 90min | 2h -- switches the unit off after that long

  'none' is valid in any position and leaves that setting alone. Every slot
  after NAME may be omitted.

examples:
  breeze-core control 'kuhinja' cool 25.5 both auto turbo 15m
  breeze-core control 'lijeva soba' heat 30 horizontal low eco
  breeze-core control 'dnevna soba' dry 26 vertical high 25m
  breeze-core control kuhinja off
  breeze-core control kuhinja none none none none none 30m"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_first_example_from_the_specification() {
        // control 'kuhinja' cool 25.5 both auto turbo 15m
        let command = parse(&words("cool 25.5 both auto turbo 15m")).unwrap();
        assert_eq!(command.mode, Some("COOL"));
        assert_eq!(command.power, Some(true), "a mode implies switching on");
        assert_eq!(command.target_temperature, Some(25.5));
        assert_eq!(command.swing_mode, Some("BOTH"));
        assert_eq!(command.fan_speed, Some(102));
        assert_eq!(command.turbo, Some(true));
        assert_eq!(command.eco, None, "eco was not mentioned");
        assert_eq!(command.timer_minutes, Some(15));
    }

    #[test]
    fn the_second_example_has_no_timer() {
        // control 'lijeva soba' heat 30 horizontal low eco
        let command = parse(&words("heat 30 horizontal low eco")).unwrap();
        assert_eq!(command.mode, Some("HEAT"));
        assert_eq!(command.target_temperature, Some(30.0));
        assert_eq!(command.swing_mode, Some("HORIZONTAL"));
        assert_eq!(command.fan_speed, Some(40));
        assert_eq!(command.eco, Some(true));
        assert_eq!(command.turbo, None);
        assert_eq!(command.timer_minutes, None);
    }

    #[test]
    fn the_third_example_skips_the_extra_and_keeps_the_timer() {
        // control 'dnevna soba' dry 26 vertical high 25m
        let command = parse(&words("dry 26 vertical high 25m")).unwrap();
        assert_eq!(command.mode, Some("DRY"));
        assert_eq!(command.target_temperature, Some(26.0));
        assert_eq!(command.swing_mode, Some("VERTICAL"));
        assert_eq!(command.fan_speed, Some(80));
        assert_eq!(command.timer_minutes, Some(25));
        assert_eq!(command.turbo, None);
        assert_eq!(command.eco, None);
    }

    #[test]
    fn none_is_valid_in_every_position() {
        let command = parse(&words("none none none none none")).unwrap();
        assert_eq!(command, Command::default());
        assert!(
            !command.touches_the_unit(),
            "nothing was asked for, so nothing is sent"
        );
    }

    #[test]
    fn none_in_the_middle_leaves_only_what_was_named() {
        let command = parse(&words("cool none both none eco")).unwrap();
        assert_eq!(command.mode, Some("COOL"));
        assert_eq!(command.target_temperature, None, "temperature untouched");
        assert_eq!(command.swing_mode, Some("BOTH"));
        assert_eq!(command.fan_speed, None, "fan untouched");
        assert_eq!(command.eco, Some(true));

        // And the request body carries only those fields.
        let body = command.body();
        let keys: Vec<&String> = body.as_object().unwrap().keys().collect();
        assert!(!keys.iter().any(|k| *k == "target_temperature"));
        assert!(!keys.iter().any(|k| *k == "fan_speed"));
    }

    #[test]
    fn a_timer_alone_is_a_valid_command() {
        // "switch off in half an hour, change nothing else"
        let command = parse(&words("none none none none none 30m")).unwrap();
        assert!(!command.touches_the_unit());
        assert_eq!(command.timer_minutes, Some(30));
    }

    #[test]
    fn off_switches_the_unit_off_and_nothing_else() {
        let command = parse(&words("off")).unwrap();
        assert_eq!(command.power, Some(false));
        assert_eq!(command.mode, None, "off is not a mode");
        let body = command.body();
        assert_eq!(body.as_object().unwrap().len(), 1);
        assert_eq!(body["power_state"], false);
    }

    #[test]
    fn everything_is_case_insensitive() {
        let upper = parse(&words("COOL 25.5 BOTH AUTO TURBO 15M")).unwrap();
        let lower = parse(&words("cool 25.5 both auto turbo 15m")).unwrap();
        assert_eq!(upper, lower);
    }

    #[test]
    fn a_timer_is_recognised_wherever_it_appears() {
        // Insisting it come seventh when the middle slots are `none` is a rule
        // with no purpose.
        assert_eq!(parse(&words("15m")).unwrap().timer_minutes, Some(15));
        assert_eq!(
            parse(&words("cool 15m")).unwrap().timer_minutes,
            Some(15),
            "and the mode still parses"
        );
        assert_eq!(parse(&words("cool 15m")).unwrap().mode, Some("COOL"));
    }

    #[test]
    fn timer_units_are_flexible_but_not_permissive() {
        assert_eq!(parse_timer("15m"), Some(15));
        assert_eq!(parse_timer("90min"), Some(90));
        assert_eq!(parse_timer("2h"), Some(120));
        assert_eq!(parse_timer("45minutes"), Some(45));
        // A bare number is a temperature or a fan speed, not a duration.
        assert_eq!(parse_timer("45"), None);
        assert_eq!(parse_timer("25.5"), None);
        assert_eq!(parse_timer("cool"), None);
        assert_eq!(parse_timer("m"), None);
    }

    #[test]
    fn two_timers_are_refused_rather_than_one_winning_silently() {
        assert!(parse(&words("cool 15m 30m")).is_err());
    }

    #[test]
    fn temperatures_are_bounded_and_half_degrees() {
        assert_eq!(parse_temperature("16").unwrap(), 16.0);
        assert_eq!(parse_temperature("25.5").unwrap(), 25.5);
        assert_eq!(parse_temperature("30").unwrap(), 30.0);
        assert_eq!(
            parse_temperature("24c").unwrap(),
            24.0,
            "a unit suffix is fine"
        );

        assert!(parse_temperature("15.5").is_err(), "below the range");
        assert!(parse_temperature("31").is_err(), "above the range");
        assert!(parse_temperature("24.3").is_err(), "not a half degree");
        assert!(parse_temperature("warm").is_err());
    }

    #[test]
    fn fan_words_and_numbers_both_work() {
        assert_eq!(parse_fan("silent").unwrap(), 20);
        assert_eq!(parse_fan("low").unwrap(), 40);
        assert_eq!(parse_fan("medium").unwrap(), 60);
        assert_eq!(parse_fan("high").unwrap(), 80);
        assert_eq!(parse_fan("max").unwrap(), 100);
        assert_eq!(parse_fan("auto").unwrap(), 102);
        assert_eq!(parse_fan("60").unwrap(), 60);
        assert_eq!(parse_fan("60%").unwrap(), 60);
        // A speed the API would refuse is refused here, before a round-trip.
        assert!(parse_fan("55").is_err());
        assert!(parse_fan("breezy").is_err());
    }

    #[test]
    fn both_extras_at_once_are_allowed() {
        // The API has two independent flags; refusing the combination would be
        // this CLI inventing a rule the server does not have.
        let command = parse(&words("cool none none none turbo+eco")).unwrap();
        assert_eq!(command.turbo, Some(true));
        assert_eq!(command.eco, Some(true));
    }

    #[test]
    fn an_extra_can_be_turned_off() {
        let command = parse(&words("cool none none none no-turbo")).unwrap();
        assert_eq!(command.turbo, Some(false));
    }

    #[test]
    fn a_bad_word_names_the_valid_ones() {
        // The error is the documentation, for somebody who mistyped at 2am.
        let error = parse(&words("freeze")).unwrap_err();
        assert!(error.contains("cool"), "{error}");
        assert!(error.contains("off"), "{error}");

        let error = parse(&words("cool 24 sideways")).unwrap_err();
        assert!(error.contains("horizontal"), "{error}");
    }

    #[test]
    fn too_many_arguments_is_an_error_not_a_silent_truncation() {
        assert!(parse(&words("cool 24 both auto eco 15m extra")).is_err());
    }

    fn units() -> serde_json::Value {
        serde_json::json!([
            {"id": "1", "name": "Kuhinja"},
            {"id": "2", "name": "Lijeva Soba"},
            {"id": "3", "name": "Dnevna Soba"},
        ])
    }

    #[test]
    fn a_name_matches_regardless_of_case() {
        // The whole point of the request: 'kuhinja' must find "Kuhinja".
        for spelling in ["Kuhinja", "kuhinja", "KUHINJA", "kUhInJa", "  kuhinja  "] {
            let (id, name) = resolve_unit(&units(), spelling).unwrap();
            assert_eq!(id, "1", "{spelling} should have matched");
            assert_eq!(name, "Kuhinja");
        }
    }

    #[test]
    fn a_multi_word_name_matches_case_insensitively() {
        let (id, _) = resolve_unit(&units(), "lijeva soba").unwrap();
        assert_eq!(id, "2");
    }

    #[test]
    fn an_unambiguous_prefix_is_enough() {
        assert_eq!(resolve_unit(&units(), "kuh").unwrap().0, "1");
        assert_eq!(resolve_unit(&units(), "lij").unwrap().0, "2");
    }

    #[test]
    fn an_ambiguous_prefix_lists_the_candidates_rather_than_guessing() {
        // Both "Lijeva Soba" and "Dnevna Soba" end in Soba, but neither starts
        // with it -- so use a prefix that really is ambiguous.
        let ambiguous = serde_json::json!([
            {"id": "1", "name": "Soba Jedan"},
            {"id": "2", "name": "Soba Dva"},
        ]);
        let error = resolve_unit(&ambiguous, "soba").unwrap_err();
        assert!(error.contains("ambiguous"), "{error}");
        assert!(error.contains("Soba Jedan"), "{error}");
        assert!(error.contains("Soba Dva"), "{error}");
    }

    #[test]
    fn an_id_still_works_for_scripts() {
        assert_eq!(resolve_unit(&units(), "3").unwrap().1, "Dnevna Soba");
    }

    #[test]
    fn an_unknown_name_lists_what_there_is() {
        let error = resolve_unit(&units(), "garaza").unwrap_err();
        assert!(error.contains("Kuhinja"), "{error}");
        assert!(error.contains("Lijeva Soba"), "{error}");
    }

    #[test]
    fn describing_a_unit_that_is_off_says_so_briefly() {
        let state = serde_json::json!({
            "power_state": false,
            "operational_mode": "COOL",
            "indoor_temperature": 26.5,
        });
        let line = describe(&state);
        assert!(line.starts_with("off"), "{line}");
        assert!(line.contains("26.5"), "{line}");
    }

    #[test]
    fn describing_a_running_unit_reads_like_a_sentence() {
        let state = serde_json::json!({
            "power_state": true,
            "operational_mode": "COOL",
            "target_temperature": 25.5,
            "swing_mode": "BOTH",
            "fan_speed": 102,
            "eco": false,
            "turbo": true,
            "indoor_temperature": 27.0,
        });
        let line = describe(&state);
        assert!(line.contains("COOL"), "{line}");
        assert!(line.contains("25.5 °C"), "{line}");
        assert!(line.contains("swing both"), "{line}");
        assert!(line.contains("fan auto"), "{line}");
        assert!(line.contains("turbo"), "{line}");
        assert!(!line.contains("eco"), "eco was off: {line}");
        assert!(line.contains("indoor 27.0"), "{line}");
    }

    #[test]
    fn a_missing_reading_shows_as_dashes_rather_than_zero() {
        // A unit with no outdoor probe reports null, and "0.0 °C" would be a lie.
        let state = serde_json::json!({"power_state": true, "operational_mode": "AUTO"});
        let line = describe(&state);
        assert!(line.contains("--"), "{line}");
        assert!(!line.contains("0.0"), "{line}");
    }
}
