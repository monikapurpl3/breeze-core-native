//! The NetHome Plus cloud — login, then one token.
//!
//! Three form-encoded POSTs, each signed with a SHA-256 over the request path,
//! its sorted parameters and a fixed app key:
//!
//! 1. `/v1/user/login/id/get` — turns an account name into a `loginId`, which is
//!    the salt for the password hash. So the password cannot be hashed until the
//!    server has been asked about the account.
//! 2. `/v1/user/login` — exchanges the hashed password for a `sessionId`.
//! 3. `/v1/iot/secure/getToken` — the actual point of all this.
//!
//! The constants are the vendor's, taken from msmart. They are not secrets: they
//! identify the NetHome Plus app to the API, and every client that talks to it
//! uses the same ones.

use std::collections::BTreeMap;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::{hex, CloudError, Credentials, Token};

const BASE_URL: &str = "https://mapp.appsmb.com";
const APP_ID: &str = "1017";
const APP_KEY: &str = "3742e9e5842d4ad59c2db887e12449f9";

/// Long enough for a slow mobile API, short enough that somebody watching a
/// pairing screen does not conclude it has hung.
const TIMEOUT: Duration = Duration::from_secs(20);

/// Regions the API is served in. Only the account's own region will accept it.
pub const REGIONS: [&str; 3] = ["DE", "KR", "US"];

/// A logged-in session. Holds no credentials — only the session id the cloud
/// issued.
pub struct Session {
    session_id: String,
    /// Randomised per session, as the app does. Nothing depends on its value.
    device_id: String,
}

/// Log in, then fetch the credentials for one unit.
///
/// The whole flow in one call, because there is no reason to hold a session open:
/// this happens once, while a person waits, and then the credentials live in
/// `config.json` instead.
///
/// `device_id` is the unit's id as discovery reported it. Both byte orders of the
/// derived udpid are tried, because firmware is inconsistent about it and the
/// wrong one simply returns nothing.
pub fn fetch_token(credentials: &Credentials, device_id: u64) -> Result<Token, CloudError> {
    let session = login(credentials)?;
    let mut last = None;
    for big_endian in [false, true] {
        match get_token(&session, device_id, big_endian) {
            Ok(token) => return Ok(token),
            // A refusal is about the account or the API, not the byte order, so
            // there is nothing to gain from trying the other one.
            Err(e @ CloudError::Api { .. }) => return Err(e),
            Err(e) => last = Some(e),
        }
    }
    Err(last
        .unwrap_or_else(|| CloudError::NoToken("the cloud returned no token for this unit".into())))
}

/// Steps 1 and 2.
pub fn login(credentials: &Credentials) -> Result<Session, CloudError> {
    let device_id = random_hex(8);
    let mut session = Session {
        session_id: String::new(),
        device_id,
    };

    // The loginId is the salt, so it has to come first.
    let response = request(
        &session,
        "/v1/user/login/id/get",
        [("loginAccount", credentials.account.as_str())],
    )?;
    let login_id = response
        .get("loginId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CloudError::Malformed("no loginId in the response".into()))?
        .to_string();

    let hashed = encrypt_password(&login_id, credentials.password());
    let response = request(
        &session,
        "/v1/user/login",
        [
            ("loginAccount", credentials.account.as_str()),
            ("password", hashed.as_str()),
        ],
    )?;
    session.session_id = response
        .get("sessionId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CloudError::Malformed("no sessionId in the response".into()))?
        .to_string();
    Ok(session)
}

/// Step 3.
fn get_token(session: &Session, device_id: u64, big_endian: bool) -> Result<Token, CloudError> {
    let wanted = crate::udpid(device_id, big_endian);
    let response = request(
        session,
        "/v1/iot/secure/getToken",
        [("udpid", wanted.as_str())],
    )?;

    let list = response
        .get("tokenlist")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CloudError::Malformed("no tokenlist in the response".into()))?;

    for entry in list {
        // Only the entry for the udpid that was asked about: the API returns a
        // list, and taking the first would be the wrong unit's credentials.
        if entry.get("udpId").and_then(|v| v.as_str()) != Some(wanted.as_str()) {
            continue;
        }
        let token = entry.get("token").and_then(|v| v.as_str());
        let key = entry.get("key").and_then(|v| v.as_str());
        if let (Some(token), Some(key)) = (token, key) {
            return Ok(Token {
                token: token.to_string(),
                key: key.to_string(),
            });
        }
    }
    Err(CloudError::NoToken(format!(
        "the cloud listed {} token(s), none of them for this unit",
        list.len()
    )))
}

/// One signed, form-encoded POST, unwrapping the response envelope.
fn request<'a, const N: usize>(
    session: &Session,
    endpoint: &str,
    extra: [(&'a str, &'a str); N],
) -> Result<serde_json::Value, CloudError> {
    // Sorted, because the signature is over the sorted parameters.
    let mut body: BTreeMap<&str, String> = BTreeMap::new();
    body.insert("appId", APP_ID.to_string());
    body.insert("src", APP_ID.to_string());
    body.insert("format", "2".to_string());
    body.insert("clientType", "1".to_string());
    body.insert("language", "en_US".to_string());
    body.insert("deviceId", session.device_id.clone());
    body.insert("stamp", timestamp());
    body.insert("sessionId", session.session_id.clone());
    for (key, value) in extra {
        body.insert(key, value.to_string());
    }

    let signature = sign(endpoint, &body);
    let fields: Vec<(&str, &str)> = body
        .iter()
        .map(|(k, v)| (*k, v.as_str()))
        .chain(std::iter::once(("sign", signature.as_str())))
        .collect();

    let url = format!("{BASE_URL}{endpoint}");
    let response = ureq::post(&url).timeout(TIMEOUT).send_form(&fields);

    let text = match response {
        Ok(r) => r
            .into_string()
            .map_err(|e| CloudError::Transport(e.to_string()))?,
        // A non-2xx still carries a body, and the body is where the code is.
        Err(ureq::Error::Status(_, r)) => r
            .into_string()
            .map_err(|e| CloudError::Transport(e.to_string()))?,
        Err(e) => return Err(CloudError::Transport(e.to_string())),
    };

    parse_envelope(&text)
}

/// `{"errorCode": "0", "result": {...}, "msg": "..."}`.
fn parse_envelope(text: &str) -> Result<serde_json::Value, CloudError> {
    let body: serde_json::Value =
        serde_json::from_str(text).map_err(|e| CloudError::Malformed(e.to_string()))?;

    // The code arrives as a string in practice, and as a number in some
    // responses. Accept either rather than failing on the shape.
    let code = body
        .get("errorCode")
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .or_else(|| v.as_i64())
        })
        .ok_or_else(|| CloudError::Malformed("no errorCode in the response".into()))?;

    if code != 0 {
        let message = body
            .get("msg")
            .and_then(|v| v.as_str())
            .unwrap_or("no message")
            .to_string();
        return Err(CloudError::Api { code, message });
    }
    body.get("result")
        .cloned()
        .ok_or_else(|| CloudError::Malformed("a success with no result".into()))
}

/// `sha256(path + sorted "k=v&k=v" + app key)`.
///
/// The values go in raw. msmart url-encodes and then decodes them, which is a
/// no-op with extra steps, and getting that wrong would produce a signature the
/// API rejects with no hint as to why.
fn sign(path: &str, body: &BTreeMap<&str, String>) -> String {
    let query: Vec<String> = body.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let message = format!("{path}{}{APP_KEY}", query.join("&"));
    hex(&Sha256::digest(message.as_bytes()))
}

/// `sha256(login_id + sha256(password) + app key)`.
fn encrypt_password(login_id: &str, password: &str) -> String {
    let first = hex(&Sha256::digest(password.as_bytes()));
    hex(&Sha256::digest(
        format!("{login_id}{first}{APP_KEY}").as_bytes(),
    ))
}

/// UTC, `yyyymmddHHMMSS`, as the API wants it.
fn timestamp() -> String {
    // Hand-formatted from the epoch rather than pulling in a date library for
    // one string. Days-from-civil, in the usual formulation.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rest) = ((now / 86_400) as i64, now % 86_400);
    let (hour, minute, second) = (rest / 3600, (rest % 3600) / 60, rest % 60);

    // Epoch day 0 is 1970-01-01; shift to a March-based year to make the leap
    // day the last day of the cycle.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };

    format!("{year:04}{month:02}{day:02}{hour:02}{minute:02}{second:02}")
}

fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    if getrandom::getrandom(&mut buffer).is_err() {
        // Only ever an opaque identifier; a fixed one is worse than a random one
        // but not a security property.
        buffer.fill(0x42);
    }
    hex(&buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signature_matches_the_reference_construction() {
        // Computed independently with Python:
        //   sha256(("/v1/user/login/id/get" + "a=1&b=2" + APP_KEY).encode()).hexdigest()
        let mut body = BTreeMap::new();
        body.insert("b", "2".to_string());
        body.insert("a", "1".to_string());
        let signature = sign("/v1/user/login/id/get", &body);
        assert_eq!(
            signature, "4db1f51885d6d7605c3f2ee7ccc218fb07ffff26408a97c2350195af0ce759f9",
            "the signature is what the API checks; a wrong one is a silent refusal"
        );
    }

    #[test]
    fn the_signature_sorts_its_parameters() {
        // Insertion order must not matter: a BTreeMap already sorts, and this
        // pins that the signature depends on the sorted form.
        let mut one = BTreeMap::new();
        one.insert("a", "1".to_string());
        one.insert("b", "2".to_string());
        let mut other = BTreeMap::new();
        other.insert("b", "2".to_string());
        other.insert("a", "1".to_string());
        assert_eq!(sign("/x", &one), sign("/x", &other));
    }

    #[test]
    fn the_signature_covers_the_path_and_the_values() {
        let mut body = BTreeMap::new();
        body.insert("a", "1".to_string());
        assert_ne!(sign("/one", &body), sign("/two", &body));

        let mut changed = BTreeMap::new();
        changed.insert("a", "2".to_string());
        assert_ne!(sign("/one", &body), sign("/one", &changed));
    }

    #[test]
    fn the_password_hash_is_salted_by_the_login_id() {
        // Which is why the account has to be looked up before the password can
        // be hashed at all.
        let a = encrypt_password("login-one", "hunter2");
        let b = encrypt_password("login-two", "hunter2");
        assert_ne!(a, b, "a different loginId must give a different hash");
        assert_eq!(a.len(), 64);
        // And the password itself never appears in it.
        assert!(!a.contains("hunter2"));
    }

    #[test]
    fn the_password_hash_matches_the_reference_construction() {
        // Python:
        //   m1 = sha256(b"hunter2").hexdigest()
        //   sha256(("abc" + m1 + APP_KEY).encode()).hexdigest()
        assert_eq!(
            encrypt_password("abc", "hunter2"),
            "6ecb00b326ddd9fd20dfccf6311c0f8aa007dfd05c2d4fb67cbc2fc6b0d72793"
        );
    }

    #[test]
    fn the_timestamp_is_fourteen_digits() {
        let stamp = timestamp();
        assert_eq!(stamp.len(), 14, "yyyymmddHHMMSS");
        assert!(stamp.bytes().all(|b| b.is_ascii_digit()));
        // Sanity: this decade, not 1970.
        let year: u32 = stamp[..4].parse().unwrap();
        assert!((2020..2100).contains(&year), "got year {year}");
    }

    #[test]
    fn an_error_envelope_becomes_an_api_error_with_its_code() {
        // The codes are the whole diagnostic value: 3102 is a wrong password,
        // 9999 is the API refusing.
        let refused = parse_envelope(r#"{"errorCode":"9999","msg":"system error"}"#);
        match refused {
            Err(CloudError::Api { code, message }) => {
                assert_eq!(code, 9999);
                assert_eq!(message, "system error");
            }
            other => panic!("expected an Api error, got {other:?}"),
        }

        // A numeric code, in case the API is inconsistent about it.
        match parse_envelope(r#"{"errorCode":3102,"msg":"nope"}"#) {
            Err(CloudError::Api { code, .. }) => assert_eq!(code, 3102),
            other => panic!("expected an Api error, got {other:?}"),
        }
    }

    #[test]
    fn a_success_envelope_yields_its_result() {
        let value = parse_envelope(r#"{"errorCode":"0","result":{"loginId":"abc"}}"#).unwrap();
        assert_eq!(value["loginId"], "abc");
    }

    #[test]
    fn a_malformed_envelope_is_an_error_not_a_panic() {
        for bad in [
            "",
            "not json",
            "{}",
            r#"{"errorCode":"0"}"#,
            r#"{"result":{}}"#,
        ] {
            assert!(
                parse_envelope(bad).is_err(),
                "{bad:?} should not have parsed"
            );
        }
    }

    #[test]
    fn a_random_device_id_is_hex_and_varies() {
        let a = random_hex(8);
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(a, random_hex(8));
    }
}
