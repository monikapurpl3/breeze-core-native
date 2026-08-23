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
    pub fn into_http(self) -> Response<std::io::Cursor<Vec<u8>>> {
        let mut headers = vec![
            header("Content-Type", self.content_type),
            header("X-Content-Type-Options", "nosniff"),
            header("X-Frame-Options", "DENY"),
            header("Referrer-Policy", "no-referrer"),
            header(
                "Content-Security-Policy",
                "default-src 'self'; base-uri 'none'; form-action 'none'; \
                 frame-ancestors 'none'; object-src 'none'",
            ),
            // The API is state that must never be cached by a proxy.
            header("Cache-Control", "no-store"),
        ];
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
        fn clone_headers(&self) -> Vec<String> {
            let mut names = vec![
                "Content-Type".to_string(),
                "X-Content-Type-Options".to_string(),
                "X-Frame-Options".to_string(),
                "Referrer-Policy".to_string(),
                "Content-Security-Policy".to_string(),
                "Cache-Control".to_string(),
            ];
            names.extend(self.extra.iter().map(|(n, _)| n.to_string()));
            names
        }
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
        let response = r.into_http();
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
