# Breeze Core Native

A native rewrite of [Breeze Core](https://github.com/monikapurpl3/breeze-core) —
the LAN-first REST API and web panel for Midea air conditioners — in Rust, with
Zig as the cross-linker.

**Status: early. Phase 1 of 5.** This repository currently contains
`breeze-proto`, the protocol layer. There is no server here yet; the Python
implementation remains the one in production.

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
cargo test        # 62 tests, no hardware needed
cargo clippy --all-targets
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
- **Cloud pairing.** Coming, in this same binary; it needs TLS.

Reimplementing the protocol means owning the device quirks that msmart-ng
collects upstream for hardware we do not have. That is a deliberate trade, taken
with eyes open, and quirks get handled as they surface.

## Licence

AGPL-3.0-or-later, matching Breeze Core. The protocol layer is a reimplementation
informed by [msmart-ng](https://github.com/mill1000/midea-msmart) (MIT) and its
test vectors, and by [MideaUART](https://github.com/dudanov/MideaUART) for the
temperature encoding.
