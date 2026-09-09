# Architecture

```
   Browser / Breeze app ─┐
                         │  HTTPS (via reverse proxy)  or  HTTP on the LAN
   breeze-core control ──┤
   breeze-core diag ─────┤
                         ▼
              ┌──────────────────────────────────┐
              │  breeze-core serve               │
              │   ├── /api/auth/*    pairing     │
              │   ├── /api/units*    control     │
              │   ├── /api/programs  favourites / schedules / curves
              │   ├── /api/timers    one-shot    │
              │   ├── /metrics       Prometheus  │
              │   └── /              the panel, compiled in
              │            │ breeze-proto        │
              │            ▼                     │
              │   cached Device per unit         │
              └────────────┬─────────────────────┘
                           │ TCP :6444 per unit
                    Midea AC units on the LAN
```

Three components share exactly one contract — the `/api/*` endpoints — and are
otherwise fully decoupled. Delete either client and the API and the other client
keep working.

The Midea LAN protocol is implemented directly here — the framing, the V3
handshake, the encryption and discovery — and the panel is bytes inside the
executable rather than files on disk. Both are described below.

## Seven crates

Each one either has no dependencies on the others or a single direction of
dependency. Nothing is circular, and nothing reaches for global state.

| Crate | Depends on | Responsibility |
|---|---|---|
| `breeze-proto` | — | The Midea LAN protocol: V2/V3 framing, the handshake, encryption, discovery, and the AC command and response layout. |
| `breeze-store` | — | The four store files, read and written **byte-compatibly with the format already on disk**. Also the shared `ControlRequest` and its bounds. |
| `breeze-cloud` | — | Fetching a unit's V3 credentials from the vendor cloud, once, as a last resort. |
| `breeze-auth` | `breeze-store` | The two-credential model: API key, v1 bearer tokens, v2 Ed25519 request signing, the enrolment handshake, the nonce cache. |
| `breeze-device` | `breeze-proto` | Connection lifecycle, the per-unit cache and lock, LAN scanning. |
| `breeze-http` | the above | The REST surface, the SSE stream, the scheduler and timer runners, the panel, `/metrics`. |
| `breeze-core` | the above | The binary: argument parsing, `serve`, and every client subcommand. |

**`breeze-store` exists so that an install can be upgraded rather than
migrated.** Its whole job is reading and writing exactly the shapes already on
disk — including fields this implementation never itself reads, and including
`null` for an absent optional rather than omitting the key, because the stored
files record absent fields explicitly. A migration that rewrites every file
holding a paired unit's irreplaceable credentials is not one anybody should
have to trust.

## The dependency list is the interesting part

Fourteen external crates, and the absences matter as much as the entries:

`aes` · `base64` · `chrono` · `ed25519-dalek` · `flate2` · `getrandom` ·
`md-5` · `serde` · `serde_json` · `sha2` · `sha3` · `subtle` · `tiny_http` ·
`ureq`

- **No async runtime.** No tokio, no futures. The HTTP server is `tiny_http`
  with a fixed thread pool, and the three background jobs are threads. A device
  round-trip is ~700 ms of waiting on one socket; there is no throughput problem
  here that an executor would solve, and the whole thing is easier to reason
  about with a lock per unit than with tasks.
- **No web framework.** The router is a match on method and path, written out.
  The surface is small and fixed, and the status codes matter more than the
  ergonomics — a known path under the wrong method is a `405` with an `Allow`
  header, as FastAPI gave.
- **No TLS server.** Deliberate: the server speaks plain HTTP and a reverse
  proxy terminates TLS. `ureq` brings TLS for the *outbound* cloud call only,
  and its roots are compiled in, which is why a container needs no
  `ca-certificates`.
- **`sha3` is not `sha2`.** Both are here and they are not interchangeable: the
  v2 canonical string uses **SHA3-512**, which WebCrypto does not implement at
  all — the panel ships a hand-written Keccak for it. Using the wrong one means
  the panel and the server disagree about every request.
- **`subtle`** for constant-time comparison. Every secret comparison — API key,
  code hashes, token hashes — goes through it.

## Four decisions worth knowing

**Connections are lazy and cached per unit.** The first request for a unit after
a restart pays connect, authenticate (V3), capability probe and refresh; later
ones reuse the session. Every state read and every control write is still a live
LAN round-trip, because the protocol has no push or subscribe — so per-unit
latency of around 0.7 s is inherent, not a bug. Each unit has its own lock, so
concurrent requests to the same unit serialise instead of racing.

**A control command restates everything.** The protocol has no notion of "change
only the fan speed", so a partial request is read, merged and written — and the
unit's own echo is what comes back to the client. That is what makes an
optimistic UI honest: the client reconciles against what actually happened
rather than what it asked for.

**The scheduler shares the control path.** Schedules, curves and timers call the
same merge-and-apply as `POST /control`, which is why a scheduled change is
indistinguishable from a manual one, including in the event stream. One process,
one scheduler, so nothing fires twice.

**The poller idles until somebody subscribes.** The SSE broadcaster only polls
the units while at least one client is streaming, so a server nobody is watching
makes no LAN traffic at all.

## Where the panel lives

Compiled into the executable. A build script walks `static/` at the repo root
and generates a table of `(url path, content type, etag, bytes)` with
`include_bytes!` for each file, so the panel lives in `.rodata`, costs no
runtime allocation, and answers `304` for an unchanged file.

Generated rather than hand-listed on purpose: a file added to the panel cannot
be silently left out of the binary.

Two consequences worth knowing. There is no `static/` directory to edit in
place, so changing the panel means rebuilding. And there is no working
directory to get wrong — the binary runs from anywhere, which is what makes a
`FROM scratch` container possible at all.

## Extending it

The seams are deliberate; add work through them rather than widening a module.

- **A new endpoint** → add the `(method, path, guard, handler)` row to the route
  table and write the handler as `fn(&AppState, &Incoming) -> Reply`. The guard
  decides the credentials; the handler never checks them.
- **A new auth factor** → the guard enum and `breeze-auth`'s verifier are the
  only places that know how a request is identified. A route depends on "the
  guard passed", not on how.
- **A new protocol command** → `breeze-proto` is pure: functions from arguments
  to bytes, with no I/O, so they can be pinned against the reference
  implementation's own byte vectors. Do that — the one command frame that
  shipped without a vector is the one that later needed proving by hand.
- **A new store field** → add it to the model in `breeze-store` and keep the
  field order, because the order is the bytes.

## What the tests actually check

There is no integration harness and no mock server. What exists:

- **479 unit tests** across 18 suites, mostly in `breeze-proto` and
  `breeze-store`, where the
  logic is pure and the answers are byte vectors.
- **Reference byte vectors** for the protocol frames, taken from msmart-ng's own
  output rather than from our reading of it.
- **Fixture files** generated by the Python implementation, so the store layer is
  tested against bytes it did not produce.
- **`packaging/test/differential.py`**, which runs 4.x and a live 3.2.0 against
  the same config and diffs every endpoint — status, JSON type, shape and value.
  This is the test that catches a misunderstanding of the contract, which no
  amount of unit testing can: a unit test agrees with whatever the code does.
- **Package verification** per platform: every Linux package installed and
  started in a container of that distribution, the BSD packages installed on
  real machines, from the live repository URL, including that an untrusted
  signing key is refused.

`breeze-core diag` is the closest thing to an end-to-end test, and it needs real
hardware. See [Command-line tools](Command-line-tools).
