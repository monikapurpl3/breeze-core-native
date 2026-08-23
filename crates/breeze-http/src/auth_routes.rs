//! `/api/auth/*` — pairing, and managing what is paired.
//!
//! The API key alone gets a client as far as *asking* to pair. Approval is a
//! separate action that must come from the LAN, so a leaked key is not enough to
//! gain control of anyone's air conditioning: someone has to be standing on the
//! trusted network to say yes.

use breeze_auth::enroll::{APPROVED, PENDING};
use breeze_store::DeviceRecord;
use serde::Deserialize;

use crate::respond::Reply;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct StartRequest {
    #[serde(default)]
    pub label: String,
    #[serde(default = "one")]
    pub auth_version: u8,
    #[serde(default)]
    pub public_key: Option<String>,
}

fn one() -> u8 {
    1
}

#[derive(Debug, Deserialize)]
pub struct PollRequest {
    #[serde(default)]
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ApproveRequest {
    #[serde(default)]
    pub code: String,
}

#[derive(Debug, Deserialize)]
pub struct UpgradeRequest {
    /// The Ed25519 public key the already-enrolled device has just generated.
    pub public_key: String,
}

/// The reference bounds this, and so does this: a public key is 43 characters of
/// base64url, so anything near the limit is already wrong.
const PUBLIC_KEY_MAX: usize = 128;

/// `POST /api/auth/upgrade` — move the calling device from v1 to v2 in place.
///
/// Authorised by the device's *existing* credential and nothing else: no admin
/// approval, and no LAN gate. That is not a gap. It trusts nobody new — it
/// re-keys a device that has just proved it holds a working credential, and the
/// old bearer token stops working the moment this returns. Requiring somebody to
/// walk to the LAN to let a phone improve its own crypto would mean most phones
/// never did.
pub fn upgrade(state: &AppState, token_id: Option<&str>, body: &[u8]) -> Reply {
    // Identified by `authorise`, not re-verified here: a second verification
    // would spend the v2 nonce twice. (A v1 device has no nonce, but this route
    // is also reachable by an already-upgraded device re-keying again.)
    let Some(token_id) = token_id else {
        return Reply::detail(401, "no authenticated device to upgrade");
    };
    let request: UpgradeRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return Reply::detail(422, format!("invalid upgrade request: {e}")),
    };
    if request.public_key.len() > PUBLIC_KEY_MAX {
        return Reply::detail(400, "invalid Ed25519 public_key");
    }
    // Checked now, once, rather than on every later request: a malformed key
    // stored here would lock the device out of everything with no way back.
    if !breeze_auth::signing::public_key_is_valid(&request.public_key) {
        return Reply::detail(400, "invalid Ed25519 public_key");
    }

    let mut devices = match state.devices.write() {
        Ok(d) => d,
        Err(_) => return Reply::detail(500, "device store unavailable"),
    };
    let Some(record) = devices.devices.iter_mut().find(|d| d.token_id == token_id) else {
        return Reply::detail(404, "device not found");
    };

    let previous = record.clone();
    record.auth_version = 2;
    record.public_key = Some(request.public_key);
    // Dropped deliberately: leaving it would keep a second, weaker way in to a
    // device that has just been told to use signatures.
    record.token_hash = None;

    if let Err(e) = breeze_store::save(
        &state.settings.devices_path,
        &*devices,
        breeze_store::Mode::Private,
    ) {
        // Roll back: a device that believes it upgraded, against a server that
        // forgot, can authenticate neither way.
        if let Some(record) = devices.devices.iter_mut().find(|d| d.token_id == token_id) {
            *record = previous;
        }
        return Reply::detail(500, format!("cannot write the device store: {e}"));
    }
    Reply::json(
        200,
        &serde_json::json!({ "token_id": token_id, "auth_version": 2 }),
    )
}

/// `POST /api/auth/enroll/start`
pub fn start(state: &AppState, body: &[u8]) -> Reply {
    // An absent body is a v1 enrolment with no label, which is what an older
    // client sends.
    let request: StartRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(_) if body.is_empty() => StartRequest {
            label: String::new(),
            auth_version: 1,
            public_key: None,
        },
        Err(e) => return Reply::detail(422, format!("invalid enrolment request: {e}")),
    };

    let mut enrollment = match state.enrollment.lock() {
        Ok(e) => e,
        Err(_) => return Reply::detail(500, "enrolment unavailable"),
    };
    let now = breeze_auth::signing::now_seconds();
    match enrollment.start(
        &request.label,
        request.auth_version,
        request.public_key.as_deref(),
        now,
    ) {
        Ok((session_id, user_code, expires_in)) => Reply::json(
            200,
            &serde_json::json!({
                "session_id": session_id,
                "user_code": user_code,
                "expires_in": expires_in,
            }),
        ),
        // A bad public key is the client's mistake, not a server failure.
        Err(e) => Reply::detail(422, e.to_string()),
    }
}

/// `POST /api/auth/enroll/approve` — LAN only.
pub fn approve(state: &AppState, body: &[u8]) -> Reply {
    let request: ApproveRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return Reply::detail(422, format!("invalid approval request: {e}")),
    };
    let now = breeze_auth::signing::now_seconds();

    let record = {
        let mut enrollment = match state.enrollment.lock() {
            Ok(e) => e,
            Err(_) => return Reply::detail(500, "enrolment unavailable"),
        };
        enrollment.approve(&request.code, now)
    };
    let Some(record) = record else {
        // Wrong, expired or already used -- deliberately not distinguished, so a
        // guesser learns nothing from the response.
        return Reply::detail(404, "no pending enrolment matches that code");
    };

    if let Err(e) = persist_new_device(state, record.clone()) {
        return Reply::detail(500, e);
    }
    Reply::json(
        200,
        &serde_json::json!({ "token_id": record.token_id, "label": record.label }),
    )
}

/// `POST /api/auth/enroll/poll`
pub fn poll(state: &AppState, body: &[u8]) -> Reply {
    let request: PollRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return Reply::detail(422, format!("invalid poll request: {e}")),
    };
    let mut enrollment = match state.enrollment.lock() {
        Ok(e) => e,
        Err(_) => return Reply::detail(500, "enrolment unavailable"),
    };
    let now = breeze_auth::signing::now_seconds();
    let outcome = enrollment.poll(&request.session_id, now);

    if outcome.status == APPROVED {
        if let Some(record) = outcome.record {
            let mut body = serde_json::json!({
                "status": APPROVED,
                "token_id": record.token_id,
                "label": record.label,
                "auth_version": record.auth_version,
                "expires_at": record.expires_at,
            });
            // v1 only, and exactly once: v2 devices already hold their key.
            if let Some(token) = outcome.device_token {
                body["device_token"] = serde_json::Value::String(token);
            }
            return Reply::json(200, &body);
        }
    }
    if outcome.status == PENDING {
        return Reply::json(200, &serde_json::json!({ "status": PENDING }));
    }
    Reply::json(200, &serde_json::json!({ "status": outcome.status }))
}

/// `GET /api/auth/devices` — LAN only.
///
/// Identity and lifecycle only. Never a `token_hash`, never a `public_key`: the
/// first is a credential and the second, while not secret, is nobody else's
/// business.
pub fn list_devices(state: &AppState) -> Reply {
    let devices = match state.devices.read() {
        Ok(d) => d,
        Err(_) => return Reply::detail(500, "device store unavailable"),
    };
    let list: Vec<serde_json::Value> = devices
        .devices
        .iter()
        .map(|d| {
            serde_json::json!({
                "token_id": d.token_id,
                "label": d.label,
                "auth_version": d.auth_version,
                "created_at": d.created_at,
                "expires_at": d.expires_at,
                "last_used": d.last_used,
            })
        })
        .collect();
    Reply::json_body(200, &list)
}

/// `DELETE /api/auth/devices/{token_id}` — LAN only.
pub fn revoke(state: &AppState, token_id: &str) -> Reply {
    let removed = {
        let mut devices = match state.devices.write() {
            Ok(d) => d,
            Err(_) => return Reply::detail(500, "device store unavailable"),
        };
        let before = devices.devices.len();
        devices.devices.retain(|d| d.token_id != token_id);
        let removed = devices.devices.len() != before;
        if removed {
            if let Err(e) = breeze_store::save(
                &state.settings.devices_path,
                &*devices,
                breeze_store::Mode::Private,
            ) {
                return Reply::detail(500, format!("cannot write the device store: {e}"));
            }
        }
        removed
    };
    if !removed {
        return Reply::detail(404, "no such device");
    }
    // 204, as Breeze Core answers.
    Reply {
        status: 204,
        body: Vec::new(),
        content_type: "application/json",
        extra: Vec::new(),
    }
}

/// Add a freshly minted record and write it out.
///
/// Written immediately rather than at shutdown: a credential a client already
/// holds but the server has forgotten is the worst of both worlds.
fn persist_new_device(state: &AppState, record: DeviceRecord) -> Result<(), String> {
    let mut devices = state
        .devices
        .write()
        .map_err(|_| "device store unavailable".to_string())?;
    devices.devices.push(record);
    breeze_store::save(
        &state.settings.devices_path,
        &*devices,
        breeze_store::Mode::Private,
    )
    .map_err(|e| format!("cannot write the device store: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_request_defaults_to_v1_with_no_label() {
        // What an older client sends: `{}` or nothing at all.
        let r: StartRequest = serde_json::from_slice(b"{}").unwrap();
        assert_eq!(r.auth_version, 1);
        assert_eq!(r.label, "");
        assert!(r.public_key.is_none());
    }

    #[test]
    fn a_v2_start_request_carries_its_public_key() {
        let r: StartRequest =
            serde_json::from_slice(br#"{"label":"Panel","auth_version":2,"public_key":"abc"}"#)
                .unwrap();
        assert_eq!(r.auth_version, 2);
        assert_eq!(r.label, "Panel");
        assert_eq!(r.public_key.as_deref(), Some("abc"));
    }

    #[test]
    fn unknown_fields_in_a_request_are_tolerated() {
        // A newer client may send more than this build knows; refusing would
        // break pairing for no reason.
        let r: StartRequest =
            serde_json::from_slice(br#"{"label":"x","future_field":true}"#).unwrap();
        assert_eq!(r.label, "x");
    }

    #[test]
    fn an_upgrade_request_is_just_a_public_key() {
        let r: UpgradeRequest = serde_json::from_slice(br#"{"public_key":"abc"}"#).unwrap();
        assert_eq!(r.public_key, "abc");
        // And it is required: an upgrade with nothing to upgrade *to* would
        // leave a device with no credential at all.
        assert!(serde_json::from_slice::<UpgradeRequest>(b"{}").is_err());
    }
}
