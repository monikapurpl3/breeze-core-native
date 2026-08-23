//! The listener, the routing, and the authentication in front of it.

use std::sync::{mpsc, Arc, Mutex};

use breeze_auth::{bearer_from_header, Decision, Presented};
use breeze_store::ControlRequest;

use crate::respond::Reply;
use crate::state::AppState;
use crate::units;

/// Features this build actually implements.
///
/// Clients feature-detect on this list — the Android app hides its hourglass
/// when `sleep_timer` is absent, for instance. So it must describe reality, not
/// ambition: advertising `live_stream` before SSE exists would make every client
/// open a stream that never arrives. Entries get added as routes land.
const FEATURES: &[&str] = &["batch_state", "beep_control", "config_api", "ed25519_auth"];

const AUTH_VERSIONS: &[u8] = &[1, 2];

/// A request, decomposed into the parts routing and auth need.
struct Incoming {
    method: String,
    /// Path only, no query.
    path: String,
    /// Path plus query, which is what a v2 signature covers.
    signed_path: String,
    body: Vec<u8>,
    api_key: Option<String>,
    bearer: Option<String>,
    auth_version: Option<String>,
    key_id: Option<String>,
    timestamp: Option<String>,
    nonce: Option<String>,
    signature: Option<String>,
}

impl Incoming {
    fn read(request: &mut tiny_http::Request) -> Self {
        // Collect everything borrowed from the request first, as owned values.
        // The body has to be read through a *mutable* borrow, so no immutable
        // borrow of headers or url may still be alive at that point.
        let method = request.method().as_str().to_string();
        let url = request.url().to_string();
        let mut found: Vec<(String, String)> = Vec::new();
        for h in request.headers() {
            found.push((
                h.field.as_str().as_str().to_ascii_lowercase(),
                h.value.as_str().to_string(),
            ));
        }
        let header = |name: &str| {
            found
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };

        let path = url
            .split_once('?')
            .map(|(p, _)| p.to_string())
            .unwrap_or_else(|| url.clone());

        let api_key = header("x-api-key");
        let authorization = header("authorization");
        let auth_version = header("x-breeze-auth-version");
        let key_id = header("x-breeze-key-id");
        let timestamp = header("x-breeze-timestamp");
        let nonce = header("x-breeze-nonce");
        let signature = header("x-breeze-signature");

        // Now the mutable borrow. A v2 signature covers the body's digest, so
        // authentication cannot proceed without having read it.
        let mut body = Vec::new();
        let _ = std::io::Read::read_to_end(request.as_reader(), &mut body);

        Self {
            method,
            path,
            signed_path: url,
            body,
            api_key,
            bearer: authorization
                .as_deref()
                .and_then(bearer_from_header)
                .map(str::to_string),
            auth_version,
            key_id,
            timestamp,
            nonce,
            signature,
        }
    }

    fn presented(&self) -> Presented<'_> {
        Presented {
            api_key: self.api_key.as_deref(),
            bearer: self.bearer.as_deref(),
            auth_version: self.auth_version.as_deref(),
            key_id: self.key_id.as_deref(),
            timestamp: self.timestamp.as_deref(),
            nonce: self.nonce.as_deref(),
            signature: self.signature.as_deref(),
            method: &self.method,
            path: &self.signed_path,
            body: &self.body,
        }
    }
}

/// What a route needs before it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Guard {
    /// No credentials at all — only `/api/health`.
    Open,
    /// The shared API key alone. Enough to read build info.
    ApiKey,
    /// API key *and* a per-device credential. Everything that touches a unit.
    Full,
}

/// Run the server until the listener dies.
pub fn serve(state: Arc<AppState>) -> Result<(), Box<dyn std::error::Error>> {
    let server = tiny_http::Server::http(&state.settings.bind)
        .map_err(|e| format!("cannot bind {}: {e}", state.settings.bind))?;
    let workers = state.settings.worker_threads.max(1);
    eprintln!(
        "breeze-core listening on {} ({} workers, min auth version {})",
        state.settings.bind, workers, state.settings.min_auth_version
    );

    // A fixed pool rather than a thread per request. Requests block for ~1.8s on
    // a unit, so some concurrency is essential, but spawning per request would
    // let anyone who can reach the port exhaust memory.
    let (tx, rx) = mpsc::channel::<tiny_http::Request>();
    let rx = Arc::new(Mutex::new(rx));
    let mut handles = Vec::new();
    for _ in 0..workers {
        let rx = Arc::clone(&rx);
        let state = Arc::clone(&state);
        handles.push(std::thread::spawn(move || loop {
            // Hold the receiver lock only long enough to take one request.
            let next = { rx.lock().ok().and_then(|r| r.recv().ok()) };
            match next {
                Some(mut request) => {
                    let reply = handle(&state, &mut request);
                    let _ = request.respond(reply.into_http());
                }
                None => break,
            }
        }));
    }

    for request in server.incoming_requests() {
        if tx.send(request).is_err() {
            break;
        }
    }
    drop(tx);
    for h in handles {
        let _ = h.join();
    }
    Ok(())
}

/// Route one request, authenticating first.
fn handle(state: &AppState, request: &mut tiny_http::Request) -> Reply {
    let incoming = Incoming::read(request);
    let (guard, route) = match resolve(&incoming.method, &incoming.path) {
        Resolved::Route(guard, route) => (guard, route),
        Resolved::MethodNotAllowed(allow) => {
            return Reply::detail(405, "Method Not Allowed").with_header("Allow", allow)
        }
        Resolved::NotFound => return Reply::detail(404, "Not Found"),
    };

    if let Err(reply) = authorise(state, &incoming, &guard) {
        return reply;
    }
    route(state, &incoming)
}

type Handler = fn(&AppState, &Incoming) -> Reply;

/// What a path and method resolved to.
///
/// Deliberately not `PartialEq`: it carries a function pointer, and comparing
/// those compares addresses, which says nothing useful. Tests match on the
/// variant.
#[derive(Debug)]
enum Resolved {
    Route(Guard, Handler),
    /// The path exists but not under this method.
    MethodNotAllowed(&'static str),
    NotFound,
}

/// Match a method and path to a guard and a handler.
///
/// Written out rather than pulled from a router crate: the surface is small and
/// fixed, and the status codes matter more than the ergonomics. A known path
/// under the wrong method is a 405 with an `Allow` header, as FastAPI gives, so a
/// client can tell "wrong verb" from "no such thing".
fn resolve(method: &str, path: &str) -> Resolved {
    // Fixed paths first, so `/api/units/state` is never mistaken for a unit whose
    // id is "state".
    let fixed: &[(&str, &str, Guard, Handler)] = &[
        ("GET", "/api/health", Guard::Open, route_health),
        ("GET", "/api/version", Guard::ApiKey, route_version),
        ("GET", "/api/units", Guard::Full, route_units),
        ("GET", "/api/units/state", Guard::Full, route_all_states),
        ("GET", "/api/config", Guard::Full, route_config),
    ];
    for (m, p, guard, handler) in fixed {
        if *p == path {
            return if *m == method {
                Resolved::Route(*guard, *handler)
            } else {
                Resolved::MethodNotAllowed(m)
            };
        }
    }

    // /api/units/{id}/state and /api/units/{id}/control
    let Some(rest) = path.strip_prefix("/api/units/") else {
        return Resolved::NotFound;
    };
    let Some((_, tail)) = rest.split_once('/') else {
        return Resolved::NotFound;
    };
    match tail {
        "state" if method == "GET" => Resolved::Route(Guard::Full, route_unit_state),
        "state" => Resolved::MethodNotAllowed("GET"),
        "control" if method == "POST" => Resolved::Route(Guard::Full, route_control),
        "control" => Resolved::MethodNotAllowed("POST"),
        _ => Resolved::NotFound,
    }
}

/// Apply a route's guard, returning the rejection to send if it fails.
fn authorise(state: &AppState, incoming: &Incoming, guard: &Guard) -> Result<(), Reply> {
    if matches!(guard, Guard::Open) {
        return Ok(());
    }

    // API key first, always: it is the cheaper check and the one that keeps
    // unauthenticated traffic away from the device store entirely.
    let expected = state.api_key();
    if let Some(rejection) = state
        .verifier
        .verify_api_key(incoming.api_key.as_deref(), expected.as_deref())
    {
        return Err(Reply::json_body(rejection.status, &rejection.body()));
    }
    if matches!(guard, Guard::ApiKey) {
        return Ok(());
    }

    let devices = state
        .devices
        .read()
        .map_err(|_| Reply::detail(500, "device store unavailable"))?;
    let mut nonces = state
        .nonces
        .lock()
        .map_err(|_| Reply::detail(500, "nonce cache unavailable"))?;
    let now = breeze_auth::signing::now_seconds();

    match state
        .verifier
        .verify_device(&devices, &incoming.presented(), &mut nonces, now)
    {
        Decision::Allow(_) => Ok(()),
        Decision::Reject(r) => Err(Reply::json_body(r.status, &r.body())),
        Decision::UpgradeRequired { min_auth_version } => Err(Reply::json(
            426,
            &breeze_auth::reject::upgrade_required_body(min_auth_version),
        )),
    }
}

// ---------------------------------------------------------------- handlers

fn route_health(_: &AppState, _: &Incoming) -> Reply {
    Reply::json(200, &serde_json::json!({ "status": "ok" }))
}

fn route_version(state: &AppState, _: &Incoming) -> Reply {
    Reply::json(
        200,
        &serde_json::json!({
            "name": "Breeze Core",
            "version": state.version,
            "commit": option_env!("BREEZE_COMMIT").unwrap_or("unknown"),
            "features": FEATURES,
            "auth_versions": AUTH_VERSIONS,
            "min_auth_version": state.settings.min_auth_version,
            "units": state.manager.known_units().len(),
        }),
    )
}

fn route_units(state: &AppState, _: &Incoming) -> Reply {
    Reply::json_body(200, &units::list_units(&state.manager))
}

fn route_all_states(state: &AppState, _: &Incoming) -> Reply {
    Reply::json_body(200, &units::all_states(&state.manager))
}

/// The sanitised configuration.
///
/// Never the API key, never a unit's V3 token or key. Breeze Core's diagnostic
/// greps this response for anything secret-looking, so the omissions are checked
/// rather than trusted.
fn route_config(state: &AppState, _: &Incoming) -> Reply {
    let config = match state.config.read() {
        Ok(c) => c,
        Err(_) => return Reply::detail(500, "configuration unavailable"),
    };
    let units: Vec<serde_json::Value> = config
        .units
        .iter()
        .map(|u| {
            serde_json::json!({
                "id": u.id.to_string(),
                "name": u.name,
                "ip": u.ip,
                "port": u.port,
                // Whether credentials exist, never what they are.
                "has_v3_credentials": u.token.is_some() && u.key.is_some(),
            })
        })
        .collect();
    Reply::json(200, &serde_json::json!({ "units": units }))
}

/// Pull the unit id out of `/api/units/{id}/...`.
fn unit_id_from(path: &str) -> Option<u64> {
    path.strip_prefix("/api/units/")?
        .split('/')
        .next()?
        .parse()
        .ok()
}

fn route_unit_state(state: &AppState, incoming: &Incoming) -> Reply {
    let Some(id) = unit_id_from(&incoming.path) else {
        return Reply::detail(404, "unknown unit");
    };
    if !state.manager.contains(id) {
        return Reply::detail(404, "unknown unit");
    }
    match units::unit_state(&state.manager, id) {
        Ok(value) => Reply::json(200, &value),
        Err(e) => Reply::detail(503, e),
    }
}

fn route_control(state: &AppState, incoming: &Incoming) -> Reply {
    let Some(id) = unit_id_from(&incoming.path) else {
        return Reply::detail(404, "unknown unit");
    };
    if !state.manager.contains(id) {
        return Reply::detail(404, "unknown unit");
    }
    let request: ControlRequest = match serde_json::from_slice(&incoming.body) {
        Ok(r) => r,
        Err(e) => return Reply::detail(422, format!("invalid control request: {e}")),
    };
    // Bounds are checked here, before the value can reach the firmware. Breeze
    // Core answers 422 for these, so a client can tell a bad value from a unit
    // that would not answer.
    if let Err(e) = request.validate() {
        return Reply::detail(422, e.to_string());
    }
    match crate::control::apply(&state.manager, id, &request) {
        Ok(value) => Reply::json(200, &value),
        Err(e) => Reply::detail(503, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard_of(method: &str, path: &str) -> Guard {
        match resolve(method, path) {
            Resolved::Route(g, _) => g,
            other => panic!("{method} {path} did not route: {other:?}"),
        }
    }

    #[test]
    fn health_needs_no_credentials_and_nothing_else_is_open() {
        assert_eq!(guard_of("GET", "/api/health"), Guard::Open);
        for path in [
            "/api/version",
            "/api/units",
            "/api/units/state",
            "/api/config",
        ] {
            assert_ne!(
                guard_of("GET", path),
                Guard::Open,
                "{path} must not be open"
            );
        }
    }

    #[test]
    fn version_needs_the_key_but_not_a_device() {
        assert_eq!(guard_of("GET", "/api/version"), Guard::ApiKey);
    }

    #[test]
    fn everything_touching_a_unit_needs_both_credentials() {
        for (method, path) in [
            ("GET", "/api/units"),
            ("GET", "/api/units/state"),
            ("GET", "/api/units/153931628470980/state"),
            ("POST", "/api/units/153931628470980/control"),
        ] {
            assert_eq!(
                guard_of(method, path),
                Guard::Full,
                "{method} {path} must require a device credential"
            );
        }
    }

    #[test]
    fn a_known_path_under_the_wrong_method_is_405_with_the_verb_that_works() {
        // FastAPI answers 405 here, and a client can tell "wrong verb" from
        // "no such endpoint" only if we do the same.
        for (method, path, allow) in [
            ("POST", "/api/health", "GET"),
            ("GET", "/api/units/1/control", "POST"),
            ("POST", "/api/units/1/state", "GET"),
        ] {
            match resolve(method, path) {
                Resolved::MethodNotAllowed(a) => assert_eq!(a, allow),
                other => panic!("{method} {path} gave {other:?}, expected 405"),
            }
        }
    }

    #[test]
    fn genuinely_unknown_paths_are_not_found() {
        for path in ["/api/nope", "/", "/api/units/1/nonsense"] {
            match resolve("GET", path) {
                Resolved::NotFound => {}
                other => panic!("{path} gave {other:?}, expected 404"),
            }
        }
    }

    #[test]
    fn the_batch_route_is_not_mistaken_for_a_unit_named_state() {
        // /api/units/state must hit the batch handler, not be parsed as a unit
        // with id "state" -- an ordering bug that would 404 every batch read.
        assert_eq!(guard_of("GET", "/api/units/state"), Guard::Full);
        assert!(unit_id_from("/api/units/state").is_none());
    }

    #[test]
    fn unit_ids_are_parsed_from_the_path() {
        assert_eq!(
            unit_id_from("/api/units/153931628470980/state"),
            Some(153_931_628_470_980)
        );
        assert_eq!(unit_id_from("/api/units/1/control"), Some(1));
        assert_eq!(unit_id_from("/api/units/not-a-number/state"), None);
        assert_eq!(unit_id_from("/api/units//state"), None);
        assert_eq!(unit_id_from("/other"), None);
    }

    #[test]
    fn advertised_features_are_only_those_implemented() {
        // Clients branch on this list. Advertising live_stream before SSE exists
        // would make every client open a stream that never arrives.
        assert!(
            !FEATURES.contains(&"live_stream"),
            "SSE is not implemented yet"
        );
        assert!(
            !FEATURES.contains(&"programs"),
            "programs are not implemented yet"
        );
        assert!(
            !FEATURES.contains(&"sleep_timer"),
            "timers are not implemented yet"
        );
        assert!(FEATURES.contains(&"ed25519_auth"), "v2 auth does work");
    }
}
