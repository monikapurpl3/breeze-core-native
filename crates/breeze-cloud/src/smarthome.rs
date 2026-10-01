//! MSmartHome -- Midea's current app outside China, and since August 2026 the
//! only cloud outside China that still issues tokens.
//!
//! It issues them only to the account a unit is paired with. Asked about a
//! unit some other account owns, `getToken` answers `3201 no permissions`; and
//! it insists on `applianceCodes` alongside the `udpid`, without which every
//! request is `3004 value is illegal`.
//!
//! The flow: ask which regional server holds the account, get its loginId (no
//! password yet), log in, then list the account's units and ask for a token.

use serde_json::Value;

use crate::v5::{self, Client, SMARTHOME};
use crate::{Appliance, CloudError, Credentials, Token};

/// A logged-in SmartHome session.
pub struct Session {
    client: Client,
}

/// A client pointed at the account's own regional server.
///
/// `/v1/multicloud/platform/user/route` takes the account name and answers
/// with the `masUrl` that serves it. Routing failing is not fatal -- the
/// default server answers for most accounts -- so only a refusal that names
/// the account as unknown is passed on.
fn routed(account: &str, base: Option<&str>) -> Result<Client, CloudError> {
    let mut client = Client::new(&SMARTHOME, account);
    if let Some(base) = base {
        client.set_base(base);
    }
    let mut data = client.general();
    data.insert("userType".into(), "0".into());
    data.insert("userName".into(), account.into());
    match client.request("/v1/multicloud/platform/user/route", data) {
        Ok(answer) => {
            if let Some(url) = answer.get("masUrl").and_then(Value::as_str) {
                // A test server stays the test server.
                if base.is_none() && !url.is_empty() {
                    client.set_base(url);
                }
            }
        }
        Err(e @ CloudError::Api { code: 10004, .. }) => return Err(e),
        Err(_) => {}
    }
    Ok(client)
}

/// Whether SmartHome has this account at all, asked without a password.
///
/// `Ok(false)` for the two answers that mean "no such account here": `10004`
/// from routing and `3102` from the loginId lookup. An account made in
/// NetHome Plus, or migrated from another Midea app, gets exactly these.
pub fn account_known(account: &str) -> Result<bool, CloudError> {
    account_known_at(account, None)
}

pub(crate) fn account_known_at(account: &str, base: Option<&str>) -> Result<bool, CloudError> {
    let client = match routed(account, base) {
        Ok(c) => c,
        Err(CloudError::Api { code: 10004, .. }) => return Ok(false),
        Err(e) => return Err(e),
    };
    match client.login_id(account) {
        Ok(_) => Ok(true),
        Err(CloudError::Api { code: 3102, .. }) => Ok(false),
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
    let mut client = routed(account, base)?;
    let login_id = client.login_id(account)?;

    let mut iot = client.general();
    iot.remove("uid");
    iot.insert(
        "iampwd".into(),
        v5::smarthome_iampwd(&login_id, credentials.password()).into(),
    );
    iot.insert("loginAccount".into(), account.into());
    iot.insert(
        "password".into(),
        v5::password(&SMARTHOME, &login_id, credentials.password()).into(),
    );
    let stamp = crate::timestamp();
    iot.insert("stamp".into(), stamp.clone().into());
    let mut data = serde_json::Map::new();
    data.insert("iotData".into(), Value::Object(iot));
    data.insert(
        "data".into(),
        serde_json::json!({
            "appKey": SMARTHOME.app_key,
            "deviceId": client.device_id(),
            "platform": "2",
        }),
    );
    data.insert("stamp".into(), stamp.into());

    let answer = client.request("/mj/user/login", data)?;
    client.uid = answer
        .get("uid")
        .and_then(Value::as_str)
        .map(str::to_string);
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
    /// The units paired with this account.
    pub fn appliances(&self) -> Result<Vec<Appliance>, CloudError> {
        let answer = self
            .client
            .request("/v1/appliance/user/list/get", self.client.general())?;
        let list = answer
            .get("list")
            .and_then(Value::as_array)
            .ok_or_else(|| CloudError::Malformed("no appliance list in the response".into()))?;
        Ok(list.iter().filter_map(Appliance::from_cloud).collect())
    }

    /// The unit's token and key, trying both udpid byte orders.
    pub fn token(&self, device_id: u64) -> Result<Token, CloudError> {
        crate::try_both_orders(|big_endian| {
            let wanted = crate::udpid(device_id, big_endian);
            let mut data = self.client.general();
            data.insert("udpid".into(), wanted.clone().into());
            // Without it, every request is "3004 value is illegal".
            data.insert("applianceCodes".into(), device_id.to_string().into());
            let answer = self.client.request("/v1/iot/secure/getToken", data)?;
            crate::pick_token(&answer, &wanted)
        })
    }
}
