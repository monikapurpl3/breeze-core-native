//! Device pairing: the RFC 8628-style handshake.
//!
//! 1. A client presents the API key and gets a short code to show a human.
//! 2. An admin **on the LAN** relays that code to the approval endpoint, which
//!    mints the device's record.
//! 3. The client polls and collects the result exactly once.
//!
//! The point of the middle step is that the API key alone is only an *enrolment*
//! secret: knowing it is not enough to control anything, because a person on the
//! trusted network has to say yes. That is also why approval is LAN-restricted
//! and why losing pending sessions on restart is harmless — they are seconds old
//! and whoever was mid-pairing simply starts again. The minted records are
//! durable; the sessions are not.
//!
//! v1 and v2 differ in what approval produces: v1 mints a bearer token and hands
//! it over once, v2 stores only the public key the client already generated and
//! returns nothing secret at all.

use std::collections::HashMap;

use breeze_store::DeviceRecord;
use subtle::ConstantTimeEq;

use crate::signing;
use crate::verify::hash_secret;

/// How long a pairing code is good for. Short on purpose: a human is reading it
/// aloud or typing it, not storing it.
pub const CODE_TTL_SECONDS: u64 = 60;

/// How long a minted credential lasts. 0 or less means non-expiring.
pub const TOKEN_TTL_DAYS: i64 = 90;

/// Outcomes of a poll, spelled as the clients expect.
pub const PENDING: &str = "pending";
pub const APPROVED: &str = "approved";
pub const EXPIRED: &str = "expired";
pub const UNKNOWN: &str = "unknown";

#[derive(Debug)]
struct Session {
    code_hash: String,
    label: String,
    expires_at: f64,
    auth_version: u8,
    public_key: Option<String>,
    approved: bool,
    /// The one-time bearer, v1 only.
    token: Option<String>,
    record: Option<DeviceRecord>,
}

/// What a client gets back from `poll`.
#[derive(Debug, Clone, PartialEq)]
pub struct PollOutcome {
    pub status: &'static str,
    /// Present exactly once, and only for v1.
    pub device_token: Option<String>,
    pub record: Option<DeviceRecord>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EnrollError {
    /// A v2 enrolment arrived without a usable Ed25519 public key.
    BadPublicKey,
    /// The system CSPRNG failed, so nothing safe can be minted.
    NoEntropy,
}

impl core::fmt::Display for EnrollError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadPublicKey => write!(f, "public_key is not a valid Ed25519 key"),
            Self::NoEntropy => write!(f, "no entropy available to mint a credential"),
        }
    }
}

impl std::error::Error for EnrollError {}

fn random_bytes(n: usize) -> Result<Vec<u8>, EnrollError> {
    let mut buf = vec![0u8; n];
    getrandom::getrandom(&mut buf).map_err(|_| EnrollError::NoEntropy)?;
    Ok(buf)
}

fn b64url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use core::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// A short code a person can read aloud: base32, uppercase, `XXXX-XXXX`.
///
/// Base32 avoids the case ambiguity of base64 and the hyphen makes it easier to
/// dictate. Five bytes is 40 bits, which is ample inside a 60-second,
/// single-use window.
///
/// It said "rate-limited window" until 4.0.1, and nothing here is rate limited:
/// the reference throttles the three enrolment endpoints per source address and
/// this does not. Recorded rather than quietly reworded, because a comment
/// describing a control that does not exist is worse than no comment.
pub fn new_pairing_code() -> Result<String, EnrollError> {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let raw = random_bytes(5)?;
    // Exactly one base32 group: 5 bytes -> 8 characters, no padding needed.
    let mut n: u64 = 0;
    for b in &raw {
        n = (n << 8) | *b as u64;
    }
    let mut chars = [0u8; 8];
    for (i, slot) in chars.iter_mut().enumerate() {
        let shift = 35 - (i * 5);
        *slot = ALPHABET[((n >> shift) & 0x1F) as usize];
    }
    let text = String::from_utf8_lossy(&chars).into_owned();
    Ok(format!("{}-{}", &text[..4], &text[4..]))
}

/// A fresh API key: 24 random bytes, base64url, 32 characters.
///
/// The same shape the reference mints (`secrets.token_urlsafe(24)`), so a
/// config written by `breeze-core pair` is indistinguishable from one written by
/// the Python tool — which matters because people paste this key into a phone
/// and would notice it changing character.
///
/// Lives here rather than in the CLI so that every credential this project mints
/// comes from one place, with one entropy failure path.
pub fn random_api_key() -> Result<String, EnrollError> {
    Ok(b64url(&random_bytes(24)?))
}

/// Canonicalise a human-entered code: uppercase, no spaces or hyphens.
///
/// So `k7q2 9mrx` and `K7Q2-9MRX` are the same code, because someone is typing
/// it from memory or from a phone screen.
pub fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(|c| c.to_uppercase())
        .collect()
}

pub struct EnrollmentService {
    sessions: HashMap<String, Session>,
    code_ttl: u64,
    token_ttl_days: i64,
}

impl Default for EnrollmentService {
    fn default() -> Self {
        Self::new(CODE_TTL_SECONDS, TOKEN_TTL_DAYS)
    }
}

impl EnrollmentService {
    pub fn new(code_ttl: u64, token_ttl_days: i64) -> Self {
        Self {
            sessions: HashMap::new(),
            code_ttl,
            token_ttl_days,
        }
    }

    /// Open a pending session. Returns `(session_id, user_code, expires_in)`.
    ///
    /// Only the code's hash is retained: the plaintext exists to be shown to a
    /// human and is never stored.
    pub fn start(
        &mut self,
        label: &str,
        auth_version: u8,
        public_key: Option<&str>,
        now: f64,
    ) -> Result<(String, String, u64), EnrollError> {
        if auth_version == 2 {
            match public_key {
                Some(k) if signing::public_key_is_valid(k) => {}
                // Refused here rather than at first use, so a malformed key
                // fails while someone is watching the pairing screen.
                _ => return Err(EnrollError::BadPublicKey),
            }
        }
        self.sweep(now);
        let session_id = b64url(&random_bytes(18)?);
        let user_code = new_pairing_code()?;
        let label = label.trim();
        self.sessions.insert(
            session_id.clone(),
            Session {
                code_hash: hash_secret(&normalize_code(&user_code)),
                label: if label.is_empty() {
                    "unnamed device".to_string()
                } else {
                    label.to_string()
                },
                expires_at: now + self.code_ttl as f64,
                auth_version,
                public_key: public_key.map(str::to_string),
                approved: false,
                token: None,
                record: None,
            },
        );
        Ok((session_id, user_code, self.code_ttl))
    }

    /// Approve a pending session by its code, producing the device's record.
    ///
    /// Compares against every live session without an early exit, so a wrong
    /// code is not distinguishable from an expired one by timing.
    pub fn approve(&mut self, user_code: &str, now: f64) -> Option<DeviceRecord> {
        self.sweep(now);
        let target = hash_secret(&normalize_code(user_code));
        let mut matched: Option<String> = None;
        for (id, session) in &self.sessions {
            if session.approved {
                continue;
            }
            let same: bool = session.code_hash.len() == target.len()
                && session.code_hash.as_bytes().ct_eq(target.as_bytes()).into();
            if same {
                matched = Some(id.clone());
            }
        }
        let id = matched?;

        let expires_at = if self.token_ttl_days <= 0 {
            None
        } else {
            Some(now + self.token_ttl_days as f64 * 86400.0)
        };
        let token_id = hex(&random_bytes(8).ok()?);

        let session = self.sessions.get_mut(&id)?;
        let (record, token) = if session.auth_version == 2 {
            // Nothing secret is minted: the device already holds its private key.
            (
                DeviceRecord {
                    token_id,
                    label: session.label.clone(),
                    auth_version: 2,
                    token_hash: None,
                    public_key: session.public_key.clone(),
                    created_at: now,
                    expires_at,
                    last_used: None,
                },
                None,
            )
        } else {
            let token = b64url(&random_bytes(32).ok()?);
            (
                DeviceRecord {
                    token_id,
                    label: session.label.clone(),
                    auth_version: 1,
                    // Only the hash is stored; the token itself is handed over once.
                    token_hash: Some(hash_secret(&token)),
                    public_key: None,
                    created_at: now,
                    expires_at,
                    last_used: None,
                },
                Some(token),
            )
        };
        session.approved = true;
        session.token = token;
        session.record = Some(record.clone());
        Some(record)
    }

    /// Check a session, delivering the credential exactly once.
    pub fn poll(&mut self, session_id: &str, now: f64) -> PollOutcome {
        self.sweep(now);
        let Some(session) = self.sessions.get_mut(session_id) else {
            // Either it never existed or it expired and was swept. A client is
            // told to start over either way.
            return PollOutcome {
                status: UNKNOWN,
                device_token: None,
                record: None,
            };
        };
        if !session.approved {
            return PollOutcome {
                status: PENDING,
                device_token: None,
                record: None,
            };
        }
        // Consume the session: the bearer token must never be collectable twice.
        let token = session.token.take();
        let record = session.record.clone();
        self.sessions.remove(session_id);
        PollOutcome {
            status: APPROVED,
            device_token: token,
            record,
        }
    }

    pub fn pending(&self) -> usize {
        self.sessions.len()
    }

    /// Drop sessions past their expiry. An approved session is kept until it is
    /// collected, so a slow client does not lose a credential that was minted
    /// for it — but only until its code would have expired anyway.
    fn sweep(&mut self, now: f64) {
        self.sessions.retain(|_, s| s.expires_at >= now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 1_787_488_496.0;

    #[test]
    fn a_pairing_code_is_readable_and_unpredictable() {
        let a = new_pairing_code().unwrap();
        assert_eq!(a.len(), 9, "XXXX-XXXX");
        assert_eq!(a.chars().nth(4), Some('-'));
        assert!(
            a.chars()
                .all(|c| c == '-' || c.is_ascii_uppercase() || c.is_ascii_digit()),
            "unreadable code {a}"
        );
        // Base32's alphabet excludes 0, 1, 8 and 9 to avoid O/I/B confusion.
        assert!(!a.contains('0') && !a.contains('1'));
        let b = new_pairing_code().unwrap();
        assert_ne!(a, b, "codes must not repeat");
    }

    #[test]
    fn codes_are_compared_the_way_people_type_them() {
        assert_eq!(normalize_code("k7q2 9mrx"), "K7Q29MRX");
        assert_eq!(normalize_code("K7Q2-9MRX"), "K7Q29MRX");
        assert_eq!(normalize_code(" k7q2-9mrx "), "K7Q29MRX");
    }

    #[test]
    fn the_v1_flow_delivers_a_bearer_token_exactly_once() {
        let mut e = EnrollmentService::default();
        let (session, code, ttl) = e.start("Phone", 1, None, NOW).unwrap();
        assert_eq!(ttl, CODE_TTL_SECONDS);

        // Before approval, pending -- and no credential.
        let out = e.poll(&session, NOW);
        assert_eq!(out.status, PENDING);
        assert!(out.device_token.is_none());

        let record = e.approve(&code, NOW).expect("approval");
        assert_eq!(record.auth_version, 1);
        assert!(record.token_hash.is_some(), "v1 stores a hash");
        assert!(record.public_key.is_none());

        let out = e.poll(&session, NOW);
        assert_eq!(out.status, APPROVED);
        let token = out.device_token.expect("v1 must hand over a token");
        // The stored hash must match the token that was handed out.
        assert_eq!(
            record.token_hash.as_deref(),
            Some(hash_secret(&token).as_str())
        );

        // Collecting twice must be impossible.
        let again = e.poll(&session, NOW);
        assert_eq!(again.status, UNKNOWN);
        assert!(again.device_token.is_none());
    }

    #[test]
    fn the_v2_flow_returns_no_secret_at_all() {
        use base64::Engine;
        let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            ed25519_dalek::SigningKey::from_bytes(&[9u8; 32])
                .verifying_key()
                .to_bytes(),
        );
        let mut e = EnrollmentService::default();
        let (session, code, _) = e.start("Panel", 2, Some(&key), NOW).unwrap();
        let record = e.approve(&code, NOW).unwrap();
        assert_eq!(record.auth_version, 2);
        assert_eq!(record.public_key.as_deref(), Some(key.as_str()));
        assert!(record.token_hash.is_none(), "v2 must store no secret");

        let out = e.poll(&session, NOW);
        assert_eq!(out.status, APPROVED);
        assert!(
            out.device_token.is_none(),
            "v2 has nothing to hand back -- the private key never left the device"
        );
    }

    #[test]
    fn a_v2_enrolment_without_a_usable_key_is_refused_up_front() {
        let mut e = EnrollmentService::default();
        for bad in [None, Some(""), Some("not-base64!!"), Some("aGVsbG8")] {
            assert_eq!(e.start("x", 2, bad, NOW), Err(EnrollError::BadPublicKey));
        }
        assert_eq!(e.pending(), 0, "a refused start must not leave a session");
    }

    #[test]
    fn a_wrong_code_approves_nothing() {
        let mut e = EnrollmentService::default();
        let (_, code, _) = e.start("Phone", 1, None, NOW).unwrap();
        assert!(e.approve("AAAA-AAAA", NOW).is_none());
        // The real code still works afterwards.
        assert!(e.approve(&code, NOW).is_some());
    }

    #[test]
    fn a_code_cannot_be_approved_twice() {
        let mut e = EnrollmentService::default();
        let (_, code, _) = e.start("Phone", 1, None, NOW).unwrap();
        assert!(e.approve(&code, NOW).is_some());
        assert!(
            e.approve(&code, NOW).is_none(),
            "a single-use code must not mint a second credential"
        );
    }

    #[test]
    fn an_expired_code_is_gone_rather_than_approvable() {
        let mut e = EnrollmentService::default();
        let (session, code, _) = e.start("Phone", 1, None, NOW).unwrap();
        let later = NOW + CODE_TTL_SECONDS as f64 + 1.0;
        assert!(e.approve(&code, later).is_none());
        assert_eq!(e.poll(&session, later).status, UNKNOWN);
        assert_eq!(e.pending(), 0, "expired sessions must not accumulate");
    }

    #[test]
    fn polling_an_unknown_session_is_not_an_error() {
        let mut e = EnrollmentService::default();
        assert_eq!(e.poll("never-existed", NOW).status, UNKNOWN);
    }

    #[test]
    fn a_zero_ttl_mints_a_non_expiring_credential() {
        let mut e = EnrollmentService::new(60, 0);
        let (_, code, _) = e.start("Forever", 1, None, NOW).unwrap();
        let record = e.approve(&code, NOW).unwrap();
        assert!(record.expires_at.is_none());
    }

    #[test]
    fn the_default_ttl_is_ninety_days() {
        let mut e = EnrollmentService::default();
        let (_, code, _) = e.start("Phone", 1, None, NOW).unwrap();
        let record = e.approve(&code, NOW).unwrap();
        let expires = record.expires_at.expect("should expire");
        assert_eq!(expires, NOW + 90.0 * 86400.0);
    }

    #[test]
    fn an_empty_label_gets_a_placeholder_rather_than_being_blank() {
        let mut e = EnrollmentService::default();
        let (_, code, _) = e.start("   ", 1, None, NOW).unwrap();
        assert_eq!(e.approve(&code, NOW).unwrap().label, "unnamed device");
    }

    #[test]
    fn concurrent_enrolments_do_not_collide() {
        let mut e = EnrollmentService::default();
        let (s1, c1, _) = e.start("One", 1, None, NOW).unwrap();
        let (s2, c2, _) = e.start("Two", 1, None, NOW).unwrap();
        assert_ne!(s1, s2);
        assert_ne!(c1, c2);
        // Approving one must not approve the other.
        e.approve(&c1, NOW).unwrap();
        assert_eq!(e.poll(&s2, NOW).status, PENDING);
        assert_eq!(e.poll(&s1, NOW).status, APPROVED);
    }
}
