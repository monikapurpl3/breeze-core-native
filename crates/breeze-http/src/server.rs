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
/// when `sleep_timer` is absent, for instance. So every entry must be a feature
/// a client can actually *use*, not one this build has some of.
///
/// That distinction is here because it was got wrong: `config_api` was listed
/// while only its read half existed. `GET /api/config` worked; the
/// `POST /api/units` and `PATCH /api/units/{id}` the same flag promises did not,
/// and the panel calls all three from its manage screen — so the flag produced
/// buttons that failed. When the reference groups several endpoints under one
/// flag, the flag means all of them, and `a_flag_covering_several_endpoints_
/// needs_all_of_them` now enforces that for this one.
const FEATURES: &[&str] = &[
    "batch_state",
    "beep_control",
    "config_api",
    "delete_unit",
    "device_pairing",
    "ed25519_auth",
    "live_stream",
    "programs",
    "sleep_timer",
    "system_info",
    "unit_history",
    "metrics",
    "unit_scan",
    "whoami",
];

const AUTH_VERSIONS: &[u8] = &[1, 2];

/// The advertised feature list, so `/api/system` reports the same one
/// `/api/version` does rather than a second copy that could drift.
pub fn features() -> &'static [&'static str] {
    FEATURES
}

/// Device auth versions this build understands.
pub fn auth_versions() -> &'static [u8] {
    AUTH_VERSIONS
}

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
    /// The socket peer, and the forwarded header. Which one counts depends on
    /// `behind_proxy`; see `breeze_auth::net`.
    peer: Option<std::net::IpAddr>,
    forwarded_for: Option<String>,
    /// Set by `authorise` once a device has been identified, so a route never
    /// re-verifies. Re-verifying would spend the v2 nonce twice and reject the
    /// caller as a replay -- which is exactly what `whoami` used to do.
    device_token_id: Option<String>,
    /// For the panel: lets an unchanged file answer 304 instead of resending.
    if_none_match: Option<String>,
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
        let forwarded_for = header("x-forwarded-for");
        let if_none_match = header("if-none-match");
        let peer = request.remote_addr().map(|a| a.ip());

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
            peer,
            forwarded_for,
            if_none_match,
            device_token_id: None,
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
    /// API key *and* a private client address. Approving a pairing and
    /// managing devices: a leaked key must not be enough to gain control, so
    /// somebody has to be on the trusted network.
    AdminLan,
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
                Some(mut request) => match handle(&state, &mut request) {
                    Outcome::Reply(reply) => {
                        let _ = request.respond(reply.into_http(state.settings.security_headers));
                    }
                    // An endless response cannot be handed to `respond()`, and
                    // it must not hold a pooled worker either: eight open
                    // streams would starve the whole API. It gets its own thread.
                    Outcome::Stream => crate::stream::hijack(Arc::clone(&state), request),
                },
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

/// What the worker should do with a request once it has been routed.
enum Outcome {
    Reply(Reply),
    /// Hand the connection over to the SSE machinery, which owns it from then on.
    Stream,
}

/// Route one request, authenticating first.
fn handle(state: &AppState, request: &mut tiny_http::Request) -> Outcome {
    let mut incoming = Incoming::read(request);
    match resolve(&incoming.method, &incoming.path) {
        Resolved::MethodNotAllowed(allow) => {
            Outcome::Reply(Reply::detail(405, "Method Not Allowed").with_header("Allow", allow))
        }
        // The panel is the last resort, tried only once every `/api/*` path has
        // failed to match -- the same ordering as the reference, whose static
        // mount sits at `/` below the routers. It needs no credentials, also as
        // there: the page prompts for the API key itself, and gating it would
        // leave nowhere to type one in.
        Resolved::NotFound => {
            if incoming.method == "GET" || incoming.method == "HEAD" {
                if let Some(mut reply) =
                    crate::panel::serve(&incoming.path, incoming.if_none_match.as_deref())
                {
                    if incoming.method == "HEAD" {
                        reply.body.clear();
                    }
                    return Outcome::Reply(reply);
                }
            }
            Outcome::Reply(Reply::detail(404, "Not Found"))
        }
        Resolved::Route(guard, route) => match authorise(state, &incoming, &guard) {
            Err(reply) => Outcome::Reply(reply),
            Ok(who) => {
                // Handed to the route rather than left for it to work out again:
                // a second verification would spend the v2 nonce twice.
                incoming.device_token_id = who;
                Outcome::Reply(route(state, &incoming))
            }
        },
        // Authenticated exactly like any other route -- a stream reads live
        // state, so it sits behind the same guard as reading one unit. Nothing
        // is written here; the caller passes the connection on.
        Resolved::Stream(guard) => match authorise(state, &incoming, &guard) {
            Err(reply) => Outcome::Reply(reply),
            Ok(_) => Outcome::Stream,
        },
    }
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
    /// `GET /api/units/stream` — the connection is handed to the SSE machinery
    /// rather than answered with a body.
    Stream(Guard),
}

/// Match a method and path to a guard and a handler.
///
/// Written out rather than pulled from a router crate: the surface is small and
/// fixed, and the status codes matter more than the ergonomics. A known path
/// under the wrong method is a 405 with an `Allow` header, as FastAPI gives, so a
/// client can tell "wrong verb" from "no such thing".
fn resolve(method: &str, path: &str) -> Resolved {
    // Before anything else: a stream is not a `Handler`. It never produces a
    // body to hand back, so it cannot be expressed as one — and it has to be
    // matched ahead of the `/api/units/{id}` patterns, or "stream" reads as a
    // unit id.
    if path == "/api/units/stream" {
        return if method == "GET" {
            Resolved::Stream(Guard::Full)
        } else {
            Resolved::MethodNotAllowed("GET")
        };
    }
    // Also ahead of the /{id} patterns, or "scan" is taken for a unit id and a
    // DELETE would try to remove a unit called "scan".
    if path == "/api/units/scan" {
        return if method == "GET" {
            Resolved::Route(Guard::Full, route_scan)
        } else {
            Resolved::MethodNotAllowed("GET")
        };
    }

    // Fixed paths first, so `/api/units/state` is never mistaken for a unit whose
    // id is "state".
    let fixed: &[(&str, &str, Guard, Handler)] = &[
        ("GET", "/api/health", Guard::Open, route_health),
        ("GET", "/api/version", Guard::ApiKey, route_version),
        // Not under /api: Prometheus convention, and the reference does the
        // same. Key-gated, because indoor temperature says whether anyone is in.
        ("GET", "/metrics", Guard::ApiKey, route_metrics),
        ("GET", "/api/units", Guard::Full, route_units),
        ("GET", "/api/units/state", Guard::Full, route_all_states),
        ("GET", "/api/config", Guard::Full, route_config),
        ("GET", "/api/system", Guard::Full, route_system),
        ("POST", "/api/units", Guard::Full, route_add_unit),
        (
            "POST",
            "/api/auth/enroll/start",
            Guard::ApiKey,
            route_enroll_start,
        ),
        (
            "POST",
            "/api/auth/enroll/poll",
            Guard::ApiKey,
            route_enroll_poll,
        ),
        (
            "POST",
            "/api/auth/enroll/approve",
            Guard::AdminLan,
            route_enroll_approve,
        ),
        (
            "GET",
            "/api/auth/devices",
            Guard::AdminLan,
            route_list_devices,
        ),
        ("GET", "/api/auth/whoami", Guard::Full, route_whoami),
        ("POST", "/api/auth/upgrade", Guard::Full, route_upgrade),
        // Before the /{id} pattern, or "status" is captured as a timer id.
        ("GET", "/api/timers/status", Guard::Full, route_timer_status),
        ("GET", "/api/timers", Guard::Full, route_timers_list),
        ("POST", "/api/timers", Guard::Full, route_timer_create),
        // Likewise before /{id}, or "status" is read as a program id.
        (
            "GET",
            "/api/programs/status",
            Guard::Full,
            route_program_status,
        ),
        ("GET", "/api/programs", Guard::Full, route_programs_list),
        ("POST", "/api/programs", Guard::Full, route_program_create),
    ];
    // Two passes, because one path may accept more than one method: returning
    // on the first *path* match made POST /api/timers answer 405 pointing at GET,
    // since the GET entry came first.
    for (m, p, guard, handler) in fixed {
        if *p == path && *m == method {
            return Resolved::Route(*guard, *handler);
        }
    }
    // The path exists but not under this method; name one that works.
    for (m, p, _, _) in fixed {
        if *p == path {
            return Resolved::MethodNotAllowed(m);
        }
    }

    if let Some(timer_id) = path.strip_prefix("/api/timers/") {
        if timer_id.is_empty() || timer_id.contains('/') {
            return Resolved::NotFound;
        }
        return if method == "DELETE" {
            Resolved::Route(Guard::Full, route_timer_cancel)
        } else {
            Resolved::MethodNotAllowed("DELETE")
        };
    }

    if let Some(rest) = path.strip_prefix("/api/programs/") {
        // Either "{id}" or "{id}/apply" -- nothing else.
        if let Some(id) = rest.strip_suffix("/apply") {
            if id.is_empty() || id.contains('/') {
                return Resolved::NotFound;
            }
            return if method == "POST" {
                Resolved::Route(Guard::Full, route_program_apply)
            } else {
                Resolved::MethodNotAllowed("POST")
            };
        }
        if rest.is_empty() || rest.contains('/') {
            return Resolved::NotFound;
        }
        return match method {
            "GET" => Resolved::Route(Guard::Full, route_program_get),
            "PUT" => Resolved::Route(Guard::Full, route_program_update),
            "DELETE" => Resolved::Route(Guard::Full, route_program_delete),
            _ => Resolved::MethodNotAllowed("GET"),
        };
    }

    if let Some(token_id) = path.strip_prefix("/api/auth/devices/") {
        if token_id.is_empty() || token_id.contains('/') {
            return Resolved::NotFound;
        }
        return if method == "DELETE" {
            Resolved::Route(Guard::AdminLan, route_revoke_device)
        } else {
            Resolved::MethodNotAllowed("DELETE")
        };
    }

    // /api/units/{id}, and /api/units/{id}/{action}
    let Some(rest) = path.strip_prefix("/api/units/") else {
        return Resolved::NotFound;
    };
    let Some((_, tail)) = rest.split_once('/') else {
        // No action: the unit itself. Renaming and removing live here.
        if rest.is_empty() {
            return Resolved::NotFound;
        }
        return match method {
            "PATCH" => Resolved::Route(Guard::Full, route_rename_unit),
            "DELETE" => Resolved::Route(Guard::Full, route_delete_unit),
            _ => Resolved::MethodNotAllowed("PATCH"),
        };
    };
    match tail {
        "state" if method == "GET" => Resolved::Route(Guard::Full, route_unit_state),
        "history" if method == "GET" => Resolved::Route(Guard::Full, route_unit_history),
        "history" => Resolved::MethodNotAllowed("GET"),
        "state" => Resolved::MethodNotAllowed("GET"),
        "control" if method == "POST" => Resolved::Route(Guard::Full, route_control),
        "control" => Resolved::MethodNotAllowed("POST"),
        _ => Resolved::NotFound,
    }
}

/// Apply a route's guard, returning the rejection to send if it fails.
/// Record that a device just authenticated successfully.
///
/// In memory only, deliberately: the reference keeps the verify path free of
/// disk I/O, and writing `devices.json` on every request would turn each read of
/// a thermostat into a file rewrite. The consequence is that `last_used` resets
/// on restart, which is the reference's behaviour too.
///
/// This was missing entirely at first — `last_used` was written as `null` at
/// enrolment and never touched again, so `/api/auth/whoami` reported a device
/// that had never been seen while answering the very request that used it.
/// Caught by diffing whoami against the reference.
fn mark_used(state: &AppState, token_id: &str, now: f64) {
    if let Ok(mut devices) = state.devices.write() {
        if let Some(record) = devices.devices.iter_mut().find(|d| d.token_id == token_id) {
            record.last_used = Some(now);
        }
    }
}

/// Returns the identified device's token id, when the guard involved one.
fn authorise(
    state: &AppState,
    incoming: &Incoming,
    guard: &Guard,
) -> Result<Option<String>, Reply> {
    if matches!(guard, Guard::Open) {
        return Ok(None);
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
        return Ok(None);
    }
    if matches!(guard, Guard::AdminLan) {
        let ip = breeze_auth::client_ip(
            incoming.peer,
            incoming.forwarded_for.as_deref(),
            state.settings.behind_proxy,
        );
        if !breeze_auth::is_private_ip(ip) {
            // Deliberately a 403, not a 401: the credential was fine, the
            // *location* was not, and a client retrying with better credentials
            // would never succeed.
            return Err(Reply::detail(
                403,
                "this admin action must come from the local network",
            ));
        }
        return Ok(None);
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
        Decision::Allow(who) => {
            let token_id = who.token_id.to_string();
            // Release the read lock before asking for the write lock, or this
            // deadlocks on the first authenticated request.
            drop(nonces);
            drop(devices);
            mark_used(state, &token_id, now);
            Ok(Some(token_id))
        }
        Decision::Reject(r) => Err(Reply::json_body(r.status, &r.body())),
        Decision::UpgradeRequired { min_auth_version } => Err(Reply::json(
            426,
            &breeze_auth::reject::upgrade_required_body(min_auth_version),
        )),
    }
}

// ---------------------------------------------------------------- handlers

fn route_timers_list(state: &AppState, _: &Incoming) -> Reply {
    crate::timer_routes::list(state)
}

fn route_timer_create(state: &AppState, incoming: &Incoming) -> Reply {
    crate::timer_routes::create(state, &incoming.body)
}

fn route_timer_status(state: &AppState, _: &Incoming) -> Reply {
    crate::timer_routes::status(state)
}

fn route_timer_cancel(state: &AppState, incoming: &Incoming) -> Reply {
    let id = incoming
        .path
        .strip_prefix("/api/timers/")
        .unwrap_or_default();
    crate::timer_routes::cancel(state, id)
}

fn route_programs_list(state: &AppState, _: &Incoming) -> Reply {
    crate::program_routes::list(state)
}

fn route_program_create(state: &AppState, incoming: &Incoming) -> Reply {
    crate::program_routes::create(state, &incoming.body)
}

fn route_program_status(state: &AppState, _: &Incoming) -> Reply {
    crate::program_routes::status(state)
}

/// The id in `/api/programs/{id}`.
fn program_id_from(path: &str) -> &str {
    path.strip_prefix("/api/programs/").unwrap_or_default()
}

fn route_program_get(state: &AppState, incoming: &Incoming) -> Reply {
    crate::program_routes::get(state, program_id_from(&incoming.path))
}

fn route_program_update(state: &AppState, incoming: &Incoming) -> Reply {
    crate::program_routes::update(state, program_id_from(&incoming.path), &incoming.body)
}

fn route_program_delete(state: &AppState, incoming: &Incoming) -> Reply {
    crate::program_routes::delete(state, program_id_from(&incoming.path))
}

fn route_program_apply(state: &AppState, incoming: &Incoming) -> Reply {
    let id = program_id_from(&incoming.path)
        .strip_suffix("/apply")
        .unwrap_or_default();
    crate::program_routes::apply_now(state, id)
}

fn route_upgrade(state: &AppState, incoming: &Incoming) -> Reply {
    crate::auth_routes::upgrade(state, incoming.device_token_id.as_deref(), &incoming.body)
}

fn route_enroll_start(state: &AppState, incoming: &Incoming) -> Reply {
    crate::auth_routes::start(state, &incoming.body)
}

fn route_enroll_poll(state: &AppState, incoming: &Incoming) -> Reply {
    crate::auth_routes::poll(state, &incoming.body)
}

fn route_enroll_approve(state: &AppState, incoming: &Incoming) -> Reply {
    crate::auth_routes::approve(state, &incoming.body)
}

fn route_list_devices(state: &AppState, _: &Incoming) -> Reply {
    crate::auth_routes::list_devices(state)
}

fn route_revoke_device(state: &AppState, incoming: &Incoming) -> Reply {
    let token_id = incoming
        .path
        .strip_prefix("/api/auth/devices/")
        .unwrap_or_default();
    crate::auth_routes::revoke(state, token_id)
}

/// `GET /api/auth/whoami` — which device is calling.
///
/// Needs the device credential it then describes, so a client can confirm what
/// the server thinks it is without guessing.
fn route_whoami(state: &AppState, incoming: &Incoming) -> Reply {
    // Uses the identity `authorise` already established. This used to re-run
    // verification here, on the theory that the guard says yes without saying
    // who -- which works for a v1 bearer token and is *broken* for v2: the
    // nonce was spent by the first verification, so the second is a replay and
    // every Ed25519 client got a 401 from the one route meant to tell it who it
    // is.
    let Some(token_id) = incoming.device_token_id.as_deref() else {
        return Reply::detail(401, "not an authenticated device");
    };
    let devices = match state.devices.read() {
        Ok(d) => d,
        Err(_) => return Reply::detail(500, "device store unavailable"),
    };
    match devices.devices.iter().find(|d| d.token_id == token_id) {
        Some(d) => Reply::json(
            200,
            &serde_json::json!({
                "token_id": d.token_id,
                "label": d.label,
                "auth_version": d.auth_version,
                "created_at": d.created_at,
                "expires_at": d.expires_at,
                // Was missing here while the reference sent it -- caught by
                // diffing the two responses. A client showing "last seen" for
                // this device would have had nothing to show.
                "last_used": d.last_used,
            }),
        ),
        None => Reply::detail(404, "device not found"),
    }
}

/// `GET /api/system`
fn route_system(state: &AppState, incoming: &Incoming) -> Reply {
    // How this client is reaching us, reported back because it is the fastest
    // way to explain a class of confusing failures: a request that arrives
    // looking like 127.0.0.1 means the proxy is not forwarding the real
    // address, which is what silently breaks LAN-only approval.
    let peer = breeze_auth::client_ip(
        incoming.peer,
        incoming.forwarded_for.as_deref(),
        state.settings.behind_proxy,
    );
    let connection = serde_json::json!({
        "client_ip": peer.map(|ip| ip.to_string()),
        "client_is_private": breeze_auth::is_private_ip(peer),
        "forwarded_for": incoming.forwarded_for,
        "behind_proxy_enabled": state.settings.behind_proxy,
    });
    Reply::json(200, &crate::system::snapshot(state, connection))
}

fn route_metrics(state: &AppState, _: &Incoming) -> Reply {
    crate::metrics::render(state)
}

fn route_health(_: &AppState, _: &Incoming) -> Reply {
    Reply::json(200, &serde_json::json!({ "status": "ok" }))
}

fn route_version(state: &AppState, _: &Incoming) -> Reply {
    Reply::json(
        200,
        &serde_json::json!({
            "name": "Breeze Core",
            "version": state.version,
            "commit": crate::build_commit(),
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
    let envelope = units::all_states(&state.manager);
    // Both halves of the envelope are readings worth keeping: an unreachable
    // unit is exactly the thing a graph should show a gap for.
    let now = crate::history::now_unix();
    for key in ["states", "errors"] {
        if let Some(list) = envelope.get(key).and_then(|v| v.as_array()) {
            for state_value in list {
                state.history.record(state_value, now);
            }
        }
    }
    Reply::json_body(200, &envelope)
}

/// The sanitised configuration.
///
/// Never the API key, never a unit's V3 token or key. Breeze Core's diagnostic
/// greps this response for anything secret-looking, so the omissions are checked
/// rather than trusted.
fn route_config(state: &AppState, _: &Incoming) -> Reply {
    // Delegated so there is exactly one place that decides what a unit looks
    // like to a client -- and therefore exactly one place a credential could
    // leak from, with one test guarding it.
    crate::config_routes::get_config(state)
}

fn route_add_unit(state: &AppState, incoming: &Incoming) -> Reply {
    crate::config_routes::add_unit(state, &incoming.body)
}

fn route_rename_unit(state: &AppState, incoming: &Incoming) -> Reply {
    crate::config_routes::rename_unit(state, unit_segment(&incoming.path), &incoming.body)
}

fn route_delete_unit(state: &AppState, incoming: &Incoming) -> Reply {
    crate::config_routes::delete_unit(state, unit_segment(&incoming.path))
}

fn route_scan(state: &AppState, incoming: &Incoming) -> Reply {
    // The query lives on `signed_path`, which is the full URL; `path` has been
    // stripped of it.
    let query = incoming
        .signed_path
        .split_once('?')
        .map(|(_, q)| q)
        .unwrap_or_default();
    crate::config_routes::scan(state, query)
}

/// Pull the unit id out of `/api/units/{id}/...`.
fn unit_id_from(path: &str) -> Option<u64> {
    path.strip_prefix("/api/units/")?
        .split('/')
        .next()?
        .parse()
        .ok()
}

/// The raw `{id}` segment, so an error can quote what was actually asked for.
fn unit_segment(path: &str) -> &str {
    path.strip_prefix("/api/units/")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("")
}

/// The reference's exact wording, id included.
///
/// It said `Unknown unit '999'` where this said `unknown unit` — caught by
/// diffing the two servers. Clients surface this string to a person, so the
/// difference is one a user could see.
fn unknown_unit(path: &str) -> Reply {
    Reply::detail(404, format!("Unknown unit '{}'", unit_segment(path)))
}

fn route_unit_state(state: &AppState, incoming: &Incoming) -> Reply {
    let Some(id) = unit_id_from(&incoming.path) else {
        return unknown_unit(&incoming.path);
    };
    if !state.manager.contains(id) {
        return unknown_unit(&incoming.path);
    }
    match units::unit_state(&state.manager, id) {
        Ok(value) => {
            // Remembered on the way past: history costs no extra LAN traffic,
            // it just keeps a reading something else already asked for.
            state.history.record(&value, crate::history::now_unix());
            Reply::json(200, &value)
        }
        Err(e) => Reply::detail(503, e),
    }
}

/// `GET /api/units/{id}/history`
fn route_unit_history(state: &AppState, incoming: &Incoming) -> Reply {
    let Some(id) = unit_id_from(&incoming.path) else {
        return unknown_unit(&incoming.path);
    };
    if !state.manager.contains(id) {
        return unknown_unit(&incoming.path);
    }
    let unit_id = id.to_string();
    Reply::json_body(
        200,
        &serde_json::json!({
            "id": unit_id,
            "samples": state.history.samples(&unit_id),
        }),
    )
}

fn route_control(state: &AppState, incoming: &Incoming) -> Reply {
    let Some(id) = unit_id_from(&incoming.path) else {
        return unknown_unit(&incoming.path);
    };
    if !state.manager.contains(id) {
        return unknown_unit(&incoming.path);
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
    fn pairing_is_split_between_the_key_and_the_lan() {
        // The whole point of the scheme: the key gets a client as far as asking.
        assert_eq!(guard_of("POST", "/api/auth/enroll/start"), Guard::ApiKey);
        assert_eq!(guard_of("POST", "/api/auth/enroll/poll"), Guard::ApiKey);
        // Approving, listing and revoking are admin actions from the LAN.
        assert_eq!(
            guard_of("POST", "/api/auth/enroll/approve"),
            Guard::AdminLan
        );
        assert_eq!(guard_of("GET", "/api/auth/devices"), Guard::AdminLan);
        assert_eq!(
            guard_of("DELETE", "/api/auth/devices/abc123"),
            Guard::AdminLan
        );
    }

    #[test]
    fn whoami_needs_the_credential_it_describes() {
        assert_eq!(guard_of("GET", "/api/auth/whoami"), Guard::Full);
    }

    #[test]
    fn a_device_id_is_required_to_revoke() {
        // A stray trailing slash must not look like a revoke of everything.
        for path in ["/api/auth/devices/", "/api/auth/devices/a/b"] {
            match resolve("DELETE", path) {
                Resolved::NotFound => {}
                other => panic!("{path} gave {other:?}, expected 404"),
            }
        }
    }

    #[test]
    fn revoking_by_the_wrong_method_says_which_one_works() {
        match resolve("GET", "/api/auth/devices/abc123") {
            Resolved::MethodNotAllowed(a) => assert_eq!(a, "DELETE"),
            other => panic!("expected 405, got {other:?}"),
        }
    }

    #[test]
    fn a_path_that_accepts_two_methods_routes_both() {
        // /api/timers takes GET and POST. Resolving on the first path match
        // made POST answer 405 pointing at GET, which is how the live test
        // found it -- no unit test here had tried the second method.
        assert_eq!(guard_of("GET", "/api/timers"), Guard::Full);
        assert_eq!(guard_of("POST", "/api/timers"), Guard::Full);
        // And a method neither entry offers is still a 405.
        match resolve("PUT", "/api/timers") {
            Resolved::MethodNotAllowed(_) => {}
            other => panic!("expected 405, got {other:?}"),
        }
    }

    #[test]
    fn the_timer_status_route_is_not_read_as_a_timer_id() {
        assert_eq!(guard_of("GET", "/api/programs/status"), Guard::Full);
        assert_eq!(guard_of("GET", "/api/timers/status"), Guard::Full);
        assert_eq!(guard_of("DELETE", "/api/timers/abc123"), Guard::Full);
    }

    #[test]
    fn advertised_features_are_only_those_implemented() {
        // Clients branch on this list. Advertising live_stream before SSE exists
        // would make every client open a stream that never arrives.
        assert!(
            FEATURES.contains(&"device_pairing"),
            "pairing does work now"
        );
        assert!(FEATURES.contains(&"sleep_timer"), "timers do work now");
        assert!(
            FEATURES.contains(&"live_stream"),
            "SSE does work now -- and the route must exist for this to be true"
        );
        assert!(
            matches!(resolve("GET", "/api/units/stream"), Resolved::Stream(_)),
            "live_stream is advertised, so the route has to be there"
        );
        assert!(
            FEATURES.contains(&"programs"),
            "favourites, schedules and curves do work now"
        );
        assert!(FEATURES.contains(&"ed25519_auth"), "v2 auth does work");
        // Still not implemented, and so still not advertised.
        for absent in ["compression", "unit_capabilities"] {
            assert!(
                !FEATURES.contains(&absent),
                "{absent} is advertised but not implemented"
            );
        }
    }

    #[test]
    fn a_flag_covering_several_endpoints_needs_all_of_them() {
        // The trap that put `config_api` in the list wrongly: the reference files
        // `GET /api/config`, `POST /api/units` and `PATCH /api/units/{id}` under
        // that one flag, and only the first was built. So the flag may go back in
        // only once every route it promises resolves.
        let config_api_routes = [
            ("GET", "/api/config"),
            ("POST", "/api/units"),
            ("PATCH", "/api/units/153931628470980"),
        ];
        let all_present = config_api_routes
            .iter()
            .all(|(m, p)| matches!(resolve(m, p), Resolved::Route(..)));
        assert_eq!(
            FEATURES.contains(&"config_api"),
            all_present,
            "config_api must be advertised exactly when all of {config_api_routes:?} route"
        );
    }

    #[test]
    fn the_stream_is_behind_the_same_guard_as_reading_a_unit() {
        match resolve("GET", "/api/units/stream") {
            Resolved::Stream(guard) => assert_eq!(guard, Guard::Full),
            other => panic!("stream did not route: {other:?}"),
        }
    }

    #[test]
    fn stream_is_not_mistaken_for_a_unit_id() {
        // `/api/units/{id}/state` would otherwise swallow it, and a GET would
        // 404 looking for a unit called "stream".
        assert!(matches!(
            resolve("GET", "/api/units/stream"),
            Resolved::Stream(_)
        ));
        // The batch route is still its own thing.
        assert_eq!(guard_of("GET", "/api/units/state"), Guard::Full);
        // And the wrong verb on the stream names the right one.
        match resolve("POST", "/api/units/stream") {
            Resolved::MethodNotAllowed(m) => assert_eq!(m, "GET"),
            other => panic!("expected 405, got {other:?}"),
        }
    }

    #[test]
    fn every_program_route_resolves_under_full_auth() {
        // Programs are user features, so any enrolled client manages them --
        // but nothing weaker than a paired device.
        assert_eq!(guard_of("GET", "/api/programs"), Guard::Full);
        assert_eq!(guard_of("POST", "/api/programs"), Guard::Full);
        assert_eq!(guard_of("GET", "/api/programs/abc123"), Guard::Full);
        assert_eq!(guard_of("PUT", "/api/programs/abc123"), Guard::Full);
        assert_eq!(guard_of("DELETE", "/api/programs/abc123"), Guard::Full);
        assert_eq!(guard_of("POST", "/api/programs/abc123/apply"), Guard::Full);
    }

    #[test]
    fn status_is_not_mistaken_for_a_program_id() {
        // The static entry has to be matched before the /{id} pattern, or
        // GET /api/programs/status looks up a program called "status" and 404s.
        match resolve("GET", "/api/programs/status") {
            Resolved::Route(..) => {}
            other => panic!("status did not route: {other:?}"),
        }
    }

    #[test]
    fn a_wrong_method_on_a_program_names_one_that_works() {
        match resolve("PATCH", "/api/programs/abc123") {
            Resolved::MethodNotAllowed(m) => assert_eq!(m, "GET"),
            other => panic!("expected 405, got {other:?}"),
        }
        // apply is POST-only.
        match resolve("GET", "/api/programs/abc123/apply") {
            Resolved::MethodNotAllowed(m) => assert_eq!(m, "POST"),
            other => panic!("expected 405, got {other:?}"),
        }
    }

    #[test]
    fn nonsense_program_paths_are_not_found() {
        assert!(matches!(
            resolve("GET", "/api/programs/"),
            Resolved::NotFound
        ));
        assert!(matches!(
            resolve("POST", "/api/programs//apply"),
            Resolved::NotFound
        ));
        // A deeper path is not a program id with a slash in it.
        assert!(matches!(
            resolve("GET", "/api/programs/abc/def"),
            Resolved::NotFound
        ));
    }

    #[test]
    fn an_unknown_unit_is_reported_the_way_the_reference_reports_it() {
        // Not cosmetic: clients put this string in front of a person, and the
        // two servers disagreed until a byte-level diff caught it.
        assert_eq!(unit_segment("/api/units/999/state"), "999");
        assert_eq!(unit_segment("/api/units/abc/control"), "abc");
        assert_eq!(unit_segment("/api/units/7"), "7");
        assert_eq!(unit_segment("/nonsense"), "");

        let body: serde_json::Value =
            serde_json::from_slice(&unknown_unit("/api/units/999/state").body).unwrap();
        assert_eq!(body["detail"], "Unknown unit '999'");
        // A non-numeric id is quoted as given rather than swallowed.
        let body: serde_json::Value =
            serde_json::from_slice(&unknown_unit("/api/units/abc/state").body).unwrap();
        assert_eq!(body["detail"], "Unknown unit 'abc'");
    }

    #[test]
    fn the_config_write_routes_all_resolve() {
        // The flag test above ties `config_api` to these, so this is what makes
        // that assertion meaningful rather than circular.
        assert_eq!(guard_of("GET", "/api/config"), Guard::Full);
        assert_eq!(guard_of("POST", "/api/units"), Guard::Full);
        assert_eq!(guard_of("PATCH", "/api/units/153931628470980"), Guard::Full);
        assert_eq!(
            guard_of("DELETE", "/api/units/153931628470980"),
            Guard::Full
        );
        assert_eq!(guard_of("GET", "/api/units/scan"), Guard::Full);
    }

    #[test]
    fn scan_is_not_mistaken_for_a_unit_id() {
        // Without its own arm, "scan" reads as an id -- and a DELETE would try
        // to remove a unit by that name.
        match resolve("GET", "/api/units/scan") {
            Resolved::Route(..) => {}
            other => panic!("scan did not route: {other:?}"),
        }
        match resolve("DELETE", "/api/units/scan") {
            Resolved::MethodNotAllowed(m) => assert_eq!(m, "GET"),
            other => panic!("expected 405 for DELETE on scan, got {other:?}"),
        }
    }

    #[test]
    fn a_bare_unit_path_takes_only_patch_and_delete() {
        // GET /api/units/{id} is not a route in the reference either -- state
        // lives at /api/units/{id}/state.
        match resolve("GET", "/api/units/7") {
            Resolved::MethodNotAllowed(m) => assert_eq!(m, "PATCH"),
            other => panic!("expected 405, got {other:?}"),
        }
        assert!(matches!(resolve("GET", "/api/units/"), Resolved::NotFound));
    }

    #[test]
    fn the_batch_and_stream_paths_still_win_over_the_id_pattern() {
        // Three static paths now sit where a unit id would go. Each must be
        // matched before the pattern, and this is the regression test for all
        // of them at once.
        assert_eq!(guard_of("GET", "/api/units/state"), Guard::Full);
        assert!(matches!(
            resolve("GET", "/api/units/stream"),
            Resolved::Stream(_)
        ));
        assert_eq!(guard_of("GET", "/api/units/scan"), Guard::Full);
        // And a real id still reaches its own routes.
        assert_eq!(guard_of("GET", "/api/units/7/state"), Guard::Full);
        assert_eq!(guard_of("POST", "/api/units/7/control"), Guard::Full);
    }
}
