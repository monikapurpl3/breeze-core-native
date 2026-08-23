# CLAUDE.md

Guidance for Claude Code working in this repository.

## What this is

The native rewrite of [Breeze Core](https://github.com/monikapurpl3/breeze-core) —
a LAN-first REST API and web panel for Midea air conditioners — in **Rust**, with
**Zig as the cross-linker**. It ships as **v4.0.0** and must be a **drop-in
replacement**: an existing installation is pointed at it and must keep working,
with the same store files, the same environment variables, and no re-pairing.

The Python implementation stays in production until this reaches parity. It is
also the reference: when in doubt about behaviour, read `../breeze-core` rather
than inventing an answer.

**Why:** Python packages are 25 MB and 65 MB resident, which is what makes
riscv64/s390x/ppc64le frozen proof-of-concept tiers and an 8–16 MB OpenWrt router
impossible. The native server binary is currently **815 KB**.

## Architecture

Six crates, each depending only on those above it. The layering is the point: the
protocol knows nothing about sockets, and the transport knows nothing about
appliances.

| crate | owns | knows about |
|---|---|---|
| `breeze-proto` | framing, crypto, discovery, AC commands | bytes only — **no sockets** |
| `breeze-device` | connections, retries, per-unit locking | sockets, `breeze-proto` |
| `breeze-store` | the four JSON store files | serde only |
| `breeze-auth` | API key, v1 bearer, v2 Ed25519 | `breeze-store` — **no HTTP** |
| `breeze-http` | routes, guards, response shapes | all of the above |
| `breeze-core` | the binary | `breeze-http` |

The protocol itself is four layers wrapped one inside the next:

```
┌ lan     0x8370 session (V3 only) — AES-256-CBC + SHA-256
│ ┌ packet  0x5A5A V2 packet      — AES-128-ECB + MD5
│ │ ┌ frame   0xAA appliance frame — checksum
│ │ │ ┌ ac      command / state payload — CRC-8
```

Keeping `breeze-proto` and `breeze-auth` free of I/O is what makes them testable
without hardware or a network. Do not add a socket to either.

## Commands

```bash
cargo test --workspace                                   # no hardware needed
cargo clippy --all-targets --workspace -- -D warnings    # CI gate
cargo fmt --all --check

# Cross-compile. zig supplies libc and the linker for every target.
cargo +nightly zigbuild --release --target x86_64-unknown-linux-musl --workspace

# Run against a real deployment, on a spare port so the Python service keeps 8420
AC_CONFIG=./config.json BREEZE_PORT=8421 cargo run -p breeze-core

# The acceptance test: the project's own diagnostic, pointed at this server
breeze-core diag --base-url http://127.0.0.1:8421 --auto
```

Toolchain: **nightly** (needed for `-Zbuild-std` on tier-3 targets), **Zig
0.16.0 pinned** in CI, and `cargo-zigbuild`.

## The one rule that has caught every real bug

**Verify against something external, never against your own fixtures.** Fixtures
test your understanding; only an outside reference tests the code. Everything
found so far came from an external check, and nothing was found by a test written
from the same reading as the code:

- the swing axis was transposed — caught by diffing live output against the Python service;
- the V3 session key is 32 bytes (AES-256, not 128) — caught by real hardware refusing every handshake;
- `Program.id` serialises **last** — caught by round-tripping pydantic's own bytes;
- serde_json's default float parser is off by 1 ULP — caught by the real `devices.json`, not by my fixtures, which passed;
- `/api/units/state` is an envelope, not an array — caught by `breeze-core diag`.

So: msmart's test vectors for the protocol, pydantic-generated fixtures for the
stores, Python-generated signatures for auth, and `diag` for the HTTP surface.

## Conventions and gotchas specific to this repo

### Wire contract — three unchanged clients depend on it

- **`id` is a string**, not a number. Breeze Core serialises `str(unit.id)`; the
  ids are 48-bit and clients put them in URLs. A JSON number is valid and breaks
  every client.
- **Enums serialise as names** (`"COOL"`, never `2`). Python 3.11 changed
  `IntEnum.__str__` to render a bare int and that bit the project once.
- **`GET /api/units/state` is an envelope**: `{"states": [...], "errors": [...]}`.
  One unreachable unit lands in `errors` while the rest still arrive, so an
  unplugged AC never 503s the batch.
- An unrecognised mode or swing value becomes `"UNKNOWN"`. Never coerce to the
  first variant — confidently wrong is worse than visibly unknown.
- The auth **reason codes** and the `retryable` flag are wire contract. Exactly
  `clock_skew`, `replay` and `incomplete_signature` are retryable. They exist
  because a drifted clock once produced a 401 indistinguishable from a revoked
  credential, so the app deleted its key and stranded users.
- **`FEATURES` must describe reality, not ambition.** Clients feature-detect on
  it; advertising `live_stream` before SSE exists makes every client open a
  stream that never arrives.
- **Never add a CORS header.** The panel is same-origin, and a permissive policy
  would let any other LAN page drive this API. Its absence is load-bearing and
  there is a test for it.

### Store files must round-trip byte-identically

A migration nobody can reverse is not trustworthy. `cargo test -p breeze-store`
compares **bytes**, not values, against fixtures generated by Breeze Core's own
pydantic models.

- **`serde_json` needs the `float_roundtrip` feature.** Its default parser is off
  by one ULP, and `devices.json` stores every timestamp as a float. Without it
  the server rewrites the file on every save.
- `None` serialises as an explicit `null`, never skipped.
- 2-space indent, **no trailing newline** (`model_dump_json(indent=2)` +
  `write_text`).
- `Program.id` comes last, because pydantic appends subclass fields.
- `config.json` is mode **640** so the admin CLIs work without `sudo` (2.4.3
  changed it from 600 for exactly that reason); the other three are 600.

### Protocol traps

- **Key sizes differ per layer.** The packet layer uses a fixed 16-byte key
  (AES-128-ECB); a V3 session uses the device's 32-byte cloud key
  (AES-256-CBC, zero IV).
- **A unit ignores its first request after a handshake.** Wait
  `lan::POST_HANDSHAKE_SETTLE_MS` (~1 s) or every command times out looking
  exactly like an unreachable unit.
- `SwingMode` is **`HORIZONTAL = 0x3`, `VERTICAL = 0xC`**. Easy to transpose,
  invisible in review.
- Discovery and packet parsing read bytes straight off a network. Every parser
  has a truncation-fuzz test; keep it that way.

### Auth ordering is load-bearing

- **Verify the signature before spending the nonce.** Otherwise anyone can burn a
  victim's next nonce with a forged request and lock out the real client.
- **Check the clock before the signature**, so a drifted phone gets `clock_skew`
  with the server's own time and can re-sign, rather than a bare rejection.
- A device is pinned to its scheme: a v2 device cannot fall back to a bearer
  token, a v1 device cannot present a signature. No silent downgrade.

### Rust-specific

- **`env!("CARGO_PKG_VERSION")` inside a library reads the *library's* manifest.**
  `/api/version` advertised `0.1.0` because of this; the binary passes its own
  version into `AppState`.
- **Do not derive `PartialEq` on an enum holding a function pointer** — it
  compares addresses. Match on the variant in tests instead.
- The batch route must **fan out**, one thread per unit. Per-unit locks exist for
  exactly that; serially it measured 5.4 s against Python's 1.8 s.
- Blocking I/O and `tiny_http`, deliberately. A request spends ~1.8 s waiting on
  a LAN round-trip and there are a handful of clients, so an async runtime would
  cost ~1 MB of binary and buy nothing.

### Cross-compilation

- **FreeBSD and OpenBSD cross-compile**; OpenBSD is tier 3 and needs
  `-Zbuild-std`.
- **NetBSD needs the real VM** — `cargo-zigbuild` refuses it, as zig ships no
  NetBSD libc.
- **MIPS links with the OpenWrt SDK toolchain**, not zig: zig 0.16's bundled
  `mipsel` musl emits references to its own std internals it then fails to
  provide.
- Pin the Zig version in CI. `setup-zig@v1` could not fetch 0.16 at all — Zig
  renamed release artifacts (arch before OS, under `/download/` not `/builds/`).

## Deliberately not implemented

- **The commercial-appliance class (`0xCC`).** Not for want of hardware — the
  criterion is *verifiability*. An s390x build is checkable under emulation and
  fails in CI; a `0xCC` implementation could only be checked against msmart's
  vectors and would fail silently in someone's building. The architecture keeps
  the door open: outside tests only `frame::DeviceType` and the `ac` module know
  what an appliance is.
- **V1 devices** (XML discovery, separate TCP query).
- Still to come in Phase 3: `/api/auth/*` enrolment, programs, timers, SSE, the
  embedded panel.

## The README is not the shape to ship

The current README is a working document written while the repo was private. Before
going public it must be rewritten to mirror **breeze-core's** README, with **one
short line** (two at most) about how this differs and the bulk in the wiki. To a
visitor this *is* Breeze Core; the rewrite is a footnote about implementation, and
the point is that users migrate to it rather than adopt an experiment.
