//! Fetching a unit's V3 credentials from the vendor cloud, once.
//!
//! # Why this exists at all, reluctantly
//!
//! A V3 unit will not answer anything without a `token` and a `key`. They are
//! not derivable, and the unit will not hand them over: a bare `0x5A5A` packet
//! to a real unit here got no reply at all, and the V3 handshake proves
//! knowledge of the key without ever transmitting it. The only issuer is Midea.
//!
//! And Midea has been closing the door. Measured against the live services:
//!
//! | cloud | app id | what happens |
//! |---|---|---|
//! | MSmartHome (`mp-prod.appsmb.com`) | 1010 | `3004 value is illegal`; adding the `applianceCodes` field turns that into `3201 You have no permissions` |
//! | NetHome Plus (`mapp.appsmb.com`) | 1017 | logs in, then `9999 system error` for a unit the account does not own |
//!
//! Which matches what the wider community reports: token fetching is already
//! withdrawn on Meiju and SmartHome, NetHome Plus is the last one answering, and
//! it is expected to follow.
//!
//! # So the shape of this is deliberate
//!
//! * **Credentials are borrowed, never kept.** They are used for one exchange
//!   and dropped. Nothing writes them anywhere, and [`Credentials`] scrubs itself
//!   on the way out.
//! * **It is a last resort, not the happy path.** The durable way to hold a V3
//!   unit's credentials is to *have* them: `POST /api/units` takes a `token` and
//!   `key` directly, and `config.json` is the backup. When Midea finishes
//!   turning this off, that path keeps working and this one stops.
//! * **NetHome Plus only**, because it is the only one that still answers. There
//!   is no point carrying code for two dead APIs.

use std::fmt;

pub mod nethome;

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
            Self::Api { code: 3102, .. } => {
                "That account or password was not accepted. It has to be the Midea \
                 app account the unit is registered to."
            }
            Self::Api { code: 9999, .. } | Self::Api { code: 3201, .. } => {
                "The cloud accepted the login and then refused to issue a token. \
                 That usually means this account is not the one the unit is \
                 registered to. Midea has also been withdrawing this API, so it may \
                 simply be gone — if you already hold a token and key, add the unit \
                 with those instead."
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
                "The cloud answered but had nothing for this unit, which means this \
                 account does not have it registered."
            }
            Self::Malformed(_) => {
                "The cloud's answer could not be read. The API has probably changed."
            }
        }
    }
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
        assert!(refused.advice().contains("registered to"));
        assert!(
            refused.advice().contains("token and key"),
            "it should point at the path that still works"
        );
    }
}
