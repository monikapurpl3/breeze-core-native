//! Why a request was refused, in the exact shape clients already parse.
//!
//! These strings are a wire contract, not internal detail. Breeze Core 3.0.2
//! added them after a real lockout in which several users lost access for days
//! and nothing in the log said why: a phone whose clock had drifted past the
//! signature window produced a 401 indistinguishable from a revoked credential,
//! so the app concluded its key was dead, deleted it, and — with LAN-only
//! enrolment — stranded anyone away from home.
//!
//! The fix was to make failures *legible*: a stable `error` code, a
//! `retryable` flag, and for clock skew the server's own time so a client can
//! measure its offset and re-sign. The Android app branches on all three. Change
//! a string here and you reintroduce that outage.

use serde::Serialize;

/// The stable reason codes. Values match Breeze Core's `security/authlog.py`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    NoCredential,
    UnknownKey,
    Expired,
    BadSignature,
    BadApiKey,
    ClockSkew,
    Replay,
    IncompleteSignature,
}

impl Reason {
    pub fn code(self) -> &'static str {
        match self {
            Self::NoCredential => "no_credential",
            Self::UnknownKey => "unknown_key",
            Self::Expired => "expired",
            Self::BadSignature => "bad_signature",
            Self::BadApiKey => "bad_api_key",
            Self::ClockSkew => "clock_skew",
            Self::Replay => "replay",
            Self::IncompleteSignature => "incomplete_signature",
        }
    }

    /// Whether a client should try again rather than assume its credential is
    /// finished.
    ///
    /// Exactly the three from `authlog.RETRYABLE`. A drifted clock and a
    /// verbatim retry over a flaky mobile link are both *transient*: the
    /// credential is fine. Treating them as fatal is what caused the lockout.
    pub fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::ClockSkew | Self::Replay | Self::IncompleteSignature
        )
    }
}

/// Extra fields a `clock_skew` rejection carries so a client can self-heal.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SkewInfo {
    /// Our clock, rounded to milliseconds as Python does.
    pub server_time: f64,
    /// Whatever the client claimed, echoed verbatim.
    pub client_time: String,
    pub max_skew_seconds: u64,
}

/// A refusal, ready to become an HTTP response.
#[derive(Debug, Clone, PartialEq)]
pub struct Rejection {
    pub status: u16,
    pub reason: Reason,
    /// Human-readable, and deliberately so: older clients surface it verbatim.
    pub detail: String,
    pub skew: Option<SkewInfo>,
}

/// The response body, shaped as `{"detail": {...}}` because that is what
/// FastAPI's `HTTPException` produces and what clients unwrap.
#[derive(Debug, Clone, Serialize)]
pub struct RejectionBody {
    pub detail: RejectionDetail,
}

#[derive(Debug, Clone, Serialize)]
pub struct RejectionDetail {
    pub error: &'static str,
    pub detail: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_skew_seconds: Option<u64>,
}

impl Rejection {
    pub fn new(status: u16, reason: Reason, detail: impl Into<String>) -> Self {
        Self {
            status,
            reason,
            detail: detail.into(),
            skew: None,
        }
    }

    /// A 401 with the messages Breeze Core already sends.
    pub fn unauthorized(reason: Reason, detail: impl Into<String>) -> Self {
        Self::new(401, reason, detail)
    }

    pub fn with_skew(mut self, skew: SkewInfo) -> Self {
        self.skew = Some(skew);
        self
    }

    pub fn body(&self) -> RejectionBody {
        RejectionBody {
            detail: RejectionDetail {
                error: self.reason.code(),
                detail: self.detail.clone(),
                retryable: self.reason.is_retryable(),
                server_time: self.skew.as_ref().map(|s| s.server_time),
                client_time: self.skew.as_ref().map(|s| s.client_time.clone()),
                max_skew_seconds: self.skew.as_ref().map(|s| s.max_skew_seconds),
            },
        }
    }
}

/// The 426 a device below `min_auth_version` gets.
///
/// Separate from `Rejection` because its body has a different shape and it is
/// not an authentication *failure* — the credential is valid, the scheme is too
/// old. The message is what an un-updated client shows the user verbatim, which
/// is why it says "update Breeze" rather than naming a version number.
pub fn upgrade_required_body(min_auth_version: u8) -> serde_json::Value {
    serde_json::json!({
        "detail": {
            "error": "auth_upgrade_required",
            "min_auth_version": min_auth_version,
            "detail": "This version of the app uses an outdated, less secure \
                       connection. Please update Breeze to continue controlling \
                       your units."
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_the_python_constants() {
        // Copied from security/authlog.py. A typo here is a silently broken
        // client, since the app matches on these strings.
        for (reason, code) in [
            (Reason::NoCredential, "no_credential"),
            (Reason::UnknownKey, "unknown_key"),
            (Reason::Expired, "expired"),
            (Reason::BadSignature, "bad_signature"),
            (Reason::BadApiKey, "bad_api_key"),
            (Reason::ClockSkew, "clock_skew"),
            (Reason::Replay, "replay"),
            (Reason::IncompleteSignature, "incomplete_signature"),
        ] {
            assert_eq!(reason.code(), code);
        }
    }

    #[test]
    fn exactly_three_reasons_are_retryable() {
        // authlog.RETRYABLE = {CLOCK_SKEW, REPLAY, INCOMPLETE_SIGNATURE}
        let retryable: Vec<&str> = [
            Reason::NoCredential,
            Reason::UnknownKey,
            Reason::Expired,
            Reason::BadSignature,
            Reason::BadApiKey,
            Reason::ClockSkew,
            Reason::Replay,
            Reason::IncompleteSignature,
        ]
        .into_iter()
        .filter(|r| r.is_retryable())
        .map(|r| r.code())
        .collect();
        assert_eq!(retryable, ["clock_skew", "replay", "incomplete_signature"]);
    }

    #[test]
    fn a_dead_credential_is_never_retryable() {
        // The lockout hinged on this: retrying a revoked key forever is what
        // hammered the server and tripped fail2ban.
        assert!(!Reason::UnknownKey.is_retryable());
        assert!(!Reason::Expired.is_retryable());
        assert!(!Reason::BadSignature.is_retryable());
    }

    #[test]
    fn the_body_is_nested_under_detail() {
        let r = Rejection::unauthorized(Reason::BadApiKey, "missing or invalid X-API-Key header");
        let json = serde_json::to_value(r.body()).unwrap();
        assert_eq!(json["detail"]["error"], "bad_api_key");
        assert_eq!(json["detail"]["retryable"], false);
        assert_eq!(
            json["detail"]["detail"],
            "missing or invalid X-API-Key header"
        );
        // Skew fields must be absent, not null, when they do not apply.
        assert!(json["detail"].get("server_time").is_none());
    }

    #[test]
    fn a_skew_rejection_carries_the_server_clock() {
        let r = Rejection::unauthorized(Reason::ClockSkew, "check the device clock").with_skew(
            SkewInfo {
                server_time: 1787488496.123,
                client_time: "1787488000".into(),
                max_skew_seconds: 60,
            },
        );
        let json = serde_json::to_value(r.body()).unwrap();
        assert_eq!(json["detail"]["retryable"], true);
        assert_eq!(json["detail"]["server_time"], 1787488496.123);
        assert_eq!(json["detail"]["client_time"], "1787488000");
        assert_eq!(json["detail"]["max_skew_seconds"], 60);
    }

    #[test]
    fn the_upgrade_message_tells_a_user_what_to_do() {
        let body = upgrade_required_body(2);
        assert_eq!(body["detail"]["error"], "auth_upgrade_required");
        assert_eq!(body["detail"]["min_auth_version"], 2);
        let text = body["detail"]["detail"].as_str().unwrap();
        assert!(text.contains("update Breeze"), "got {text:?}");
        assert!(
            !text.contains("auth_version"),
            "must not leak jargon to users"
        );
    }
}
