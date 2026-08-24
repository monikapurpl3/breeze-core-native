# Breeze Core Native

A native rewrite of [Breeze Core](https://github.com/monikapurpl3/breeze-core) —
the LAN-first REST API and web panel for Midea air conditioners — in Rust, with
Zig as the cross-linker.

**Status: 4.0.0 is built and packaged.** Breeze Core 3.2.0 is the last release of
the Python line, which is being sunset; its packages stay published and
installable, and everything after it happens here.

Every endpoint the Python server has — all 30 — verified against 3.2.0 running
side by side: 23 of 24 compared responses byte-identical (the exception is
documented), unit capabilities agreeing on all three real air conditioners, and
`breeze-core diag --auto` passing with no failures. The CLI answers every verb
the packaged Python binary answers, including `pair`, `devices` and `revoke`, and
takes the same flags the shell aliases in the wild pass. 473 tests.

**31 packages** — six Linux architectures × deb/rpm/pacman/apk/ipk, plus a
NetBSD one built on a NetBSD machine — installed in clean containers and checked
to run, to keep `/etc/breeze-core` on removal, and to install *over* the Python
package of the same name without losing a paired unit's credentials.
They come from **one static musl binary per architecture** — no libc dependency
is declared, because there is none to satisfy — and are served from
[aspic](https://aspic.salataputarica.hr.eu.org/) as five signed repositories
(apt, dnf/zypper, pacman, apk, opkg) plus an unsigned pkgin feed for NetBSD.
`packaging/` has the details, `site/` has the host.

| | size |
|---|---|
| the binary | 2.0–2.6 MB depending on architecture |
| an rpm / deb | ~1.4 MB (zstd) |
| the same thing in Python | 25.1 MB, 61.7 MB installed |

It also fixes what never worked here: **automatic pairing**. Broadcast discovery
found nothing because a reply to a broadcast matches no conntrack entry and gets
dropped, so this sweeps the local subnet by unicast as well — a scan now finds
every unit. Getting a *new* V3 unit's `token`/`key` is a harder problem that is
not ours: Midea has withdrawn token fetching from all but one of its apps, and
the one left only answers for the account the unit is registered to. There is a
last-resort path for that, and `POST /api/units` takes a `token` and `key`
directly, which is what keeps working when the API finally goes.

| build | size | what you give up |
|---|---|---|
| `cargo build --release` | **2.5 MB** | nothing |
| `--no-default-features` | **1.3 MB** | cloud pairing (the TLS stack is 1.2 MB of that) |


## Why

Breeze Core works, but it is a Python application, and that has a cost measured
in megabytes and architectures:

|  | Python today | native target | measured |
|---|---|---|---|
| package size | **25.0 MB** | **~2–2.5 MB** | 1.8 MB for the full dependency set incl. TLS |
| resident memory | **65 MB** | no runtime, no GC | — |
| riscv64 / s390x / ppc64le | frozen proof-of-concept | ordinary targets | 1.3–1.5 MB each |
| OpenWrt on 8–16 MB flash | documented as impossible | fits | — |

The bloat is not incidental. A self-contained bundle has to carry CPython, every
dependency, and a PyInstaller shim; that is what makes an 8 MB router impossible
and what makes each new architecture an emulated multi-hour build.

## Why Rust, and why Zig too

Both were measured rather than assumed. For an equivalent binary — AES, MD5,
HMAC, JSON, an HTTP server — Go produced 3.3–4.1 MB across 15 targets with no
toolchain at all; Rust produced 375–601 KB but could not even link musl on the
build host. `cargo-zigbuild` closes exactly that gap: Zig supplies libc and the
linker for every target, and can pin a glibc floor (`-target
x86_64-linux-gnu.2.17`) — which is what the Python build currently achieves by
compiling inside deliberately ancient containers.

The one gap is MIPS: Zig 0.16's bundled `mipsel` musl emits references to its own
standard-library internals that it then fails to provide. MIPS links with the
OpenWrt SDK toolchain instead, which this project already uses for `.ipk` builds.

## Layout

```
crates/breeze-proto     the Midea LAN protocol: framing, crypto, discovery, AC commands
crates/breeze-device    connections, retries, per-unit locking
crates/breeze-store     the four JSON store files, written byte-compatibly
crates/breeze-auth      API key, v1 bearer tokens, v2 Ed25519 signatures
crates/breeze-http      routes, guards, the SSE stream, the embedded panel
crates/breeze-cloud     one cloud round-trip for a V3 token (optional; pulls in TLS)
crates/breeze-core      the binary
static/                 the web panel, compiled into that binary by build.rs
tools/                  scripts that diff this server against the Python one
site/                   the aspic host: its pages, vhost and deployment
packaging/              binaries, packages, the signed repository tree
```

The protocol is four layers, wrapped one inside the next:

```
┌ lan     0x8370 session (V3 only) — AES-256-CBC + SHA-256
│ ┌ packet  0x5A5A V2 packet      — AES-128-ECB + MD5
│ │ ┌ frame   0xAA appliance frame — checksum
│ │ │ ┌ ac      command / state payload — CRC-8
```

Every layer is codec-only — bytes in, bytes out, no sockets — so all of it is
testable without hardware, and the transport can be swapped without touching the
protocol.

## Correctness

The conformance vectors in `crates/breeze-proto/tests/vectors.rs` are ported from
msmart-ng's own test suite: real captured messages with the values a known-good
implementation decodes from them. They are the closest thing this protocol has to
a specification.

That mattered. Two bugs were caught this way rather than in the field:

- the swing axis was **transposed** (`HORIZONTAL` is `0x3`, `VERTICAL` is `0xC`),
  which is invisible in review and shows up only as a unit waving the wrong flap;
- the V3 session key is **32 bytes, so it is AES-256**, not AES-128.

A third quirk is not a bug and cannot be tested for: a unit ignores its first
request after a handshake, so you must wait ~1 s or every command times out with
no error at all.

```bash
cargo test --workspace   # 473 tests, no hardware needed
cargo clippy --all-targets --workspace -- -D warnings
```

## What is deliberately not here

- **The commercial-appliance class (`0xCC`).** Not for want of hardware — the
  criterion is verifiability. An s390x build can be checked without a
  mainframe: cross-compile, run the suite under emulation, and being wrong
  fails in CI. A `0xCC` implementation could only be checked against
  msmart's vectors, never against reality, and being wrong fails silently in
  someone's building. The architecture keeps the door open: outside tests,
  only `frame::DeviceType` and the `ac` module know what an appliance is, so
  adding it is one enum variant and a module, not a refactor.
- **V1 devices.** They answer discovery with XML and need a separate TCP query.
- **A way to get a new V3 unit's credentials that does not involve Midea.**
  Not for want of trying. A bare V2 packet to a V3 unit gets no reply, so there
  is no downgrade path; the unit never reveals its own key; msmart's shared
  accounts are refused by every cloud that still answers; and the one API left
  standing only issues a token to the account the unit is registered to. So
  `breeze-cloud` asks for that account, uses it once, and forgets it -- and
  `POST /api/units` takes a `token` and `key` directly, which is the path that
  survives Midea finishing the job. Back up your `config.json`.

Reimplementing the protocol means owning the device quirks that msmart-ng
collects upstream for hardware we do not have. That is a deliberate trade, taken
with eyes open, and quirks get handled as they surface.

## Licence

AGPL-3.0-or-later, matching Breeze Core. The protocol layer is a reimplementation
informed by [msmart-ng](https://github.com/mill1000/midea-msmart) (MIT) and its
test vectors, and by [MideaUART](https://github.com/dudanov/MideaUART) for the
temperature encoding.
