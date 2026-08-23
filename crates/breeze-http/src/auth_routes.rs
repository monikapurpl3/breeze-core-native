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
}
