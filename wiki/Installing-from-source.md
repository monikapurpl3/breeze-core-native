# Installing from source

For an OS, an architecture or an init system with no package — or because you
would rather build it yourself.

You get one executable. Where it goes and how it starts is then up to you, and
this page covers both.

## What you need

- **Rust**, a recent stable. There is no `rust-toolchain.toml` pinning a
  version, and nothing here uses a nightly feature.
- **A C linker**, which your platform's usual build tools provide.
- **Nothing else.** No Python, no Node, no system libraries to find, no
  `pkg-config`. The panel is generated at build time by a build script that
  walks `static/`, and the TLS roots are compiled in.

## Build

```sh
git clone https://github.com/monikapurpl3/breeze-core-native
cd breeze-core-native
cargo build --release
```

The binary lands at `target/release/breeze-core`, around 2.5 MB. Install it
wherever your system expects one:

```sh
sudo install -m 0755 target/release/breeze-core /usr/local/bin/breeze-core
```

The release profile is tuned for size — `opt-level = "z"`, LTO, one codegen
unit, `panic = "abort"`, stripped — which is what gets a whole daemon into
about two megabytes. A `cargo build` without `--release` produces something
much larger and slower, and is only for development.

Run the tests if you like: `cargo test --workspace`, 480 of them across 18
suites, no hardware or network needed.

## Set it up

Same as any other install:

```sh
sudo mkdir -p /etc/breeze-core
sudo useradd --system --no-create-home --home-dir /etc/breeze-core \
  --shell /sbin/nologin breeze
sudo chown breeze:breeze /etc/breeze-core && sudo chmod 750 /etc/breeze-core

sudo breeze-core pair          # finds the units, writes config.json
```

Then pick an init template from **`deploy/init/`** in the repository —
**runit**, **s6**, **SysV**, **supervisord** and a macOS **launchd** daemon,
alongside the systemd unit, OpenRC service, procd script and three BSD rc
scripts under `packaging/`. Each names its own install steps at the top.

All of them read `/etc/breeze-core/breeze-core.env`, set one `AC_CONFIG_DIR`
rather than four separate paths, and set **no working directory** — the panel
is inside the binary, so there is nothing to be relative to.

If you are writing your own, three things matter:

1. **Pass `serve` explicitly.** A bare `breeze-core` prints its usage and
   exits 0, which as a service start command looks like success and leaves you
   with no server.
2. **Run it in the foreground** and let your supervisor own it. There is no
   `--daemonize`.
3. **Plain `TERM` to stop it.** Every store is written to a temporary file and
   renamed into place, so no signal can catch one half-written. Nothing traps
   `INT` specially.

## Cross-compiling

The release binaries are cross-built on one machine with
[`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild), which uses
Zig as the linker. No emulator is involved:

```sh
cargo install cargo-zigbuild
rustup target add aarch64-unknown-linux-musl
cargo zigbuild --release --target aarch64-unknown-linux-musl
```

`./packaging/build-binaries.sh` does every published target, or one by name.
See [Ports and architectures](Ports-and-architectures) for the target list and
for the one exception — s390x, which is built against glibc because statically
linking glibc breaks `getaddrinfo`.

**musl targets are static by default**, so do not add `+crt-static` yourself.
Forcing it on a *glibc* target produces a binary that dies inside
`getaddrinfo`, and only when a hostname is resolved — which is a long way from
where you would look.

## An architecture with no prebuilt `std`

MIPS and other tier-3 targets need `-Z build-std` on a nightly toolchain. That
is a well-trodden route, but it is the only case where the toolchain
requirement above stops being "recent stable". The traps are written up in
[Ports and architectures](Ports-and-architectures).

## On the BSDs

Build natively; there is no cross-compilation story, because Zig bundles no BSD
libc. `cargo build --release` works as-is on FreeBSD, NetBSD and OpenBSD — the
whole dependency set compiles on all three, TLS included — and that is how the
published packages are made.

`~/.cargo` on a fresh NetBSD install is sometimes root-owned; set `CARGO_HOME`
if cargo complains it cannot write there.

## On macOS

Builds and runs. There is no package, so use the launchd template at
`deploy/init/com.breeze.core.plist` — a LaunchDaemon rather than a
LaunchAgent, because an agent only runs while its user is logged in, which is
the wrong lifetime for something a phone talks to.

## What you are actually compiling

**Fourteen** external direct dependencies, 142 crates in the lock file once
everything transitive is counted:

`aes` · `base64` · `chrono` · `ed25519-dalek` · `flate2` · `getrandom` ·
`md-5` · `serde` · `serde_json` · `sha2` · `sha3` · `subtle` · `tiny_http` ·
`ureq`

The absences are as deliberate as the entries: **no async runtime, no web
framework, no TLS server.** `tiny_http` is a small synchronous HTTP server run
on a thread pool, and TLS is somebody else's job — see
[Architecture](Architecture) and
[Reverse proxy and TLS](Reverse-proxy-and-TLS).

`ureq` is the odd one out, and the only reason anything here can make an
outbound connection: it fetches a V3 unit's credentials from the vendor cloud,
once, during pairing. It sits behind the **`cloud`** feature, which is on by
default:

```sh
cargo build --release --no-default-features
```

That drops the whole TLS client stack — about **1.2 MB**, which is the
difference between fitting on a router and not — and leaves a server that
never speaks to anything but your air conditioners. If your units are V1/V2,
or you already have their credentials in `config.json`, you lose nothing.

## Verifying your build

```sh
breeze-core --version      # version and the commit it came from
breeze-core diag           # against a running server
```

`--version` reports the git commit, and appends `-dirty` if the tree had
uncommitted changes — worth checking if you are unsure which build you are
running.
