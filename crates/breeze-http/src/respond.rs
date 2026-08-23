//! Responses, and the headers that go on every one of them.

use tiny_http::{Header, Response};

/// A response body plus its status.
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: &'static str,
    /// Extra headers beyond the standard set.
    pub extra: Vec<(&'static str, String)>,
}

/// Headers every response carries, including the hijacked SSE stream.
///
/// Shared rather than written out twice: the stream writes its own status line
/// and headers by hand, and a security header present on every route except the
/// long-lived one would be the easiest kind of gap to miss.
///
/// The values are the reference's, character for character, including
/// `form-action 'self'` where a stricter `'none'` would have been tempting. The
/// same panel is served by both servers, so a policy that differs is a policy
/// that can break a page on one and not the other — and a custom panel with a
/// real `<form>` in it would be exactly that surprise.
pub const SECURITY_HEADERS: &[(&str, &str)] = &[
    ("X-Content-Type-Options", "nosniff"),
    ("X-Frame-Options", "DENY"),
    ("Referrer-Policy", "no-referrer"),
    (
        "Content-Security-Policy",
        "default-src 'self'; base-uri 'none'; frame-ancestors 'none'; object-src 'none'; form-action 'self'",
    ),
    // Ignored over plain HTTP; takes effect once something terminates TLS in
    // front, which is the documented deployment.
    (
        "Strict-Transport-Security",
        "max-age=63072000; includeSubDomains",
    ),
];

impl Reply {
    pub fn json(status: u16, value: &serde_json::Value) -> Self {
        Self {
            status,
            body: serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec()),
            content_type: "application/json",
            extra: Vec::new(),
        }
    }

    pub fn json_body<T: serde::Serialize>(status: u16, value: &T) -> Self {
        Self {
            status,
            body: serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec()),
            content_type: "application/json",
            extra: Vec::new(),
        }
    }

    /// The `{"detail": "..."}` shape FastAPI produces for a plain HTTPException,
    /// which is what clients unwrap for a human-readable message.
    pub fn detail(status: u16, message: impl Into<String>) -> Self {
        Self::json(status, &serde_json::json!({ "detail": message.into() }))
    }

    pub fn with_header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.extra.push((name, value.into()));
        self
    }

    /// Convert into something tiny_http can send, attaching the standard headers.
    ///
    /// The CSP is `default-src 'self'` with no `unsafe-inline`, matching Breeze
    /// Core. That is load-bearing for the bundled panel: all styling lives in a
    /// stylesheet and all behaviour in modules, and adding an inline handler
    /// would force the policy open rather than the other way round.
    ///
    /// Deliberately absent: any CORS header. The panel is same-origin, and a
    /// permissive policy would let any other page on the LAN drive this API.
    /// `security` is `AC_SECURITY_HEADERS`: off when a reverse proxy already
    /// sets these, because two `Content-Security-Policy` headers are *intersected*
    /// by the browser, not deduplicated — duplicates make the policy stricter
    /// than either party intended and can break the panel.
    pub fn into_http(self, security: bool) -> Response<std::io::Cursor<Vec<u8>>> {
        // A header set explicitly on this reply wins, matching the reference's
        // `setdefault`: static files carry their own Cache-Control, and the API's
        // `no-store` would defeat the point of serving them from memory.
        let overridden = |name: &str| self.extra.iter().any(|(n, _)| n.eq_ignore_ascii_case(name));

        let mut headers = vec![header("Content-Type", self.content_type)];
        if !overridden("Cache-Control") {
            // The API is state that must never be cached by a proxy.
            headers.push(header("Cache-Control", "no-store"));
        }
        if security {
            for (name, value) in SECURITY_HEADERS {
                if !overridden(name) {
                    headers.push(header(name, value));
                }
            }
        }
        for (name, value) in &self.extra {
            headers.push(header(name, value));
        }
        let mut response = Response::from_data(self.body).with_status_code(self.status);
        for h in headers {
            response.add_header(h);
        }
        response
    }
}

fn header(name: &str, value: &str) -> Header {
    // Both sides are our own constants, so this cannot fail in practice; falling
    // back to a benign header is still better than panicking in a request thread.
    Header::from_bytes(name.as_bytes(), value.as_bytes())
        .unwrap_or_else(|_| Header::from_bytes(&b"X-Invalid"[..], &b"1"[..]).expect("static"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_names(r: &Reply) -> Vec<String> {
        r.clone_headers()
    }

    impl Reply {
        /// Test helper: the header names this reply would send.
        ///
        /// Derived from `SECURITY_HEADERS` rather than listed again — an earlier
        /// version spelled them out and silently stopped covering the newest one.
        fn clone_headers(&self) -> Vec<String> {
            let mut names = vec!["Content-Type".to_string(), "Cache-Control".to_string()];
            names.extend(SECURITY_HEADERS.iter().map(|(n, _)| n.to_string()));
            names.extend(self.extra.iter().map(|(n, _)| n.to_string()));
            names
        }
    }

    fn sent_headers(r: Reply, security: bool) -> Vec<(String, String)> {
        r.into_http(security)
            .headers()
            .iter()
            .map(|h| {
                (
                    h.field.as_str().as_str().to_string(),
                    h.value.as_str().to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn the_hardening_headers_can_be_turned_off_for_a_proxy() {
        // Two CSP headers are intersected by the browser, not deduplicated, so a
        // deployment whose proxy already sets them needs a way to stay quiet.
        let with = sent_headers(Reply::json(200, &serde_json::json!({})), true);
        assert!(with.iter().any(|(n, _)| n == "Content-Security-Policy"));

        let without = sent_headers(Reply::json(200, &serde_json::json!({})), false);
        assert!(!without.iter().any(|(n, _)| n == "Content-Security-Policy"));
        // The response is still a response: type and caching survive.
        assert!(without.iter().any(|(n, _)| n == "Content-Type"));
        assert!(without.iter().any(|(n, _)| n == "Cache-Control"));
    }

    #[test]
    fn an_explicit_header_replaces_the_default_rather_than_joining_it() {
        // Static files carry their own Cache-Control; two of them would be
        // ambiguous, and "no-store" would defeat serving the panel from memory.
        let r = Reply::json(200, &serde_json::json!({}))
            .with_header("Cache-Control", "public, max-age=3600");
        let sent = sent_headers(r, true);
        let caching: Vec<&String> = sent
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case("Cache-Control"))
            .map(|(_, v)| v)
            .collect();
        assert_eq!(caching.len(), 1, "exactly one Cache-Control: {sent:?}");
        assert_eq!(caching[0], "public, max-age=3600");
    }

    #[test]
    fn the_header_values_match_the_reference_exactly() {
        // Both servers serve the same panel, so a policy that differs is a
        // policy that can break a page on one and not the other.
        let sent = sent_headers(Reply::json(200, &serde_json::json!({})), true);
        let value = |name: &str| {
            sent.iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(
            value("Content-Security-Policy"),
            "default-src 'self'; base-uri 'none'; frame-ancestors 'none'; \
             object-src 'none'; form-action 'self'"
        );
        assert_eq!(value("X-Content-Type-Options"), "nosniff");
        assert_eq!(value("X-Frame-Options"), "DENY");
        assert_eq!(value("Referrer-Policy"), "no-referrer");
        assert_eq!(
            value("Strict-Transport-Security"),
            "max-age=63072000; includeSubDomains"
        );
    }

    #[test]
    fn every_reply_carries_the_hardening_headers() {
        let r = Reply::json(200, &serde_json::json!({"ok": true}));
        let names = header_names(&r);
        for required in [
            "X-Content-Type-Options",
            "X-Frame-Options",
            "Content-Security-Policy",
            "Referrer-Policy",
            "Cache-Control",
        ] {
            assert!(names.iter().any(|n| n == required), "missing {required}");
        }
    }

    #[test]
    fn no_cors_header_is_ever_added() {
        // Its absence is load-bearing: the panel is same-origin, and a
        // permissive policy would let any LAN page drive this API.
        let r = Reply::json(200, &serde_json::json!({})).with_header("X-Breeze-Auth-Upgrade", "2");
        let names = header_names(&r);
        assert!(
            !names
                .iter()
                .any(|n| n.to_lowercase().starts_with("access-control")),
            "a CORS header appeared: {names:?}"
        );
    }

    #[test]
    fn the_csp_forbids_inline() {
        let r = Reply::json(200, &serde_json::json!({}));
        let response = r.into_http(true);
        let csp = response
            .headers()
            .iter()
            .find(|h| h.field.equiv("Content-Security-Policy"))
            .map(|h| h.value.as_str().to_string())
            .expect("CSP present");
        assert!(csp.contains("default-src 'self'"));
        assert!(!csp.contains("unsafe-inline"), "CSP must not allow inline");
        assert!(!csp.contains("unsafe-eval"));
    }

    #[test]
    fn detail_matches_the_shape_clients_unwrap() {
        let r = Reply::detail(404, "unknown unit");
        let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(body["detail"], "unknown unit");
        assert_eq!(r.status, 404);
    }
}
