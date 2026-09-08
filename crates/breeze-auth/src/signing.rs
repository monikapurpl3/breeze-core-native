//! Auth v2: Ed25519 request signing with a SHA3-512 body digest.
//!
//! The client holds an Ed25519 private key that never leaves it; the server
//! stores only the public half. So unlike the v1 bearer scheme — where we store
//! a hash of a secret the client also holds — a leak of `devices.json` yields
//! nothing forgeable.
//!
//! Every request is signed over a canonical string:
//!
//! ```text
//! breeze-auth-v2\n{METHOD}\n{path?query}\n{timestamp}\n{nonce}\n{sha3_512(body) hex}
//! ```
//!
//! The timestamp bounds replay to a window, the nonce makes each request
//! single-use inside it, and the body digest makes tampering fail. Three clients
//! already reproduce this construction byte for byte — the Android app, the web
//! panel, and the diagnostic CLI — so it is fixed, not ours to improve.

use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use sha3::Digest as _;

/// Domain separation, so a v2 signature can never be mistaken for any other
/// signed blob this project might introduce.
pub const CANONICAL_PREFIX: &str = "breeze-auth-v2";

/// Ed25519 public keys are 32 raw bytes.
pub const PUBLIC_KEY_LEN: usize = 32;

/// How far a client's clock may drift, each way, and how long a nonce is
/// remembered.
pub const DEFAULT_SKEW_SECONDS: u64 = 60;

/// Decode URL-safe base64 that may have lost its padding in transit.
///
/// Clients differ: the app sends unpadded, the panel's WebCrypto output is
/// unpadded, and a hand-rolled curl invocation might pad. Python's
/// `urlsafe_b64decode` is fed re-padded input for the same reason.
pub fn b64_decode(value: &str) -> Option<Vec<u8>> {
    let trimmed = value.trim_end_matches('=');
    // BOTH alphabets, because the reference accepts both and clients rely on it.
    //
    // Python's `base64.urlsafe_b64decode` translates - and _ into + and /, then
    // hands the result to the STANDARD decoder -- which leaves any + and / that
    // were already there perfectly valid. So the reference accepts standard
    // base64 as well as base64url, and a client emitting either has always
    // worked.
    //
    // Decoding URL_SAFE_NO_PAD only, as this did, rejects + and / outright. The
    // web panel emits base64url so it was unaffected, but any client using
    // standard base64 broke -- and broke *intermittently*, which is far worse
    // than breaking outright: a signature is 64 random bytes, so it encodes
    // without a + or / only about 7% of the time. One request in fourteen
    // succeeded. The symptom was an air conditioner that changed mode once and
    // then ignored everything, with a 401 nobody was logging.
    let normalised: String = trimmed
        .chars()
        .map(|c| match c {
            '-' => '+',
            '_' => '/',
            other => other,
        })
        .collect();
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(normalised)
        .ok()
}

/// Hex SHA3-512 of the request body — the digest that goes in the canonical
/// string.
///
/// Note this is SHA-3, not SHA-2. WebCrypto has no SHA-3 at all, which is why
/// the web panel ships a hand-written Keccak; getting this wrong means the panel
/// and the server disagree about every request.
pub fn sha3_512_hex(data: &[u8]) -> String {
    let digest = sha3::Sha3_512::digest(data);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use core::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Assemble the exact bytes that get signed.
///
/// `path` must already include the query string, because that is what the
/// clients sign — a signature over the path alone would let a query be swapped.
pub fn build_canonical(
    method: &str,
    path: &str,
    timestamp: &str,
    nonce: &str,
    body: &[u8],
) -> Vec<u8> {
    format!(
        "{CANONICAL_PREFIX}\n{}\n{path}\n{timestamp}\n{nonce}\n{}",
        method.to_uppercase(),
        sha3_512_hex(body)
    )
    .into_bytes()
}

/// Whether a base64 string is a usable Ed25519 public key.
///
/// Checked once at enrolment rather than on every request, so a malformed key is
/// refused while someone is watching.
pub fn public_key_is_valid(public_key_b64: &str) -> bool {
    parse_public_key(public_key_b64).is_some()
}

fn parse_public_key(public_key_b64: &str) -> Option<VerifyingKey> {
    let raw = b64_decode(public_key_b64)?;
    let bytes: [u8; PUBLIC_KEY_LEN] = raw.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

/// Verify a signature over `canonical`. Never panics; any malformed input is a
/// rejection.
///
/// Uses `verify_strict`, which additionally refuses small-order keys and
/// non-canonical signature encodings. No honest client produces either — the
/// app, the panel's WebCrypto and the CLI all emit canonical signatures — so
/// this costs nothing and closes off signature-malleability tricks.
pub fn verify_signature(public_key_b64: &str, canonical: &[u8], signature_b64: &str) -> bool {
    let Some(key) = parse_public_key(public_key_b64) else {
        return false;
    };
    let Some(sig_bytes) = b64_decode(signature_b64) else {
        return false;
    };
    let Ok(sig_array) = <[u8; 64]>::try_from(sig_bytes.as_slice()) else {
        return false;
    };
    key.verify_strict(canonical, &Signature::from_bytes(&sig_array))
        .is_ok()
}

/// Seconds since the epoch, as the protocol's timestamps are expressed.
pub fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both base64 alphabets decode, padded or not.
    ///
    /// Two vectors, because no single one exercises every interesting
    /// character: 0..64 encodes with a '+' and 0xFF repeated encodes with a '/'
    /// (and their url-safe counterparts '-' and '_'). Generated rather than
    /// typed, so they cannot be subtly wrong in a test whose whole job is
    /// catching a subtly wrong encoding.
    #[test]
    fn both_base64_alphabets_are_accepted() {
        for raw in [(0u8..64).collect::<Vec<u8>>(), vec![0xFFu8; 33]] {
            let standard = base64::engine::general_purpose::STANDARD.encode(&raw);
            let url_safe = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&raw);
            assert!(
                standard.contains('+') || standard.contains('/'),
                "vector does not exercise the standard alphabet: {standard}"
            );
            assert!(
                url_safe.contains('-') || url_safe.contains('_'),
                "vector does not exercise the url-safe alphabet: {url_safe}"
            );
            for (label, value) in [
                ("standard, padded", standard.clone()),
                ("standard, unpadded", standard.trim_end_matches('=').to_string()),
                ("url-safe, unpadded", url_safe.clone()),
                ("url-safe, padded", format!("{url_safe}==")),
            ] {
                assert_eq!(b64_decode(&value).as_deref(), Some(&raw[..]), "{label}");
            }
        }
    }


    #[test]
    fn the_empty_body_digest_is_the_known_sha3_512_value() {
        // A published constant, so this catches SHA-2 being used by mistake --
        // the two are the same length and would otherwise look plausible.
        assert_eq!(
            sha3_512_hex(b""),
            "a69f73cca23a9ac5c8b567dc185a756e97c982164fe25859e0d1dcc1475c80a6\
             15b2123af1f5f94c11e3e9402c3ac558f500199d95b6d3e301758586281dcd26"
        );
    }

    #[test]
    fn a_query_string_is_part_of_what_is_signed() {
        let a = build_canonical("GET", "/api/units", "1", "n", b"");
        let b = build_canonical("GET", "/api/units?all=1", "1", "n", b"");
        assert_ne!(a, b, "swapping the query must invalidate the signature");
    }

    #[test]
    fn changing_the_body_changes_the_canonical_string() {
        let a = build_canonical(
            "POST",
            "/api/units/1/control",
            "1",
            "n",
            br#"{"power":true}"#,
        );
        let b = build_canonical(
            "POST",
            "/api/units/1/control",
            "1",
            "n",
            br#"{"power":false}"#,
        );
        assert_ne!(a, b, "the body digest must bind the payload");
    }

    #[test]
    fn base64_decodes_with_or_without_padding() {
        // "hello" -> aGVsbG8
        assert_eq!(b64_decode("aGVsbG8").unwrap(), b"hello");
        assert_eq!(b64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(b64_decode("aGVsbG8==").unwrap(), b"hello");
    }

    /// Deliberately the OPPOSITE of what this once asserted.
    ///
    /// It used to require the url-safe alphabet and refuse the standard one, on
    /// the reasoning that two encodings of one key should not both be valid.
    /// That reasoning is defensible in isolation and wrong here: the reference
    /// accepts both, so refusing one breaks clients that the reference served
    /// happily -- and it breaks them one request in fourteen, which is how it
    /// survived every test and reached a real deployment.
    ///
    /// The "two encodings" worry costs nothing in practice: devices are looked
    /// up by key id, never by key string, and both encodings decode to the same
    /// bytes, so a signature verifies identically either way.
    #[test]
    fn both_alphabets_decode_to_the_same_bytes() {
        let url_safe = "____________________________________________";
        let standard = "////////////////////////////////////////////";
        assert_eq!(b64_decode(url_safe), b64_decode(standard));
        assert_eq!(b64_decode(url_safe).unwrap(), vec![0xFFu8; 33]);
    }

    #[test]
    fn malformed_keys_and_signatures_are_refused_not_fatal() {
        for key in ["", "!!!!", "aGVsbG8", "AAAA"] {
            assert!(!public_key_is_valid(key), "{key:?} should be invalid");
            assert!(!verify_signature(key, b"x", "AAAA"));
        }
        // A well-formed key with a nonsense signature.
        let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0u8; 32]);
        assert!(!verify_signature(&key, b"x", "not-base64!!"));
        assert!(!verify_signature(&key, b"x", "aGVsbG8"), "wrong length");
    }

    #[test]
    fn a_real_signature_verifies_and_a_tampered_one_does_not() {
        use ed25519_dalek::{Signer, SigningKey};
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let public = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(signing.verifying_key().to_bytes());
        assert!(public_key_is_valid(&public));

        let canonical = build_canonical("POST", "/api/timers", "1787488496", "nonce-1", b"{}");
        let sig = signing.sign(&canonical);
        let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig.to_bytes());
        assert!(verify_signature(&public, &canonical, &sig_b64));

        // Same signature, different request.
        let other = build_canonical("POST", "/api/timers", "1787488497", "nonce-1", b"{}");
        assert!(!verify_signature(&public, &other, &sig_b64));

        // Right request, wrong key.
        let other_key = SigningKey::from_bytes(&[8u8; 32]);
        let other_public = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(other_key.verifying_key().to_bytes());
        assert!(!verify_signature(&other_public, &canonical, &sig_b64));
    }

    #[test]
    fn an_all_zero_key_is_rejected_as_small_order() {
        // verify_strict refuses these; a client cannot legitimately have one.
        use ed25519_dalek::{Signer, SigningKey};
        let signing = SigningKey::from_bytes(&[1u8; 32]);
        let canonical = build_canonical("GET", "/", "1", "n", b"");
        let sig = signing.sign(&canonical);
        let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig.to_bytes());
        let zero = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0u8; 32]);
        assert!(!verify_signature(&zero, &canonical, &sig_b64));
    }
}
