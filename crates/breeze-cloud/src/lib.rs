//! Fetching a unit's V3 credentials from the vendor cloud, once.
//!
//! # Why this exists at all, reluctantly
//!
//! A V3 unit will not answer anything without a `token` and a `key`. They are
//! not derivable, and the unit will not hand them over: a bare `0x5A5A` packet
//! to a real unit here got no reply at all, and the V3 handshake proves
//! knowledge of the key without ever transmitting it. The only issuer is Midea.
//!
//! And Midea has been closing the door. As of October 2026:
//!
//! | cloud | app id | what happens |
//! |---|---|---|
//! | MSmartHome (`mp-prod.appsmb.com`) | 1010 | **works for the account the unit is paired with**, given `applianceCodes`; any other account gets `3201 no permissions` |
//! | 美的美居 Meiju (`mp-prod.smartmidea.net`) | 900 | the same, through v2 `getToken` with the unit's home; v1 answers `40404` since late August 2026 |
//! | NetHome Plus (`mapp.appsmb.com`) | 1017 | logs in, then `9999 system error` for every unit, its owner's included, since about 19 August 2026 |
//!
//! So shared and borrowed accounts are finished everywhere: the token is
//! issued to the unit's own account or not at all. An account made in NetHome
//! Plus does not exist on SmartHome (it answers `3102` before a password is
//! even sent), so a NetHome Plus unit has to be paired to a SmartHome account
//! first. `breeze-core fetch` walks a person through all of that.
//!
//! # So the shape of this is deliberate
//!
//! * **Credentials are borrowed, never kept.** They are used for one exchange
//!   and dropped. Nothing writes them anywhere, and [`Credentials`] scrubs itself
//!   on the way out.
//! * **It is a last resort, not the happy path.** The durable way to hold a V3
//!   unit's credentials is to *have* them: `POST /api/units` takes a `token` and
//!   `key` directly, and `config.json` is the backup. The unit never forgets its
//!   token; only Midea can stop handing it out.
//! * **Whether a cloud knows an account is asked without a password**
//!   ([`Cloud::account_known`]), so the password goes only to a cloud that has
//!   the account.

use std::fmt;

pub mod meiju;
pub mod nethome;
pub mod smarthome;
mod v5;

#[cfg(test)]
mod mock_cloud;

/// A Midea account, held only as long as one exchange takes.
///
/// Not `Clone`, and its `Debug` shows nothing: a password in a log outlives the
/// request it was for.
pub struct Credentials {
    pub account: String,
    password: String,
    pub region: String,
}

impl Credentials {
    pub fn new(
        account: impl Into<String>,
        password: impl Into<String>,
        region: impl Into<String>,
    ) -> Self {
        Self {
            account: account.into(),
            password: password.into(),
            region: region.into(),
        }
    }

    /// The password, for the one function that has to hash it.
    pub fn password(&self) -> &str {
        &self.password
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The account is a useful diagnostic; the password is never printable.
        f.debug_struct("Credentials")
            .field("account", &self.account)
            .field("password", &"<redacted>")
            .field("region", &self.region)
            .finish()
    }
}

impl Drop for Credentials {
    fn drop(&mut self) {
        // Overwrite before the allocation goes back. Not a guarantee — the
        // compiler may have copied it, and a `String` that reallocated left the
        // old bytes behind — but it removes the easy case for nothing.
        scrub(&mut self.password);
    }
}

/// Overwrite a string's bytes in place.
fn scrub(text: &mut String) {
    // SAFETY: zero is valid UTF-8, so the string stays well-formed throughout.
    unsafe {
        for byte in text.as_bytes_mut() {
            *byte = 0;
        }
    }
    text.clear();
}

/// What a unit needs before it will accept a V3 session.
#[derive(Clone, PartialEq)]
pub struct Token {
    /// 64 bytes, hex.
    pub token: String,
    /// 32 bytes, hex.
    pub key: String,
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // These are credentials. Their lengths are the useful part.
        write!(
            f,
            "Token {{ token: <{} chars>, key: <{} chars> }}",
            self.token.len(),
            self.key.len()
        )
    }
}

#[derive(Debug)]
pub enum CloudError {
    /// The network, or TLS, or a timeout.
    Transport(String),
    /// The cloud answered with a refusal. Carries the code, because the codes
    /// are the only way to tell "wrong password" from "this API is gone".
    Api { code: i64, message: String },
    /// It answered, but not with a token for the unit that was asked about.
    NoToken(String),
    /// The response was not the shape it should be.
    Malformed(String),
    /// None of the clouds asked has this account at all.
    UnknownAccount,
}

impl fmt::Display for CloudError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "could not reach the cloud: {e}"),
            Self::Api { code, message } => {
                write!(f, "the cloud refused the request ({code}): {message}")
            }
            Self::NoToken(e) => write!(f, "{e}"),
            Self::Malformed(e) => write!(f, "the cloud answered something unexpected: {e}"),
            Self::UnknownAccount => write!(f, "no Midea cloud knows this account"),
        }
    }
}

impl std::error::Error for CloudError {}

impl CloudError {
    /// What to tell a person, as opposed to what to log.
    ///
    /// The codes seen in practice: `3102` is a wrong account or password, and
    /// `9999`/`3004`/`3201` are the API refusing whatever it is sent.
    pub fn advice(&self) -> &'static str {
        match self {
            Self::Api { code: 3101, .. } | Self::Api { code: 3102, .. } => {
                "That account or password was not accepted. It has to be the account \
                 of the Midea app the unit is paired with -- and an account made in \
                 NetHome Plus does not exist on MSmartHome: they are separate systems."
            }
            Self::Api { code: 9999, .. } => {
                "NetHome Plus logged in and then refused to issue a token, as it has \
                 for every unit, its owners' included, since August 2026. Pair the \
                 unit with an MSmartHome account and fetch with that (`breeze-core \
                 fetch` walks through it), or, if you already hold a token and key, \
                 add the unit with those instead."
            }
            Self::Api { code: 3201, .. } => {
                "The cloud logged in and then refused this unit, which means it is not \
                 paired with this account. Only the account a unit is paired with gets \
                 its token. If you already hold a token and key, add it with those."
            }
            Self::Api { code: 3004, .. } => {
                "The cloud rejected the request outright, which is what a withdrawn \
                 token API looks like. If you already hold a token and key, add the \
                 unit with those instead."
            }
            // Any other refusal: say what is known rather than guessing at a
            // code nobody here has seen.
            Self::Api { .. } => {
                "The cloud refused. If you already hold a token and key for this unit, add it with those instead."
            }
            Self::Transport(_) => "The cloud could not be reached at all.",
            Self::NoToken(_) => {
                "The cloud answered but had nothing for this unit, which means it is \
                 not paired with this account."
            }
            Self::UnknownAccount => {
                "Neither MSmartHome, Meiju nor NetHome Plus has this account (each was \
                 asked without the password). Check the spelling, or use the account \
                 of the app the unit is paired with."
            }
            Self::Malformed(_) => {
                "The cloud's answer could not be read. The API has probably changed."
            }
        }
    }
}

/// Which Midea app, and so which cloud, an account belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cloud {
    SmartHome,
    Meiju,
    NetHomePlus,
}

impl Cloud {
    /// The app's name as a person would see it on their phone.
    pub fn app_name(self) -> &'static str {
        match self {
            Self::SmartHome => "MSmartHome",
            Self::Meiju => "美的美居 (Meiju)",
            Self::NetHomePlus => "NetHome Plus",
        }
    }

    /// Whether this cloud has the account, asked without the password.
    ///
    /// NetHome Plus answers the same question the same way, but has nothing to
    /// offer once it says yes, so asking it is only useful for explaining why.
    pub fn account_known(self, account: &str) -> Result<bool, CloudError> {
        match self {
            Self::SmartHome => smarthome::account_known(account),
            Self::Meiju => meiju::account_known(account),
            Self::NetHomePlus => nethome::account_known(account),
        }
    }

    /// Log in, list the account's units, and fetch tokens for them.
    pub fn login(self, credentials: &Credentials) -> Result<Session, CloudError> {
        Ok(match self {
            Self::SmartHome => Session::SmartHome(smarthome::login(credentials)?),
            Self::Meiju => Session::Meiju(meiju::login(credentials)?),
            Self::NetHomePlus => Session::NetHomePlus(nethome::login(credentials)?),
        })
    }
}

/// A logged-in session with one of the clouds.
pub enum Session {
    SmartHome(smarthome::Session),
    Meiju(meiju::Session),
    NetHomePlus(nethome::Session),
}

impl Session {
    /// The units paired with the account, where the cloud will say. NetHome
    /// Plus is not asked: it issues no tokens any more, so its list would only
    /// raise hopes.
    pub fn appliances(&self) -> Result<Option<Vec<Appliance>>, CloudError> {
        match self {
            Self::SmartHome(s) => s.appliances().map(Some),
            Self::Meiju(s) => s.appliances().map(Some),
            Self::NetHomePlus(_) => Ok(None),
        }
    }

    pub fn token(&self, device_id: u64) -> Result<Token, CloudError> {
        match self {
            Self::SmartHome(s) => s.token(device_id),
            Self::Meiju(s) => s.token(device_id),
            Self::NetHomePlus(s) => nethome::fetch_with(s, device_id),
        }
    }
}

/// For a caller with nobody to talk to -- the panel's add-unit form: find the
/// cloud that knows the account, asking each without the password, log in to
/// that one alone, and fetch.
///
/// SmartHome first, because outside China it is the one that still issues
/// tokens; NetHome Plus last, because it no longer does, and its refusal is
/// still the most useful thing to tell a NetHome Plus user.
pub fn fetch_token_any(
    credentials: &Credentials,
    device_id: u64,
) -> Result<(Cloud, Token), CloudError> {
    let mut last = None;
    for cloud in [Cloud::SmartHome, Cloud::Meiju, Cloud::NetHomePlus] {
        match cloud.account_known(&credentials.account) {
            Ok(true) => {
                return cloud
                    .login(credentials)
                    .and_then(|session| session.token(device_id))
                    .map(|token| (cloud, token));
            }
            Ok(false) => {}
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or(CloudError::UnknownAccount))
}

/// One unit as an account's cloud lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Appliance {
    /// The id discovery reports, and the one tokens are keyed by.
    pub id: u64,
    pub name: String,
    /// `0xAC` for an air conditioner.
    pub kind: Option<u8>,
    pub online: bool,
}

impl Appliance {
    /// From either cloud's listing: SmartHome calls the id `id`, Meiju
    /// `applianceCode`, and either may send it as a string or a number.
    pub(crate) fn from_cloud(value: &serde_json::Value) -> Option<Self> {
        let raw = value.get("id").or_else(|| value.get("applianceCode"))?;
        let id = raw
            .as_u64()
            .or_else(|| raw.as_str().and_then(|s| s.trim().parse().ok()))?;
        let kind = value.get("type").and_then(|t| t.as_str()).and_then(|t| {
            u8::from_str_radix(t.trim_start_matches("0x").trim_start_matches("0X"), 16).ok()
        });
        Some(Self {
            id,
            name: value
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("unnamed")
                .to_string(),
            kind,
            online: value.get("onlineStatus").and_then(|s| s.as_str()) == Some("1"),
        })
    }
}

/// The token for `wanted` out of a `getToken` answer's `tokenlist`.
///
/// Only the entry for the udpid that was asked about: the list can carry other
/// units' entries, and taking the first would be the wrong unit's credentials.
pub(crate) fn pick_token(answer: &serde_json::Value, wanted: &str) -> Result<Token, CloudError> {
    let list = answer
        .get("tokenlist")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CloudError::Malformed("no tokenlist in the response".into()))?;
    for entry in list {
        let udpid = entry.get("udpId").and_then(|v| v.as_str()).unwrap_or("");
        if !udpid.eq_ignore_ascii_case(wanted) {
            continue;
        }
        if let (Some(token), Some(key)) = (
            entry.get("token").and_then(|v| v.as_str()),
            entry.get("key").and_then(|v| v.as_str()),
        ) {
            return Ok(Token {
                token: token.to_ascii_lowercase(),
                key: key.to_ascii_lowercase(),
            });
        }
    }
    Err(CloudError::NoToken(format!(
        "the cloud listed {} token(s), none of them for this unit",
        list.len()
    )))
}

/// Ask with each udpid byte order, little-endian first. Firmware is
/// inconsistent about it, and the wrong one simply finds nothing. A refusal is
/// about the account or the API, not the byte order, so it ends the search.
pub(crate) fn try_both_orders(
    mut ask: impl FnMut(bool) -> Result<Token, CloudError>,
) -> Result<Token, CloudError> {
    let mut last = None;
    for big_endian in [false, true] {
        match ask(big_endian) {
            Ok(token) => return Ok(token),
            Err(e @ CloudError::Api { .. }) => return Err(e),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| CloudError::NoToken("no token for this unit".into())))
}

/// The udpid the token API keys a device by.
///
/// `sha256` of the id as six bytes, with the digest's two halves XOR-ed
/// together. The byte order is not consistent across firmware, so callers try
/// both — which is what msmart does, and why this takes it as an argument rather
/// than guessing.
pub fn udpid(device_id: u64, big_endian: bool) -> String {
    use sha2::{Digest, Sha256};

    let bytes = device_id.to_be_bytes();
    // The low six bytes, in the requested order.
    let mut six = [0u8; 6];
    six.copy_from_slice(&bytes[2..8]);
    if !big_endian {
        six.reverse();
    }

    let digest = Sha256::digest(six);
    let folded: Vec<u8> = digest[..16]
        .iter()
        .zip(&digest[16..])
        .map(|(a, b)| a ^ b)
        .collect();
    hex(&folded)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// UTC, `yyyymmddHHMMSS`, as the API wants it.
pub(crate) fn timestamp() -> String {
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

pub(crate) fn random_hex(bytes: usize) -> String {
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
    fn a_password_never_shows_up_in_debug_output() {
        // The most important property in this crate: a request that gets logged,
        // or an error that gets formatted, must not carry the password with it.
        let creds = Credentials::new("someone@example.com", "hunter2", "DE");
        let rendered = format!("{creds:?}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(rendered.contains("redacted"));
        // The account is fine to show — it is what tells a person which login
        // was refused.
        assert!(rendered.contains("someone@example.com"));
    }

    #[test]
    fn a_token_never_shows_up_in_debug_output() {
        let token = Token {
            token: "aa".repeat(64),
            key: "bb".repeat(32),
        };
        let rendered = format!("{token:?}");
        assert!(!rendered.contains("aa"), "{rendered}");
        assert!(!rendered.contains("bb"), "{rendered}");
        assert!(rendered.contains("128 chars"));
        assert!(rendered.contains("64 chars"));
    }

    #[test]
    fn scrubbing_empties_a_string() {
        let mut secret = String::from("hunter2");
        scrub(&mut secret);
        assert!(secret.is_empty());
    }

    #[test]
    fn the_udpid_matches_the_reference_derivation() {
        // Computed independently, with Python, for units that exist:
        //   d = sha256(id.to_bytes(6, endian)).digest()
        //   bytes(a ^ b for a, b in zip(d[:16], d[16:])).hex()
        // This is the one value here that must agree with msmart exactly, or the
        // cloud is being asked about a device nobody has heard of. An earlier
        // draft of this test had invented the second half of these strings; they
        // are now the real ones.
        assert_eq!(
            udpid(153931628470980, false),
            "151e2a7e34ca256ffdd5239efe9f8173"
        );
        assert_eq!(
            udpid(153931628470980, true),
            "43fd91ead9d7f6567eb09f868593e954"
        );
        assert_eq!(
            udpid(152832116843678, false),
            "e2ab253624dfad06161eacc3d2905736"
        );
        assert_eq!(
            udpid(152832116843678, true),
            "2e4b9e43a767b7ce9dada5b3c66b59d0"
        );
        // A small id, to pin the six-byte padding rather than only 48-bit values.
        assert_eq!(udpid(1, false), "49a3ad37238dd4754c623adcebe2e3f5");
        assert_eq!(udpid(1, true), "13e129a6fd15446b720b556eb61c0c8b");
    }

    #[test]
    fn the_udpid_is_sixteen_bytes_of_hex_and_endian_sensitive() {
        let little = udpid(153931628470980, false);
        assert_eq!(little.len(), 32, "16 bytes, hex");
        assert!(little.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(
            little,
            udpid(153931628470980, true),
            "the byte order matters, which is why both get tried"
        );
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff]), "000fff");
        assert_eq!(hex(&[]), "");
    }

    #[test]
    fn the_advice_distinguishes_a_bad_password_from_a_dead_api() {
        let wrong_password = CloudError::Api {
            code: 3102,
            message: "Account or password incorrect".into(),
        };
        assert!(wrong_password.advice().contains("not accepted"));

        let refused = CloudError::Api {
            code: 9999,
            message: "system error".into(),
        };
        assert!(
            refused.advice().contains("MSmartHome"),
            "it should say what works now"
        );
        assert!(
            refused.advice().contains("token and key"),
            "it should point at the path that still works"
        );
        let not_theirs = CloudError::Api {
            code: 3201,
            message: "You have no permissions".into(),
        };
        assert!(not_theirs.advice().contains("not paired with this account"));
    }
}
