//! A mock of Midea's v5 cloud, and the SmartHome and Meiju flows run against it.
//!
//! It refuses what the real API refuses -- a wrong signature, SmartHome's
//! missing headers, a wrong password hash, no access token, a `getToken`
//! without `applianceCodes` (`3004`) or for a unit the account does not own
//! (`3201`) -- and answers the way it answers, codes as strings included. The
//! unit's firmware is big-endian here, so a token is only found on the second
//! byte order, and the token list carries a decoy for another unit. Nothing
//! here proves the live API still behaves like this; it proves the client
//! sends what the live API was seen to want.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::v5::{self, Profile, MEIJU, SMARTHOME};
use crate::{udpid, CloudError, Credentials};

const ACCOUNT: &str = "owner@example.com";
const PASSWORD: &str = "correct horse";
const LOGIN_ID: &str = "0123456789abcdef";
const ACCESS: &str = "t0k";
const HOME: &str = "77";
/// A unit on the account.
const DEVICE: u64 = 151_732_605_161_920;
/// A unit somebody else owns.
const STRANGER: u64 = 151_732_605_000_001;

fn token() -> String {
    "AB".repeat(64)
}
fn key() -> String {
    "CD".repeat(32)
}

/// Serve `profile`'s API on a free port. Returns the base URL and the
/// endpoints called, in order.
fn start(profile: &'static Profile) -> (String, Arc<Mutex<Vec<String>>>) {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("a free port");
    let port = server.server_addr().to_ip().expect("an IP listener").port();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&calls);
    std::thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let endpoint = request
                .url()
                .split("alias=")
                .nth(1)
                .unwrap_or("")
                .to_string();
            let headers: HashMap<String, String> = request
                .headers()
                .iter()
                .map(|h| {
                    (
                        h.field.as_str().as_str().to_ascii_lowercase(),
                        h.value.to_string(),
                    )
                })
                .collect();
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            seen.lock().unwrap().push(endpoint.clone());
            let reply = answer(profile, &endpoint, &headers, &body).to_string();
            let _ = request.respond(tiny_http::Response::from_string(reply));
        }
    });
    (
        format!("http://127.0.0.1:{port}/mas/v5/app/proxy?alias="),
        calls,
    )
}

/// A refusal, its code a string as the live API sends it.
fn refuse(code: i64, message: &str) -> Value {
    json!({ "code": code.to_string(), "msg": message })
}

fn ok(data: Value) -> Value {
    json!({ "code": 0, "msg": "ok", "data": data })
}

fn answer(
    profile: &Profile,
    endpoint: &str,
    headers: &HashMap<String, String>,
    body: &str,
) -> Value {
    let random = headers.get("random").cloned().unwrap_or_default();
    if headers.get("sign") != Some(&v5::sign(profile, body, &random)) {
        return refuse(1001, "sign error");
    }
    if profile.smarthome_headers
        && (headers.get("x-recipe-app").map(String::as_str) != Some(profile.app_id)
            || headers.get("authorization") != Some(&format!("Basic {}", v5::basic(profile))))
    {
        return refuse(1002, "missing app headers");
    }
    let data: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let signed_in = headers.get("accesstoken").map(String::as_str) == Some(ACCESS);

    match endpoint {
        "/v1/multicloud/platform/user/route" if profile.smarthome_headers => {
            if data["userName"] == ACCOUNT {
                ok(json!({ "masUrl": "https://unused.invalid/" }))
            } else {
                refuse(10004, "user route does not exist")
            }
        }
        "/v1/user/login/id/get" => {
            if data["loginAccount"] == ACCOUNT {
                ok(json!({ "loginId": LOGIN_ID }))
            } else {
                refuse(3102, "this account does not exist")
            }
        }
        "/mj/user/login" => {
            let iot = &data["iotData"];
            let iampwd = if profile.smarthome_headers {
                v5::smarthome_iampwd(LOGIN_ID, PASSWORD)
            } else {
                v5::md5_twice(PASSWORD)
            };
            if iot["password"] == v5::password(profile, LOGIN_ID, PASSWORD)
                && iot["iampwd"] == iampwd
            {
                ok(json!({ "uid": "u1", "mdata": { "accessToken": ACCESS } }))
            } else {
                refuse(3101, "Account or password incorrect, please re-enter")
            }
        }
        _ if !signed_in => refuse(40002, "the access token is invalid"),
        "/v1/appliance/user/list/get" => ok(json!({ "list": [
            { "id": DEVICE.to_string(), "name": "Bedroom", "type": "0xAC", "onlineStatus": "1" },
        ]})),
        "/v1/homegroup/list/get" => {
            ok(json!({ "homeList": [{ "homegroupId": 77, "name": "Home" }] }))
        }
        "/v1/appliance/home/list/get" => {
            ok(json!({ "homeList": [{ "roomList": [{ "applianceList": [
                { "applianceCode": DEVICE, "name": "Bedroom", "type": "0xAC", "onlineStatus": "0" },
            ]}]}]}))
        }
        "/v1/iot/secure/getToken" | "/v2/iot/secure/getToken" => {
            token_answer(profile, endpoint, &data)
        }
        _ => refuse(40404, "the access address does not exist"),
    }
}

fn token_answer(profile: &Profile, endpoint: &str, data: &Value) -> Value {
    let v2 = endpoint.starts_with("/v2");
    // Meiju's v1 is gone, as it is in production.
    if !profile.smarthome_headers && !v2 {
        return refuse(40404, "the access address does not exist");
    }
    let codes = &data["applianceCodes"];
    let asked_for = if v2 {
        codes
            .as_array()
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .map(str::to_string)
    } else {
        codes.as_str().map(str::to_string)
    };
    let Some(asked_for) = asked_for else {
        return refuse(3004, "value is illegal");
    };
    if v2 && data["homegroupId"] != HOME {
        return refuse(1002, "none parameter is found");
    }
    if asked_for != DEVICE.to_string() {
        return refuse(3201, "You have no permissions");
    }
    let decoy =
        json!({ "udpId": udpid(STRANGER, true), "token": "EE".repeat(64), "key": "FF".repeat(32) });
    let mine = udpid(DEVICE, true);
    let list = if data["udpid"] == mine {
        json!([decoy, { "udpId": mine.to_uppercase(), "token": token(), "key": key() }])
    } else {
        json!([decoy])
    };
    ok(json!({ "tokenlist": list }))
}

fn owner() -> Credentials {
    Credentials::new(ACCOUNT, PASSWORD, "")
}

fn count(calls: &Arc<Mutex<Vec<String>>>, endpoint: &str) -> usize {
    calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.as_str() == endpoint)
        .count()
}

#[test]
fn smarthome_knows_its_own_accounts_and_says_so_without_a_password() {
    let (base, calls) = start(&SMARTHOME);
    assert!(crate::smarthome::account_known_at(ACCOUNT, Some(&base)).unwrap());
    assert!(!crate::smarthome::account_known_at("nethome-user@example.com", Some(&base)).unwrap());
    assert_eq!(
        count(&calls, "/mj/user/login"),
        0,
        "no password was sent to find out"
    );
}

#[test]
fn smarthome_logs_in_lists_the_unit_and_fetches_its_token() {
    let (base, calls) = start(&SMARTHOME);
    let session = crate::smarthome::login_at(&owner(), Some(&base)).unwrap();
    let units = session.appliances().unwrap();
    assert_eq!(units.len(), 1);
    assert_eq!(
        (units[0].id, units[0].kind, units[0].online),
        (DEVICE, Some(0xAC), true)
    );

    let got = session.token(DEVICE).unwrap();
    assert_eq!(
        got.token,
        token().to_lowercase(),
        "this unit's, not the decoy's, lowercased"
    );
    assert_eq!(got.key, key().to_lowercase());
    assert_eq!(
        count(&calls, "/v1/iot/secure/getToken"),
        2,
        "little-endian found nothing, big-endian found it"
    );
}

#[test]
fn smarthome_refuses_a_wrong_password_and_a_unit_the_account_does_not_own() {
    let (base, _) = start(&SMARTHOME);
    let wrong = Credentials::new(ACCOUNT, "wrong", "");
    assert!(matches!(
        crate::smarthome::login_at(&wrong, Some(&base)),
        Err(CloudError::Api { code: 3101, .. })
    ));
    let session = crate::smarthome::login_at(&owner(), Some(&base)).unwrap();
    assert!(matches!(
        session.token(STRANGER),
        Err(CloudError::Api { code: 3201, .. })
    ));
}

#[test]
fn meiju_fetches_through_v2_with_the_home() {
    let (base, calls) = start(&MEIJU);
    assert!(!crate::meiju::account_known_at("someone-else@example.com", Some(&base)).unwrap());
    let session = crate::meiju::login_at(&owner(), Some(&base)).unwrap();
    let units = session.appliances().unwrap();
    assert_eq!(
        (units.len(), units[0].id, units[0].online),
        (1, DEVICE, false)
    );
    let got = session.token(DEVICE).unwrap();
    assert_eq!(got.token, token().to_lowercase());
    assert!(count(&calls, "/v2/iot/secure/getToken") >= 1);
    assert_eq!(
        count(&calls, "/v1/iot/secure/getToken"),
        0,
        "v1 only when v2 finds nothing"
    );
}
