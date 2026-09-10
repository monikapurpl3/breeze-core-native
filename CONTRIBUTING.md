# Contributing to Breeze Core

Patches, device reports and documentation fixes are all welcome. It is a hobby
project, so replies may take a few days.

## Building it

```sh
cargo build --release
```

That is the whole of it — a recent stable Rust and a C linker, nothing else.
No Python, no Node, no system libraries to find. The web panel is generated
into the binary by a build script that walks `static/`.

Running it against a local configuration, without touching a real install:

```sh
mkdir -p /tmp/bc && printf '%s' '{"api_key":"dev","units":[]}' > /tmp/bc/config.json
AC_CONFIG_DIR=/tmp/bc cargo run -- serve --host 127.0.0.1 --port 8420
```

`BREEZE_DEBUG=1` traces every control command. `BREEZE_DEBUG_AUTH=1` dumps the
canonical string an auth v2 signature was checked against, which is the only
practical way to debug a signature mismatch.

## Before you open a PR

CI runs three checks. **Run them with `+stable`**, because this is the
toolchain CI uses and clippy lints differ between versions — a clean clippy on
nightly tells you nothing:

```sh
cargo +stable test --all-features --workspace
cargo +stable clippy --all-targets --workspace -- -D warnings
cargo +stable fmt --all --check
```

If `cargo +stable clippy` says the component is missing:

```sh
rustup component add clippy rustfmt --toolchain stable
```

## Conventions, please match these

- **Preserve the wire contract.** The `/api/*` JSON shapes, the status codes
  and the four store formats are shared by the server, the panel, the Android
  app, the CLI and anything anyone has scripted. Add to it freely; change it
  only with a reason that outweighs breaking a client.
- **Enum values are names.** `"COOL"`, never `2`. A client that receives a bare
  integer breaks silently, so `breeze-core diag` checks it on every run.
- **Unit ids are strings.** Midea ids exceed 2^53, so a JSON number is silently
  rounded by every JavaScript client on earth.
- **A bad enum is `400`, an out-of-range number is `422`.** That split is
  inherited rather than designed, and clients branch on it.
- **Add work through the seams.** Each crate either has no dependency on the
  others or exactly one direction of dependency; nothing is circular and
  nothing reaches for global state. See
  [Architecture](https://github.com/monikapurpl3/breeze-core-native/wiki/Architecture).
- **Comments say *why*, not *what*.** The code says what. A comment earns its
  place by recording a constraint, a trap, or a decision somebody would
  otherwise undo — several here exist because the obvious thing was tried and
  did not work.
- **No `unsafe`**, and no new dependency without a reason. Fourteen external
  crates is a feature of this project, not an accident.

## Testing

480 tests across 18 suites, none of which need hardware or a network — the
protocol layer is pure codec, bytes in and bytes out.

Two kinds are worth knowing about:

- **Reference vectors** (`crates/breeze-proto/tests/vectors.rs`) are captured
  messages with the values a known-good implementation decodes from them. They
  are the closest thing this protocol has to a specification, and they have
  caught real bugs that review did not — a transposed swing axis, and a session
  key that is AES-256 rather than AES-128. **Check protocol changes against
  these, not against your reading of the spec.**
- **`packaging/test/differential.py`** runs this server and a Python 3.2.0 side
  by side against the same configuration and diffs every endpoint. It catches a
  *misunderstanding* of the contract, which no unit test can, because a unit
  test agrees with whatever the code does.

`breeze-core diag` is the end-to-end check and it needs real units. Note its
last check POSTs an out-of-range temperature expecting a `422` — nothing
reaches the unit unless that bounds check is broken, which is the point of it,
but it is worth knowing before you run it against a live system.

## Packaging

Each packager has a `verify-*.sh` beside it that installs the result in a
container of the target distribution and exercises it. Run the one you touched.

Two traps that have each cost real time:

- **Nothing may hardcode the package release** — the `-N` in `4.0.2-2`. It has
  been wrong in three places, and the worst was silent: a glob that matched
  nothing, with a `continue` that skipped every architecture and published an
  empty repository while reporting success.
- **Every packager refuses to wrap a binary older than the source.** If it
  tells you to rebuild, rebuild; the guard exists because a `.deb` once went
  into a verification container carrying a CLI three commits old.

Full flow:
[Building and releasing](https://github.com/monikapurpl3/breeze-core-native/wiki/Building-and-releasing).

## Security

Please report vulnerabilities privately — see [SECURITY.md](SECURITY.md), not a
public issue.

## Licence

By contributing you agree your work is licensed under AGPL-3.0-or-later, the
same as the project.
