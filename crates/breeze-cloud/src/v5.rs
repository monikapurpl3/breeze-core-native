//! Midea's "v5 proxy" API, which SmartHome and Meiju both speak.
//!
//! Every request is a JSON POST to `<base>/<endpoint>`, where the base ends in
//! `proxy?alias=` and the endpoint is a path such as `/v1/user/login/id/get`.
//! Each one is signed: a `sign` header holding
//!
//! ```text
//! hex(HMAC-SHA256(hmac_key, iot_key + body + random))
//! ```
//!
//! over the exact body bytes sent, with `random` (the current unix time) in a
//! header of its own. Answers come back as `{"code": 0, "msg": …, "data": …}`.
//!
//! The constants are the vendors' apps' own, as every client of this API uses
//! them -- they identify the app, not a user. The protocol is reimplemented
//! here from its observed behaviour; midea-local and msmart-ng (both MIT) are
//! where it was first written down.

use std::time::Duration;

use hmac::{Hmac, Mac};
use md5::Md5;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::{hex, CloudError};

/// Which app a client is pretending to be.
pub(crate) struct Profile {
    pub app_id: &'static str,
    pub app_key: &'static str,
    /// Salts the password hashes. SmartHome uses its app key here; Meiju has a
    /// separate one.
    pub login_key: &'static str,
    pub iot_key: &'static str,
    pub hmac_key: &'static str,
    pub base_url: &'static str,
    /// SmartHome alone wants `x-recipe-app` and Basic authorisation on every
    /// request.
    pub smarthome_headers: bool,
}

/// MSmartHome, Midea's current app outside China.
pub(crate) const SMARTHOME: Profile = Profile {
    app_id: "1010",
    app_key: "ac21b9f9cbfe4ca5a88562ef25e2b768",
    login_key: "ac21b9f9cbfe4ca5a88562ef25e2b768",
    iot_key: "meicloud",
    hmac_key: "PROD_VnoClJI9aikS8dyy",
    base_url: "https://mp-prod.appsmb.com/mas/v5/app/proxy?alias=",
    smarthome_headers: true,
};

/// 美的美居 (Meiju), the app in mainland China.
pub(crate) const MEIJU: Profile = Profile {
    app_id: "900",
    app_key: "46579c15",
    login_key: "ad0ee21d48a64bf49f4fb583ab76e799",
    iot_key: "prod_secret123@muc",
    hmac_key: "PROD_VnoClJI9aikS8dyy",
    base_url: "https://mp-prod.smartmidea.net/mas/v5/app/proxy?alias=",
    smarthome_headers: false,
};

/// As for NetHome Plus: long enough for a slow mobile API, short enough that a
/// person waiting does not conclude it has hung.
const TIMEOUT: Duration = Duration::from_secs(20);

/// One account's conversation with one cloud.
pub(crate) struct Client {
    profile: &'static Profile,
    base: String,
    /// Derived from the account, as the apps derive it, so repeated runs look
    /// like one phone rather than a new one each time.
    device_id: String,
    pub uid: Option<String>,
    pub access_token: Option<String>,
}

impl Client {
    pub fn new(profile: &'static Profile, account: &str) -> Self {
        Self {
            profile,
            base: profile.base_url.to_string(),
            device_id: device_id(account),
            uid: None,
            access_token: None,
        }
    }

    /// Point at another base: SmartHome's routing answer, or a test server.
    pub fn set_base(&mut self, base: impl Into<String>) {
        self.base = base.into();
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// The fields every request carries, as the apps send them.
    pub fn general(&self) -> Map<String, Value> {
        let mut data = Map::new();
        data.insert("src".into(), self.profile.app_id.into());
        data.insert("format".into(), "2".into());
        data.insert("stamp".into(), crate::timestamp().into());
        data.insert("platformId".into(), "1".into());
        data.insert("deviceId".into(), self.device_id.clone().into());
        data.insert("reqId".into(), crate::random_hex(16).into());
        data.insert(
            "uid".into(),
            self.uid.clone().map(Value::from).unwrap_or(Value::Null),
        );
        data.insert("clientType".into(), "1".into());
        data.insert("appId".into(), self.profile.app_id.into());
        data.insert("language".into(), "en_US".into());
        data
    }

    /// One signed request, unwrapping the envelope.
    pub fn request(
        &self,
        endpoint: &str,
        mut data: Map<String, Value>,
    ) -> Result<Value, CloudError> {
        data.entry("reqId")
            .or_insert_with(|| crate::random_hex(16).into());
        data.entry("stamp")
            .or_insert_with(|| crate::timestamp().into());
        // Signed over these exact bytes, so they are serialised once and sent
        // as they are.
        let body = Value::Object(data).to_string();
        let random = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .to_string();
        let signature = sign(self.profile, &body, &random);

        let mut request = ureq::post(&format!("{}{endpoint}", self.base))
            .timeout(TIMEOUT)
            .set("content-type", "application/json; charset=utf-8")
            .set("secretVersion", "1")
            .set("sign", &signature)
            .set("random", &random);
        if let Some(uid) = &self.uid {
            request = request.set("uid", uid);
        }
        if let Some(token) = &self.access_token {
            request = request.set("accessToken", token);
        }
        if self.profile.smarthome_headers {
            request = request
                .set("x-recipe-app", self.profile.app_id)
                .set("authorization", &format!("Basic {}", basic(self.profile)));
        }

        let text = match request.send_string(&body) {
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

    /// `/v1/user/login/id/get`: the account's loginId, which salts the
    /// password hashes. **It sends no password**, so it is also how to ask
    /// whether this cloud knows an account at all without risking anything.
    pub fn login_id(&self, account: &str) -> Result<String, CloudError> {
        let mut data = self.general();
        data.insert("loginAccount".into(), account.into());
        let response = self.request("/v1/user/login/id/get", data)?;
        response
            .get("loginId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| CloudError::Malformed("no loginId in the response".into()))
    }
}

/// `{"code": 0, "msg": "...", "data": {...}}`, the code a number or a string.
pub(crate) fn parse_envelope(text: &str) -> Result<Value, CloudError> {
    let body: Value =
        serde_json::from_str(text).map_err(|e| CloudError::Malformed(e.to_string()))?;
    let code = body
        .get("code")
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .ok_or_else(|| CloudError::Malformed("no code in the response".into()))?;
    if code != 0 {
        let message = body
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("no message")
            .to_string();
        return Err(CloudError::Api { code, message });
    }
    Ok(body.get("data").cloned().unwrap_or(Value::Null))
}

/// `hex(HMAC-SHA256(hmac_key, iot_key + body + random))`.
pub(crate) fn sign(profile: &Profile, body: &str, random: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(profile.hmac_key.as_bytes())
        .expect("HMAC takes a key of any length");
    mac.update(profile.iot_key.as_bytes());
    mac.update(body.as_bytes());
    mac.update(random.as_bytes());
    hex(&mac.finalize().into_bytes())
}

/// SmartHome's Basic authorisation: the app key and iot key, base64'd.
pub(crate) fn basic(profile: &Profile) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .encode(format!("{}:{}", profile.app_key, profile.iot_key))
}

/// The apps' device id: the first 16 hex characters of
/// `sha256("Hello, <account>!")`.
pub(crate) fn device_id(account: &str) -> String {
    hex(&Sha256::digest(format!("Hello, {account}!").as_bytes()))[..16].to_string()
}

/// `sha256(login_id + sha256hex(password) + login_key)`.
///
/// Midea's login wire format, not password storage: the server computes the
/// same and compares, and nothing here keeps the result -- which is why a slow
/// password hash is not an option, however a scanner reads it.
pub(crate) fn password(profile: &Profile, login_id: &str, password: &str) -> String {
    let first = hex(&Sha256::digest(password.as_bytes()));
    hex(&Sha256::digest(
        format!("{login_id}{first}{}", profile.login_key).as_bytes(),
    ))
}

/// `md5hex(md5hex(password))`: Meiju's `iampwd` as it is, and the middle of
/// SmartHome's.
pub(crate) fn md5_twice(password: &str) -> String {
    let once = hex(&Md5::digest(password.as_bytes()));
    hex(&Md5::digest(once.as_bytes()))
}

/// SmartHome's `iampwd`: `sha256(login_id + md5hex(md5hex(password)) + login_key)`.
pub(crate) fn smarthome_iampwd(login_id: &str, password: &str) -> String {
    hex(&Sha256::digest(
        format!("{login_id}{}{}", md5_twice(password), SMARTHOME.login_key).as_bytes(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every expected value here was computed with Python's hashlib and hmac,
    // not with this code, so these test the construction rather than restating
    // it.

    #[test]
    fn hmac_sha256_matches_rfc_4231() {
        let mut mac = Hmac::<Sha256>::new_from_slice(b"Jefe").unwrap();
        mac.update(b"what do ya want for nothing?");
        assert_eq!(
            hex(&mac.finalize().into_bytes()),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn the_signature_covers_the_iot_key_the_body_and_the_random() {
        assert_eq!(
            sign(
                &SMARTHOME,
                r#"{"src":"1010","stamp":"20261001120000"}"#,
                "1790000000"
            ),
            "372cc269a5ccdd685c7d59b42df49e76999b3d009156ab997e7f3d8401e6d319"
        );
    }

    #[test]
    fn the_password_hashes_match_an_independent_construction() {
        let (id, pw) = ("0123456789abcdef", "correct horse");
        assert_eq!(
            password(&SMARTHOME, id, pw),
            "533207b76a4df182d33b8c74d0e138157d7d5ec5d499f7f20fef155eadd29835"
        );
        assert_eq!(
            smarthome_iampwd(id, pw),
            "f5a045a15bd14610119bcaa71a6cb7e2a6c52c194a45b085d8fb45ea3358266c"
        );
        assert_eq!(
            password(&MEIJU, id, pw),
            "b1deddd6f6b752476efefb86bb054b6a75eed5aa8ba75587238541a5f046d843"
        );
        assert_eq!(md5_twice(pw), "8e076cc271a46d554a161e4579dfe5e8");
    }

    #[test]
    fn the_device_id_is_derived_from_the_account() {
        assert_eq!(device_id("someone@example.com"), "50a76af52ac76251");
    }

    #[test]
    fn smarthome_basic_auth_is_the_two_keys() {
        assert_eq!(
            basic(&SMARTHOME),
            "YWMyMWI5ZjljYmZlNGNhNWE4ODU2MmVmMjVlMmI3Njg6bWVpY2xvdWQ="
        );
    }

    #[test]
    fn an_envelope_with_a_string_code_is_still_read() {
        assert!(matches!(
            parse_envelope(r#"{"code":"3102","msg":"Account or password incorrect"}"#),
            Err(CloudError::Api { code: 3102, .. })
        ));
        assert_eq!(
            parse_envelope(r#"{"code":0,"data":{"loginId":"x"}}"#).unwrap()["loginId"],
            "x"
        );
        assert!(matches!(
            parse_envelope("<html>"),
            Err(CloudError::Malformed(_))
        ));
    }
}
