//! The CLI's HTTP client.
//!
//! Talks to the server over its own REST API rather than reaching into the
//! stores directly. That is deliberate: `breeze-core control` then works
//! identically whether it is run on the server or from a laptop, it honours the
//! same authentication as every other client, and it cannot corrupt a store the
//! running service has open.

use std::time::Duration;

/// Generous, because a request that touches a unit waits on an air conditioner:
/// ~700 ms each, and a batch fans out but a cold connection pays a handshake.
const TIMEOUT: Duration = Duration::from_secs(60);

pub struct Client {
    base_url: String,
    api_key: String,
    device_token: Option<String>,
}

impl Client {
    pub fn new(base_url: String, api_key: String, device_token: Option<String>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            device_token,
        }
    }

    pub fn from_profile(profile: &super::profile::Profile) -> Self {
        Self::new(
            profile.base_url.clone(),
            profile.api_key.clone(),
            Some(profile.device_token.clone()),
        )
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn get(&self, path: &str) -> Result<serde_json::Value, String> {
        self.send("GET", path, None)
    }

    pub fn post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.send("POST", path, Some(body))
    }

    fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let url = format!("{}{path}", self.base_url);
        let mut request = ureq::request(method, &url)
            .timeout(TIMEOUT)
            .set("X-API-Key", &self.api_key);
        if let Some(token) = &self.device_token {
            request = request.set("Authorization", &format!("Bearer {token}"));
        }

        let response = match body {
            Some(json) => request.send_json(json),
            None => request.call(),
        };

        match response {
            Ok(r) => {
                let text = r.into_string().map_err(|e| e.to_string())?;
                if text.trim().is_empty() {
                    // 204, which several routes answer with.
                    return Ok(serde_json::Value::Null);
                }
                serde_json::from_str(&text).map_err(|e| format!("unreadable response: {e}"))
            }
            // The body of an error carries the reason, and the reason is the
            // whole point of showing it to a person.
            Err(ureq::Error::Status(status, r)) => {
                let text = r.into_string().unwrap_or_default();
                Err(explain(status, &text))
            }
            Err(e) => Err(format!("{e}")),
        }
    }
}

/// Turn a failed response into something worth reading.
fn explain(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            // A plain HTTPException is a string; an auth rejection is an object
            // with its own detail inside.
            v.get("detail").map(|d| match d.as_str() {
                Some(text) => text.to_string(),
                None => d
                    .get("detail")
                    .and_then(|inner| inner.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| d.to_string()),
            })
        })
        .unwrap_or_else(|| body.chars().take(200).collect());

    match status {
        401 => format!("{detail}\n  (run `breeze-core login` to enrol this machine again)"),
        403 => format!("{detail}\n  (that action has to come from the local network)"),
        404 => detail,
        503 => {
            format!("{detail}\n  (the unit did not answer -- is it powered and on the network?)")
        }
        _ => format!("{detail} [HTTP {status}]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trailing_slash_on_the_base_url_does_not_double_up() {
        let client = Client::new("http://x:8420/".into(), "k".into(), None);
        assert_eq!(client.base_url(), "http://x:8420");
    }

    #[test]
    fn a_plain_detail_is_shown_as_written() {
        assert_eq!(
            explain(404, r#"{"detail":"Unknown unit '999'"}"#),
            "Unknown unit '999'"
        );
    }

    #[test]
    fn a_nested_auth_rejection_is_unwrapped() {
        // The auth layer answers with an object, not a string, and the useful
        // sentence is inside it.
        let message = explain(
            401,
            r#"{"detail":{"error":"no_credential","detail":"missing or invalid X-API-Key header","retryable":false}}"#,
        );
        assert!(
            message.contains("missing or invalid X-API-Key header"),
            "{message}"
        );
        assert!(
            message.contains("breeze-core login"),
            "it should say what to do"
        );
    }

    #[test]
    fn a_503_explains_what_it_usually_means() {
        let message = explain(503, r#"{"detail":"couldn't reach that unit"}"#);
        assert!(message.contains("did not answer"));
    }

    #[test]
    fn a_body_that_is_not_json_is_still_shown() {
        let message = explain(500, "<html>Internal Server Error</html>");
        assert!(message.contains("Internal Server Error"), "{message}");
    }

    #[test]
    fn an_enormous_body_is_truncated_rather_than_flooding_the_terminal() {
        let message = explain(500, &"x".repeat(10_000));
        assert!(message.len() < 300, "got {} chars", message.len());
    }
}
