//! 美的美居 (Meiju), Midea's app in mainland China.
//!
//! The same v5 API as SmartHome, with its own keys and server and no routing.
//! Its v1 `getToken` has answered `40404` since late August 2026; the
//! replacement, `/v2/iot/secure/getToken`, also wants the home the unit is in
//! (`homegroupId`) and `applianceCodes` as a list. v1 is still tried after v2,
//! for accounts the old endpoint serves.

use serde_json::{Map, Value};

use crate::v5::{self, Client, MEIJU};
use crate::{Appliance, CloudError, Credentials, Token};

/// A logged-in Meiju session.
pub struct Session {
    client: Client,
}

/// Whether Meiju has this account, asked without a password.
pub fn account_known(account: &str) -> Result<bool, CloudError> {
    account_known_at(account, None)
}

pub(crate) fn account_known_at(account: &str, base: Option<&str>) -> Result<bool, CloudError> {
    let mut client = Client::new(&MEIJU, account);
    if let Some(base) = base {
        client.set_base(base);
    }
    match client.login_id(account) {
        Ok(_) => Ok(true),
        // 1006 is how Meiju answers an account it cannot even read as one --
        // seen live for an email address: "mobile=… value is illegal". Either
        // way it has no such account.
        Err(CloudError::Api {
            code: 3102 | 1006, ..
        }) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Log in.
pub fn login(credentials: &Credentials) -> Result<Session, CloudError> {
    login_at(credentials, None)
}

pub(crate) fn login_at(
    credentials: &Credentials,
    base: Option<&str>,
) -> Result<Session, CloudError> {
    let account = credentials.account.as_str();
    let mut client = Client::new(&MEIJU, account);
    if let Some(base) = base {
        client.set_base(base);
    }
    let login_id = client.login_id(account)?;
    let stamp = crate::timestamp();
    let iot = serde_json::json!({
        "clientType": 1,
        "deviceId": client.device_id(),
        "iampwd": v5::md5_twice(credentials.password()),
        "iotAppId": MEIJU.app_id,
        "loginAccount": account,
        "password": v5::password(&MEIJU, &login_id, credentials.password()),
        "reqId": crate::random_hex(16),
        "stamp": stamp,
    });
    let mut data = Map::new();
    data.insert("iotData".into(), iot);
    data.insert(
        "data".into(),
        serde_json::json!({
            "appKey": MEIJU.app_key,
            "deviceId": client.device_id(),
            "platform": 2,
        }),
    );
    data.insert("timestamp".into(), stamp.clone().into());
    data.insert("stamp".into(), stamp.into());

    let answer = client.request("/mj/user/login", data)?;
    client.access_token = answer
        .pointer("/mdata/accessToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    if client.access_token.is_none() {
        return Err(CloudError::Malformed(
            "logged in, but no access token came back".into(),
        ));
    }
    Ok(Session { client })
}

impl Session {
    /// The account's homes: tokens are issued per home.
    fn homes(&self) -> Result<Vec<String>, CloudError> {
        let answer = self.client.request("/v1/homegroup/list/get", Map::new())?;
        Ok(answer
            .get("homeList")
            .and_then(Value::as_array)
            .map(|homes| {
                homes
                    .iter()
                    .filter_map(|h| h.get("homegroupId"))
                    .map(|id| {
                        id.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| id.to_string())
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The units paired with this account, across every home.
    pub fn appliances(&self) -> Result<Vec<Appliance>, CloudError> {
        let mut out = Vec::new();
        for home in self.homes()? {
            let mut data = Map::new();
            data.insert("homegroupId".into(), home.into());
            let answer = self.client.request("/v1/appliance/home/list/get", data)?;
            let rooms = answer
                .get("homeList")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|h| h.get("roomList").and_then(Value::as_array))
                .flatten();
            for room in rooms {
                for unit in room
                    .get("applianceList")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(a) = Appliance::from_cloud(unit) {
                        out.push(a);
                    }
                }
            }
        }
        Ok(out)
    }

    /// The unit's token and key: v2 in each home, then v1.
    pub fn token(&self, device_id: u64) -> Result<Token, CloudError> {
        let mut last = None;
        for home in self.homes()? {
            let attempt = crate::try_both_orders(|big_endian| {
                let wanted = crate::udpid(device_id, big_endian);
                let mut data = self.client.general();
                data.insert("homegroupId".into(), home.clone().into());
                data.insert("udpid".into(), wanted.clone().into());
                data.insert(
                    "applianceCodes".into(),
                    serde_json::json!([device_id.to_string()]),
                );
                let answer = self.client.request("/v2/iot/secure/getToken", data)?;
                crate::pick_token(&answer, &wanted)
            });
            match attempt {
                Ok(token) => return Ok(token),
                Err(e) => last = Some(e),
            }
        }
        // The old endpoint, for accounts it still serves.
        let v1 = crate::try_both_orders(|big_endian| {
            let wanted = crate::udpid(device_id, big_endian);
            let mut data = self.client.general();
            data.insert("udpid".into(), wanted.clone().into());
            data.insert("applianceCodes".into(), device_id.to_string().into());
            let answer = self.client.request("/v1/iot/secure/getToken", data)?;
            crate::pick_token(&answer, &wanted)
        });
        v1.map_err(|e| last.unwrap_or(e))
    }
}
