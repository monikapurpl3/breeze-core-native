//! Deciding whether a request may proceed.
//!
//! Two credentials are required for the control API: the shared API key (an
//! *enrolment* secret on its own) and a per-device credential. A device is
//! pinned to the scheme it enrolled with — a v2 device cannot fall back to a
//! bearer token, and a v1 device cannot present a signature — so an attacker
//! cannot downgrade one.
//!
//! The ordering inside v2 verification is deliberate and load-bearing; see
//! [`Verifier::verify_device`].

use breeze_store::{DeviceRecord, DevicesDoc};
use sha2::Digest as _;
use subtle::ConstantTimeEq;

use crate::nonce::NonceCache;
use crate::reject::{Reason, Rejection, SkewInfo};
use crate::signing;

/// What a request presents, lifted out of whatever HTTP layer is in use so this
/// crate stays free of one.
#[derive(Debug, Clone, Default)]
pub struct Presented<'a> {
    pub api_key: Option<&'a str>,
    /// The bearer token, already stripped of its `Bearer ` prefix.
    pub bearer: Option<&'a str>,
    pub auth_version: Option<&'a str>,
    pub key_id: Option<&'a str>,
    pub timestamp: Option<&'a str>,
    pub nonce: Option<&'a str>,
    pub signature: Option<&'a str>,
    pub method: &'a str,
    /// Path including the query string, because that is what clients sign.
    pub path: &'a str,
    pub body: &'a [u8],
}

/// Parse an `Authorization` header value into a bearer token.
pub fn bearer_from_header(header: &str) -> Option<&str> {
    let (scheme, token) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

/// Hex SHA-256 of a secret, for comparison against a stored `token_hash`.
///
/// A fast hash is correct here: device tokens are 256 bits of entropy, so there
/// is no low-entropy secret for a slow KDF to protect.
pub fn hash_secret(secret: &str) -> String {
    let digest = sha2::Sha256::digest(secret.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        use core::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Outcome of a successful check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authenticated {
    pub token_id: String,
    pub auth_version: u8,
}

/// Either proceed, refuse, or tell the client to update.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow(Authenticated),
    Reject(Rejection),
    /// 426: the credential is valid but its scheme is below the configured floor.
    UpgradeRequired {
        min_auth_version: u8,
    },
}

pub struct Verifier {
    min_auth_version: u8,
}

impl Verifier {
    pub fn new(min_auth_version: u8) -> Self {
        Self { min_auth_version }
    }

    /// Constant-time API key check.
    ///
    /// Constant-time so a present-but-wrong key is not distinguishable by timing
    /// from an absent one, and the two get different reason codes so a client can
    /// tell "I sent nothing" from "I sent something wrong".
    pub fn verify_api_key(
        &self,
        presented: Option<&str>,
        expected: Option<&str>,
    ) -> Option<Rejection> {
        let presented = presented.unwrap_or("");
        let expected = expected.unwrap_or("");
        let ok = presented.len() == expected.len()
            && presented.as_bytes().ct_eq(expected.as_bytes()).into();
        if ok && !expected.is_empty() {
            return None;
        }
        Some(Rejection::unauthorized(
            if presented.is_empty() {
                Reason::NoCredential
            } else {
                Reason::BadApiKey
            },
            "missing or invalid X-API-Key header",
        ))
    }

    /// Verify the per-device credential.
    ///
    /// A client announcing `X-Breeze-Auth-Version: 2` is held to the signature
    /// scheme; anything else is treated as legacy bearer. That dispatch, rather
    /// than "try both", is what stops a downgrade.
    pub fn verify_device(
        &self,
        devices: &DevicesDoc,
        presented: &Presented<'_>,
        nonces: &mut NonceCache,
        now: f64,
    ) -> Decision {
        if presented.auth_version == Some("2") {
            self.verify_v2(devices, presented, nonces, now)
        } else {
            self.verify_v1(devices, presented, now)
        }
    }

    fn verify_v1(&self, devices: &DevicesDoc, presented: &Presented<'_>, now: f64) -> Decision {
        let token = presented.bearer.unwrap_or("");
        let record = if token.is_empty() {
            None
        } else {
            find_by_secret(devices, token, now)
        };
        let Some(record) = record else {
            return Decision::Reject(Rejection::unauthorized(
                if token.is_empty() {
                    Reason::NoCredential
                } else {
                    Reason::UnknownKey
                },
                "missing, invalid, or expired device token — enroll this device to get one",
            ));
        };
        if record.auth_version < self.min_auth_version {
            return Decision::UpgradeRequired {
                min_auth_version: self.min_auth_version,
            };
        }
        Decision::Allow(Authenticated {
            token_id: record.token_id.clone(),
            auth_version: record.auth_version,
        })
    }

    fn verify_v2(
        &self,
        devices: &DevicesDoc,
        p: &Presented<'_>,
        nonces: &mut NonceCache,
        now: f64,
    ) -> Decision {
        let (Some(key_id), Some(timestamp), Some(nonce), Some(signature)) =
            (p.key_id, p.timestamp, p.nonce, p.signature)
        else {
            return Decision::Reject(Rejection::unauthorized(
                Reason::IncompleteSignature,
                "incomplete signature headers",
            ));
        };
        if key_id.is_empty() || timestamp.is_empty() || nonce.is_empty() || signature.is_empty() {
            return Decision::Reject(Rejection::unauthorized(
                Reason::IncompleteSignature,
                "incomplete signature headers",
            ));
        }

        let Some(record) = find_by_key_id(devices, key_id, now) else {
            return Decision::Reject(Rejection::unauthorized(
                Reason::UnknownKey,
                "unknown, expired, or non-v2 device key",
            ));
        };
        if record.auth_version < self.min_auth_version {
            return Decision::UpgradeRequired {
                min_auth_version: self.min_auth_version,
            };
        }

        // Clock first: this is the failure a drifting phone hits, and answering
        // it with our own time is what lets the client self-heal instead of
        // concluding its credential is dead.
        if !nonces.timestamp_in_window(timestamp, now) {
            return Decision::Reject(
                Rejection::unauthorized(
                    Reason::ClockSkew,
                    "request timestamp outside the allowed window — check the device clock",
                )
                .with_skew(SkewInfo {
                    server_time: (now * 1000.0).round() / 1000.0,
                    client_time: timestamp.to_string(),
                    max_skew_seconds: nonces.skew_seconds(),
                }),
            );
        }

        let public_key = record.public_key.as_deref().unwrap_or("");
        let canonical = signing::build_canonical(p.method, p.path, timestamp, nonce, p.body);
        if !signing::verify_signature(public_key, &canonical, signature) {
            return Decision::Reject(Rejection::unauthorized(
                Reason::BadSignature,
                "bad request signature",
            ));
        }

        // Signature BEFORE nonce, deliberately: spending the nonce first would
        // let anyone burn a victim's future nonce with a forged request and lock
        // out the legitimate one.
        if !nonces.check_and_store(nonce, now) {
            return Decision::Reject(Rejection::unauthorized(
                Reason::Replay,
                "replayed request (nonce already used) — retry with a fresh nonce",
            ));
        }

        Decision::Allow(Authenticated {
            token_id: record.token_id.clone(),
            auth_version: record.auth_version,
        })
    }
}

/// Find the unexpired v1 record whose token hash matches.
///
/// Every v1 record is compared, without an early exit, so a present-but-wrong
/// token cannot be told from an absent one by timing. v2 records are skipped
/// entirely: a v2 device must not be able to authenticate with a bearer token.
fn find_by_secret<'a>(devices: &'a DevicesDoc, secret: &str, now: f64) -> Option<&'a DeviceRecord> {
    let candidate = hash_secret(secret);
    let mut found: Option<&DeviceRecord> = None;
    for record in &devices.devices {
        let Some(stored) = record.token_hash.as_deref() else {
            continue;
        };
        let matches: bool =
            stored.len() == candidate.len() && stored.as_bytes().ct_eq(candidate.as_bytes()).into();
        if matches {
            found = Some(record);
        }
    }
    let record = found?;
    if record.expires_at.is_some_and(|e| e < now) {
        return None;
    }
    Some(record)
}

/// Find the unexpired v2 record with this key id.
///
/// A plain lookup is fine: the key id travels in the clear and only *names* the
/// device. Possession of the private key, proven by the signature, is what
/// authorises.
fn find_by_key_id<'a>(devices: &'a DevicesDoc, key_id: &str, now: f64) -> Option<&'a DeviceRecord> {
    let record = devices
        .devices
        .iter()
        .find(|r| r.token_id == key_id && r.auth_version == 2)?;
    if record.expires_at.is_some_and(|e| e < now) {
        return None;
    }
    Some(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};

    const NOW: f64 = 1_787_488_496.0;

    fn v1_record(id: &str, secret: &str, expires: Option<f64>) -> DeviceRecord {
        DeviceRecord {
            token_id: id.into(),
            label: "test".into(),
            auth_version: 1,
            token_hash: Some(hash_secret(secret)),
            public_key: None,
            created_at: 1.0,
            expires_at: expires,
            last_used: None,
        }
    }

    fn v2_record(id: &str, key: &SigningKey, expires: Option<f64>) -> DeviceRecord {
        DeviceRecord {
            token_id: id.into(),
            label: "test".into(),
            auth_version: 2,
            token_hash: None,
            public_key: Some(
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(key.verifying_key().to_bytes()),
            ),
            created_at: 1.0,
            expires_at: expires,
            last_used: None,
        }
    }

    fn signed<'a>(
        key: &SigningKey,
        key_id: &'a str,
        timestamp: &'a str,
        nonce: &'a str,
        sig_holder: &'a mut String,
    ) -> Presented<'a> {
        let canonical = signing::build_canonical("GET", "/api/units", timestamp, nonce, b"");
        *sig_holder = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(key.sign(&canonical).to_bytes());
        Presented {
            auth_version: Some("2"),
            key_id: Some(key_id),
            timestamp: Some(timestamp),
            nonce: Some(nonce),
            signature: Some(sig_holder),
            method: "GET",
            path: "/api/units",
            body: b"",
            ..Default::default()
        }
    }

    // -- API key ------------------------------------------------------------

    #[test]
    fn the_api_key_must_match_exactly() {
        let v = Verifier::new(1);
        assert!(v.verify_api_key(Some("secret"), Some("secret")).is_none());
        assert_eq!(
            v.verify_api_key(Some("wrong"), Some("secret"))
                .unwrap()
                .reason,
            Reason::BadApiKey
        );
        assert_eq!(
            v.verify_api_key(None, Some("secret")).unwrap().reason,
            Reason::NoCredential
        );
        // A prefix must not pass, which a naive comparison could allow.
        assert!(v.verify_api_key(Some("sec"), Some("secret")).is_some());
        assert!(v.verify_api_key(Some("secretx"), Some("secret")).is_some());
    }

    #[test]
    fn an_unconfigured_api_key_refuses_everyone() {
        // An empty configured key must not mean "anything goes", which is what a
        // bare constant-time compare of two empty strings would give.
        let v = Verifier::new(1);
        assert!(v.verify_api_key(Some(""), None).is_some());
        assert!(v.verify_api_key(Some(""), Some("")).is_some());
        assert!(v.verify_api_key(Some("guess"), Some("")).is_some());
    }

    // -- v1 -----------------------------------------------------------------

    #[test]
    fn a_valid_bearer_token_is_accepted() {
        let devices = DevicesDoc {
            devices: vec![v1_record("dev1", "s3cret", None)],
        };
        let v = Verifier::new(1);
        let mut nonces = NonceCache::default();
        let p = Presented {
            bearer: Some("s3cret"),
            method: "GET",
            path: "/api/units",
            ..Default::default()
        };
        assert_eq!(
            v.verify_device(&devices, &p, &mut nonces, NOW),
            Decision::Allow(Authenticated {
                token_id: "dev1".into(),
                auth_version: 1
            })
        );
    }

    #[test]
    fn a_wrong_or_missing_token_is_distinguishable_in_the_reason() {
        let devices = DevicesDoc {
            devices: vec![v1_record("dev1", "s3cret", None)],
        };
        let v = Verifier::new(1);
        let mut nonces = NonceCache::default();

        let wrong = Presented {
            bearer: Some("nope"),
            ..Default::default()
        };
        match v.verify_device(&devices, &wrong, &mut nonces, NOW) {
            Decision::Reject(r) => assert_eq!(r.reason, Reason::UnknownKey),
            other => panic!("{other:?}"),
        }
        let absent = Presented::default();
        match v.verify_device(&devices, &absent, &mut nonces, NOW) {
            Decision::Reject(r) => assert_eq!(r.reason, Reason::NoCredential),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_expired_token_is_refused() {
        let devices = DevicesDoc {
            devices: vec![v1_record("dev1", "s3cret", Some(NOW - 1.0))],
        };
        let v = Verifier::new(1);
        let mut nonces = NonceCache::default();
        let p = Presented {
            bearer: Some("s3cret"),
            ..Default::default()
        };
        assert!(matches!(
            v.verify_device(&devices, &p, &mut nonces, NOW),
            Decision::Reject(_)
        ));
    }

    #[test]
    fn a_v2_device_cannot_authenticate_with_a_bearer_token() {
        // No silent downgrade: a v2 record has no token_hash to match.
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let devices = DevicesDoc {
            devices: vec![v2_record("dev2", &key, None)],
        };
        let v = Verifier::new(1);
        let mut nonces = NonceCache::default();
        let p = Presented {
            bearer: Some("anything"),
            ..Default::default()
        };
        assert!(matches!(
            v.verify_device(&devices, &p, &mut nonces, NOW),
            Decision::Reject(_)
        ));
    }

    #[test]
    fn a_v1_device_below_the_floor_is_told_to_upgrade_not_refused() {
        let devices = DevicesDoc {
            devices: vec![v1_record("dev1", "s3cret", None)],
        };
        let v = Verifier::new(2);
        let mut nonces = NonceCache::default();
        let p = Presented {
            bearer: Some("s3cret"),
            ..Default::default()
        };
        assert_eq!(
            v.verify_device(&devices, &p, &mut nonces, NOW),
            Decision::UpgradeRequired {
                min_auth_version: 2
            }
        );
    }

    // -- v2 -----------------------------------------------------------------

    #[test]
    fn a_correctly_signed_request_is_accepted_once() {
        let key = SigningKey::from_bytes(&[5u8; 32]);
        let devices = DevicesDoc {
            devices: vec![v2_record("dev2", &key, None)],
        };
        let v = Verifier::new(1);
        let mut nonces = NonceCache::default();
        let mut sig = String::new();
        let p = signed(&key, "dev2", "1787488496", "nonce-1", &mut sig);

        assert_eq!(
            v.verify_device(&devices, &p, &mut nonces, NOW),
            Decision::Allow(Authenticated {
                token_id: "dev2".into(),
                auth_version: 2
            })
        );
        // The very same request again is a replay.
        match v.verify_device(&devices, &p, &mut nonces, NOW) {
            Decision::Reject(r) => {
                assert_eq!(r.reason, Reason::Replay);
                assert!(
                    r.reason.is_retryable(),
                    "a client should retry with a fresh nonce"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_drifted_clock_gets_our_time_back_and_is_retryable() {
        let key = SigningKey::from_bytes(&[5u8; 32]);
        let devices = DevicesDoc {
            devices: vec![v2_record("dev2", &key, None)],
        };
        let v = Verifier::new(1);
        let mut nonces = NonceCache::default();
        let mut sig = String::new();
        // Two minutes fast: outside the 60s window.
        let p = signed(&key, "dev2", "1787488616", "nonce-1", &mut sig);
        match v.verify_device(&devices, &p, &mut nonces, NOW) {
            Decision::Reject(r) => {
                assert_eq!(r.reason, Reason::ClockSkew);
                assert!(r.reason.is_retryable());
                let skew = r.skew.expect("must report the server clock");
                assert_eq!(skew.server_time, NOW);
                assert_eq!(skew.client_time, "1787488616");
                assert_eq!(skew.max_skew_seconds, 60);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_forged_signature_does_not_burn_the_victims_nonce() {
        // The ordering that matters: verify, then spend. Otherwise an attacker
        // could pre-consume a nonce the real client is about to use.
        let key = SigningKey::from_bytes(&[5u8; 32]);
        let devices = DevicesDoc {
            devices: vec![v2_record("dev2", &key, None)],
        };
        let v = Verifier::new(1);
        let mut nonces = NonceCache::default();

        let forged = Presented {
            auth_version: Some("2"),
            key_id: Some("dev2"),
            timestamp: Some("1787488496"),
            nonce: Some("nonce-1"),
            signature: Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
            method: "GET",
            path: "/api/units",
            body: b"",
            ..Default::default()
        };
        match v.verify_device(&devices, &forged, &mut nonces, NOW) {
            Decision::Reject(r) => assert_eq!(r.reason, Reason::BadSignature),
            other => panic!("{other:?}"),
        }
        assert!(
            nonces.is_empty(),
            "a rejected request must not spend the nonce"
        );

        // The legitimate client can still use it.
        let mut sig = String::new();
        let genuine = signed(&key, "dev2", "1787488496", "nonce-1", &mut sig);
        assert!(matches!(
            v.verify_device(&devices, &genuine, &mut nonces, NOW),
            Decision::Allow(_)
        ));
    }

    #[test]
    fn incomplete_signature_headers_are_their_own_retryable_reason() {
        let key = SigningKey::from_bytes(&[5u8; 32]);
        let devices = DevicesDoc {
            devices: vec![v2_record("dev2", &key, None)],
        };
        let v = Verifier::new(1);
        let mut nonces = NonceCache::default();
        for p in [
            Presented {
                auth_version: Some("2"),
                ..Default::default()
            },
            Presented {
                auth_version: Some("2"),
                key_id: Some("dev2"),
                ..Default::default()
            },
            Presented {
                auth_version: Some("2"),
                key_id: Some("dev2"),
                timestamp: Some("1787488496"),
                nonce: Some(""),
                signature: Some("x"),
                ..Default::default()
            },
        ] {
            match v.verify_device(&devices, &p, &mut nonces, NOW) {
                Decision::Reject(r) => assert_eq!(r.reason, Reason::IncompleteSignature),
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn a_signature_for_another_path_is_refused() {
        let key = SigningKey::from_bytes(&[5u8; 32]);
        let devices = DevicesDoc {
            devices: vec![v2_record("dev2", &key, None)],
        };
        let v = Verifier::new(1);
        let mut nonces = NonceCache::default();
        let mut sig = String::new();
        let mut p = signed(&key, "dev2", "1787488496", "nonce-1", &mut sig);
        // Same signature, different target.
        p.path = "/api/units/1/control";
        match v.verify_device(&devices, &p, &mut nonces, NOW) {
            Decision::Reject(r) => assert_eq!(r.reason, Reason::BadSignature),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_v1_device_cannot_present_a_signature() {
        let devices = DevicesDoc {
            devices: vec![v1_record("dev1", "s3cret", None)],
        };
        let v = Verifier::new(1);
        let mut nonces = NonceCache::default();
        let key = SigningKey::from_bytes(&[5u8; 32]);
        let mut sig = String::new();
        let p = signed(&key, "dev1", "1787488496", "nonce-1", &mut sig);
        match v.verify_device(&devices, &p, &mut nonces, NOW) {
            Decision::Reject(r) => assert_eq!(r.reason, Reason::UnknownKey),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bearer_headers_are_parsed_the_way_clients_send_them() {
        assert_eq!(bearer_from_header("Bearer abc123"), Some("abc123"));
        assert_eq!(bearer_from_header("bearer abc123"), Some("abc123"));
        assert_eq!(bearer_from_header("BEARER abc123"), Some("abc123"));
        assert_eq!(bearer_from_header("Bearer  abc123 "), Some("abc123"));
        assert_eq!(bearer_from_header("Basic abc123"), None);
        assert_eq!(bearer_from_header("Bearer"), None);
        assert_eq!(bearer_from_header("Bearer "), None);
        assert_eq!(bearer_from_header(""), None);
    }

    #[test]
    fn hashing_matches_pythons_sha256_hexdigest() {
        // sha256("s3cret") -- so a devices.json written by the Python server is
        // matched by this implementation.
        assert_eq!(
            hash_secret("s3cret"),
            "1ec1c26b50d5d3c58d9583181af8076655fe00756bf7285940ba3670f99fcba0"
        );
    }
}
