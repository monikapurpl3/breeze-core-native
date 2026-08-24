//! `GET /api/config` and the unit-management routes beside it.
//!
//! A client's own way to see and edit the unit list, without an admin shelling
//! into the server. Behind full auth, like control: adding a unit is as
//! consequential as switching one on.
//!
//! **Never returns a secret.** No `api_key`, and no per-unit V3 `token`/`key` —
//! a unit's view carries a `has_v3_credentials` boolean instead, which is all a
//! client needs to know. That is not a detail to relax: the config API is the
//! one endpoint where leaking would hand over the whole deployment.

use std::net::Ipv4Addr;
use std::time::Duration;

use breeze_store::UnitConfig;
use serde::Deserialize;

use crate::respond::Reply;
use crate::state::AppState;

/// As long a name as a program's, for the same reason: it goes in a UI.
const NAME_MAX_CHARS: usize = 64;

/// How long a scan listens. Shorter than the device layer's default because a
/// person is watching a spinner.
const SCAN_LISTEN: Duration = Duration::from_secs(3);

#[derive(Debug, Deserialize)]
pub struct RenameRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct AddUnitRequest {
    pub ip: String,
    #[serde(default)]
    pub name: Option<String>,
    /// V3 credentials, if the caller already has them.
    ///
    /// A V3 unit cannot be driven without a `token` and `key`, and those come
    /// from Midea's cloud -- which only answers for the account the unit is
    /// registered to. Accepting them here is what makes this server usable
    /// without a cloud round-trip of its own: restoring from a backup, moving to
    /// a new machine, or pasting what another tool fetched.
    ///
    /// Hex, and validated before anything is written: a malformed credential
    /// stored now is a unit that fails to authenticate later, with nothing to
    /// say why.
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub key: Option<String>,

    /// A Midea account, used **once** to fetch the credentials above and then
    /// dropped.
    ///
    /// The last resort, and only the account the unit is registered to will
    /// work: Midea has withdrawn token fetching from its other apps, and the one
    /// still answering refuses any account that does not own the appliance.
    /// Nothing here is stored, logged, or echoed back -- see `breeze-cloud`.
    #[serde(default)]
    pub midea_account: Option<String>,
    #[serde(default)]
    pub midea_password: Option<String>,
    /// `DE`, `KR` or `US`. Only the account's own region will accept it.
    #[serde(default)]
    pub midea_region: Option<String>,
}

/// A V3 token is 64 bytes and a key is 32, both as hex.
const TOKEN_HEX_LEN: usize = 128;
const KEY_HEX_LEN: usize = 64;

/// Check a credential is hex of the right length.
fn valid_hex(value: &str, expected_len: usize) -> bool {
    value.len() == expected_len && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A unit as a client may see it: identity and address, never a credential.
fn unit_view(unit: &UnitConfig) -> serde_json::Value {
    serde_json::json!({
        "id": unit.id.to_string(),
        "name": unit.name,
        "ip": unit.ip,
        "port": unit.port,
        "has_v3_credentials": unit.token.is_some() && unit.key.is_some(),
    })
}

/// `GET /api/config`
pub fn get_config(state: &AppState) -> Reply {
    let config = match state.config.read() {
        Ok(c) => c,
        Err(_) => return Reply::detail(500, "config unavailable"),
    };
    Reply::json(
        200,
        &serde_json::json!({
            "units": config.units.iter().map(unit_view).collect::<Vec<_>>(),
        }),
    )
}

/// `PATCH /api/units/{id}` — rename.
pub fn rename_unit(state: &AppState, unit_id: &str, body: &[u8]) -> Reply {
    let request: RenameRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return Reply::detail(422, format!("invalid rename request: {e}")),
    };
    let name = request.name.trim().to_string();
    let chars = name.chars().count();
    if chars == 0 {
        return Reply::detail(422, "name must not be empty");
    }
    if chars > NAME_MAX_CHARS {
        return Reply::detail(
            422,
            format!("name must be at most {NAME_MAX_CHARS} characters, got {chars}"),
        );
    }

    let mut config = match state.config.write() {
        Ok(c) => c,
        Err(_) => return Reply::detail(500, "config unavailable"),
    };
    let Some(unit) = config.find_unit_mut(unit_id) else {
        return Reply::detail(404, format!("Unknown unit '{unit_id}'"));
    };
    let previous = std::mem::replace(&mut unit.name, name);
    let view = unit_view(config.find_unit(unit_id).expect("just renamed"));

    if let Err(e) = persist(state, &config) {
        // Put the old name back, or the API reports a rename that a restart undoes.
        if let Some(unit) = config.find_unit_mut(unit_id) {
            unit.name = previous;
        }
        return Reply::detail(500, e);
    }
    // The device layer caches the name for its state responses. Renaming in
    // place rather than replacing the unit keeps its live session.
    if let Ok(id) = unit_id.parse::<u64>() {
        state
            .manager
            .rename(id, view["name"].as_str().unwrap_or_default());
    }
    Reply::json(200, &view)
}

/// The credentials to store for a unit being added.
///
/// Either what the caller supplied, or — as a last resort — one cloud round-trip
/// with an account they typed once. The account is used here and dropped here:
/// nothing stores it, nothing logs it, and `Credentials` refuses to print and
/// scrubs itself on the way out.
///
/// Takes `device_id` from *discovery* rather than the request, because the cloud
/// keys tokens by the id the unit reported, not the address the caller guessed.
#[cfg(feature = "cloud")]
fn resolve_credentials(
    request: &mut AddUnitRequest,
    device_id: u64,
) -> Result<(Option<String>, Option<String>), Reply> {
    if request.token.is_some() {
        return Ok((request.token.take(), request.key.take()));
    }
    let (Some(account), Some(password)) =
        (request.midea_account.take(), request.midea_password.take())
    else {
        return Ok((None, None));
    };

    let region = request
        .midea_region
        .take()
        .unwrap_or_else(|| "DE".to_string());
    let credentials = breeze_cloud::Credentials::new(account, password, region);
    match breeze_cloud::nethome::fetch_token(&credentials, device_id) {
        Ok(token) => Ok((Some(token.token.clone()), Some(token.key.clone()))),
        // The advice matters more than the error: "9999" means nothing to
        // anybody, and the way out is usually a token they already hold.
        Err(e) => Err(Reply::json(
            502,
            &serde_json::json!({
                "detail": e.to_string(),
                "advice": e.advice(),
            }),
        )),
    }
}

/// The same, for a build with no TLS in it.
///
/// Says so rather than quietly ignoring the credentials and storing a unit that
/// cannot be driven — which would look like success and behave like a fault.
#[cfg(not(feature = "cloud"))]
fn resolve_credentials(
    request: &mut AddUnitRequest,
    _device_id: u64,
) -> Result<(Option<String>, Option<String>), Reply> {
    if request.token.is_none()
        && request.midea_account.is_some()
        && request.midea_password.is_some()
    {
        return Err(Reply::detail(
            501,
            "this build has no cloud support; add the unit with a token and key instead",
        ));
    }
    Ok((request.token.take(), request.key.take()))
}
/// `POST /api/units` — discover the unit at an address and store it.
///
/// Discovery rather than trust: the client supplies an address, and the unit
/// itself supplies its id, port and type. A client cannot invent a unit that is
/// not there, which is what keeps a typo from becoming a permanently broken
/// entry in someone's config.
pub fn add_unit(state: &AppState, body: &[u8]) -> Reply {
    // `resolve_credentials` takes the credential fields out of the request.
    #[allow(unused_mut)]
    let mut request: AddUnitRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return Reply::detail(422, format!("invalid add request: {e}")),
    };
    let Ok(ip) = request.ip.trim().parse::<Ipv4Addr>() else {
        return Reply::detail(422, format!("'{}' is not an IPv4 address", request.ip));
    };
    if let Some(name) = &request.name {
        if name.chars().count() > NAME_MAX_CHARS {
            return Reply::detail(
                422,
                format!("name must be at most {NAME_MAX_CHARS} characters"),
            );
        }
    }

    // Validated before the LAN is touched, and before anything is written: a
    // malformed credential stored now is a unit that fails to authenticate
    // later with nothing to explain why.
    if let Some(token) = &request.token {
        if !valid_hex(token, TOKEN_HEX_LEN) {
            return Reply::detail(422, format!("token must be {TOKEN_HEX_LEN} hex characters"));
        }
    }
    if let Some(key) = &request.key {
        if !valid_hex(key, KEY_HEX_LEN) {
            return Reply::detail(422, format!("key must be {KEY_HEX_LEN} hex characters"));
        }
    }
    // Both or neither: half a credential authenticates nothing, and storing one
    // would look like progress.
    if request.token.is_some() != request.key.is_some() {
        return Reply::detail(422, "token and key must be given together");
    }

    // A single-address sweep: the same probe a full scan sends, aimed at one
    // host. No broadcast, so the answer can only have come from the address
    // that was asked about.
    let report = match breeze_device::scan_subnet(&format!("{ip}/32"), SCAN_LISTEN) {
        Ok(r) => r,
        Err(e) => return Reply::detail(503, format!("discovery failed: {e}")),
    };
    let Some(found) = report.found.into_iter().next() else {
        return Reply::detail(404, format!("no Midea unit answered at {ip}"));
    };

    let id = match i64::try_from(found.id) {
        Ok(id) => id,
        Err(_) => return Reply::detail(503, "that unit reported an unusable id"),
    };

    // Credentials, in order of preference: what the caller supplied, or -- as a
    // last resort -- one cloud round-trip with an account they typed once.
    //
    // The account is used here and dropped here. It is not stored, not logged,
    // and not echoed: `Credentials` scrubs itself and refuses to print. The
    // fetch happens *after* discovery because it needs the id the unit reported,
    // not the address the caller guessed.
    let supplied = match resolve_credentials(&mut request, found.id) {
        Ok(pair) => pair,
        Err(reply) => return reply,
    };

    let mut config = match state.config.write() {
        Ok(c) => c,
        Err(_) => return Reply::detail(500, "config unavailable"),
    };
    // An explicit name wins; otherwise keep the one already stored, and only
    // fall back to a generated one for a genuinely new unit.
    let name = request
        .name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .or_else(|| config.find_unit(&id.to_string()).map(|u| u.name.clone()))
        .unwrap_or_else(|| format!("AC {}", found.ip));

    let previous = config.units.clone();
    config.add_or_update_unit(UnitConfig {
        name,
        ip: found.ip.to_string(),
        port: found.port,
        id,
        // Supplied, fetched, or neither. Discovery itself produces no
        // credentials, and `add_or_update_unit` keeps any the unit already had
        // rather than clearing them.
        token: supplied.0,
        key: supplied.1,
    });
    if let Err(e) = persist(state, &config) {
        config.units = previous;
        return Reply::detail(500, e);
    }

    let stored = config.find_unit(&id.to_string()).expect("just added");
    let view = unit_view(stored);
    // Make it drivable immediately. `upsert` replaces the device, which also
    // drops any stale session for a unit that has moved address -- exactly what
    // is wanted here, since its address is the thing that just changed.
    if let Some(unit) = crate::state::to_device_unit(stored) {
        state.manager.upsert(unit);
    }
    Reply::json(201, &view)
}

/// `DELETE /api/units/{id}` — 204.
pub fn delete_unit(state: &AppState, unit_id: &str) -> Reply {
    let mut config = match state.config.write() {
        Ok(c) => c,
        Err(_) => return Reply::detail(500, "config unavailable"),
    };
    if config.find_unit(unit_id).is_none() {
        return Reply::detail(404, format!("Unknown unit '{unit_id}'"));
    }
    let previous = config.units.clone();
    config.remove_unit(unit_id);
    if let Err(e) = persist(state, &config) {
        config.units = previous;
        return Reply::detail(500, e);
    }
    if let Ok(id) = unit_id.parse::<u64>() {
        state.manager.remove(id);
    }
    Reply {
        status: 204,
        body: Vec::new(),
        content_type: "application/json",
        extra: Vec::new(),
    }
}

/// `GET /api/units/scan` — what is out there.
///
/// Read-only: it reports candidates, and adding one still goes through
/// `POST /api/units`. Anything already configured is flagged `known` so a client
/// can grey it out rather than offering a duplicate.
pub fn scan(state: &AppState, query: &str) -> Reply {
    let subnet = query_value(query, "subnet");

    // One at a time. A scan is hundreds of packets and several seconds; two at
    // once would double the traffic to answer the same question, and the
    // reference returns 409 for exactly this.
    let _guard = match state.scanning.try_lock() {
        Ok(g) => g,
        Err(_) => return Reply::detail(409, "a scan is already in progress"),
    };

    let report = match subnet.as_deref() {
        Some(cidr) if !cidr.is_empty() => match breeze_device::scan_subnet(cidr, SCAN_LISTEN) {
            Ok(r) => r,
            Err(breeze_device::ScanError::BadSubnet(why)) => return Reply::detail(400, why),
            Err(e) => return Reply::detail(503, format!("scan failed: {e}")),
        },
        _ => match breeze_device::scan(SCAN_LISTEN) {
            Ok(r) => r,
            Err(e) => return Reply::detail(503, format!("scan failed: {e}")),
        },
    };

    let known: Vec<String> = match state.config.read() {
        Ok(c) => c.units.iter().map(|u| u.ip.clone()).collect(),
        Err(_) => Vec::new(),
    };

    let candidates: Vec<serde_json::Value> = report
        .found
        .iter()
        .map(|unit| {
            serde_json::json!({
                "ip": unit.ip.to_string(),
                "port": unit.port,
                "known": known.iter().any(|ip| ip == &unit.ip.to_string()),
                // Beyond the reference's shape, and worth it: a client can show
                // "Kuhinja (net_ac_F13A)" instead of a bare address, and the id
                // is what makes a candidate identifiable at all.
                "id": unit.id.to_string(),
                "ssid": unit.ssid,
                "version": format!("{:?}", unit.version),
            })
        })
        .collect();

    Reply::json(
        200,
        &serde_json::json!({
            "candidates": candidates,
            // How the answer was arrived at, so "found nothing" is diagnosable
            // rather than mysterious -- which is exactly the bug this endpoint
            // was written to make visible.
            "scanned": report.swept,
            "probes_sent": report.probes_sent,
            "broadcast_sent": report.broadcast_sent,
            "unparseable": report
                .unparseable
                .iter()
                .map(|(ip, why)| serde_json::json!({"ip": ip.to_string(), "detail": why}))
                .collect::<Vec<_>>(),
        }),
    )
}

/// Read one value out of a raw query string.
///
/// Hand-rolled because this is the only route that takes a query parameter, and
/// a URL-decoding dependency for one CIDR would be a poor trade. Handles `+`
/// and `%XX`, which is everything a subnet can contain.
fn query_value(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        if k == key {
            return Some(percent_decode(&v.replace('+', " ")));
        }
    }
    None
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn persist(state: &AppState, config: &breeze_store::AppConfig) -> Result<(), String> {
    // 640, not 600: the admin CLIs read this file, which is why it is the one
    // store that is group-readable.
    breeze_store::save(
        &state.settings.config_path,
        config,
        breeze_store::Mode::AdminReadable,
    )
    .map_err(|e| format!("cannot write the config: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(id: i64, name: &str, paired: bool) -> UnitConfig {
        UnitConfig {
            name: name.into(),
            ip: "192.168.1.73".into(),
            port: 6444,
            id,
            token: paired.then(|| "aa".repeat(64)),
            key: paired.then(|| "bb".repeat(32)),
        }
    }

    #[test]
    fn a_unit_view_never_carries_a_credential() {
        // The whole point of this module having its own serialiser.
        let view = unit_view(&unit(153931628470980, "Lijeva Soba", true));
        let text = serde_json::to_string(&view).unwrap();
        assert!(!text.contains("aa"), "a token leaked: {text}");
        assert!(!text.contains("bb"), "a key leaked: {text}");
        assert!(!text.contains("token"));
        assert!(!text.contains("\"key\""));
        assert_eq!(view["has_v3_credentials"], true);
        // And the id is a string, as everywhere else on the wire.
        assert_eq!(view["id"], "153931628470980");
    }

    #[test]
    fn an_unpaired_unit_says_so() {
        let view = unit_view(&unit(1, "No Creds", false));
        assert_eq!(view["has_v3_credentials"], false);
    }

    #[test]
    fn half_a_credential_does_not_count_as_paired() {
        let mut half = unit(1, "Half", true);
        half.key = None;
        assert_eq!(unit_view(&half)["has_v3_credentials"], false);
    }

    #[test]
    fn adding_a_unit_keeps_credentials_it_already_had() {
        // Discovery yields no token, so re-adding a paired unit by address must
        // not unpair it -- which would leave a configured unit that cannot be
        // driven and no obvious reason why.
        let mut config = breeze_store::AppConfig {
            api_key: Some("k".into()),
            units: vec![unit(7, "Kuhinja", true)],
        };
        let inserted = config.add_or_update_unit(UnitConfig {
            name: "Kuhinja".into(),
            ip: "192.168.1.99".into(),
            port: 6444,
            id: 7,
            token: None,
            key: None,
        });
        assert!(!inserted, "same id is an update, not an insert");
        let stored = config.find_unit("7").unwrap();
        assert_eq!(stored.ip, "192.168.1.99", "the address is refreshed");
        assert!(stored.token.is_some(), "the token survived");
        assert!(stored.key.is_some(), "the key survived");
    }

    #[test]
    fn a_new_id_is_an_insert() {
        let mut config = breeze_store::AppConfig::default();
        assert!(config.add_or_update_unit(unit(1, "One", false)));
        assert!(config.add_or_update_unit(unit(2, "Two", false)));
        assert_eq!(config.units.len(), 2);
    }

    #[test]
    fn units_are_found_and_removed_by_their_string_id() {
        let mut config = breeze_store::AppConfig {
            api_key: None,
            units: vec![unit(153931628470980, "Lijeva Soba", true)],
        };
        assert!(config.find_unit("153931628470980").is_some());
        assert!(config.find_unit("999").is_none());
        assert!(!config.remove_unit("999"));
        assert!(config.remove_unit("153931628470980"));
        assert!(config.units.is_empty());
    }

    #[test]
    fn a_subnet_is_read_out_of_the_query_string() {
        assert_eq!(
            query_value("subnet=192.168.1.0%2F24", "subnet").as_deref(),
            Some("192.168.1.0/24"),
            "an encoded slash must survive"
        );
        assert_eq!(
            query_value("timeout=1&subnet=10.0.0.0/24", "subnet").as_deref(),
            Some("10.0.0.0/24")
        );
        assert_eq!(query_value("subnet=", "subnet").as_deref(), Some(""));
        assert_eq!(query_value("other=1", "subnet"), None);
        assert_eq!(query_value("", "subnet"), None);
    }

    #[test]
    fn percent_decoding_leaves_malformed_escapes_alone() {
        // Better a literal '%' than a panic or a silently mangled subnet.
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("a%2Fb"), "a/b");
        assert_eq!(percent_decode("plain"), "plain");
    }

    #[test]
    fn credentials_must_be_hex_of_the_right_length() {
        // A V3 token is 64 bytes and a key is 32, both hex. Storing a malformed
        // one produces a unit that cannot authenticate, with nothing to say why.
        assert!(valid_hex(&"a".repeat(TOKEN_HEX_LEN), TOKEN_HEX_LEN));
        assert!(valid_hex(&"AB".repeat(KEY_HEX_LEN / 2), KEY_HEX_LEN));
        assert!(!valid_hex(&"a".repeat(TOKEN_HEX_LEN - 1), TOKEN_HEX_LEN));
        assert!(!valid_hex(&"a".repeat(TOKEN_HEX_LEN + 1), TOKEN_HEX_LEN));
        assert!(!valid_hex(&"z".repeat(TOKEN_HEX_LEN), TOKEN_HEX_LEN));
        assert!(!valid_hex("", TOKEN_HEX_LEN));
        // A real pair, for shape.
        assert!(valid_hex(&"aa".repeat(64), TOKEN_HEX_LEN));
        assert!(valid_hex(&"bb".repeat(32), KEY_HEX_LEN));
    }

    #[test]
    fn an_add_request_may_carry_credentials_or_not() {
        let bare: AddUnitRequest = serde_json::from_str(r#"{"ip":"192.168.1.73"}"#).unwrap();
        assert!(bare.token.is_none() && bare.key.is_none());

        let full: AddUnitRequest = serde_json::from_str(
            r#"{"ip":"192.168.1.73","name":"Kuhinja","token":"aa","key":"bb"}"#,
        )
        .unwrap();
        assert_eq!(full.name.as_deref(), Some("Kuhinja"));
        assert_eq!(full.token.as_deref(), Some("aa"));
        assert_eq!(full.key.as_deref(), Some("bb"));
    }

    #[test]
    fn a_supplied_credential_is_stored_and_never_echoed() {
        // The credential goes in; what comes back out says only that it exists.
        let mut config = breeze_store::AppConfig::default();
        config.add_or_update_unit(UnitConfig {
            name: "Kuhinja".into(),
            ip: "192.168.1.74".into(),
            port: 6444,
            id: 7,
            token: Some("aa".repeat(64)),
            key: Some("bb".repeat(32)),
        });
        let stored = config.find_unit("7").unwrap();
        assert!(stored.token.is_some(), "it was stored");

        let rendered = serde_json::to_string(&unit_view(stored)).unwrap();
        assert!(!rendered.contains("aa"), "the token came back: {rendered}");
        assert_eq!(unit_view(stored)["has_v3_credentials"], true);
    }
}
