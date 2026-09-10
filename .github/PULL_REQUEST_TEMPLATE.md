<!-- Thanks for the patch. Delete anything below that does not apply. -->

## What this changes

<!-- And why. If it fixes an issue, "Fixes #123". -->

## Checks

CI runs three things and they all have to pass. **Run them against `stable`,
not your default toolchain** — clippy lints differ between versions, so a clean
run on nightly says nothing about CI:

```sh
cargo +stable test --all-features --workspace
cargo +stable clippy --all-targets --workspace -- -D warnings
cargo +stable fmt --all --check
```

- [ ] those three pass locally
- [ ] new behaviour has a test, and the test would fail without the change

## If it touches the wire contract

The `/api/*` shapes, the status codes and the four store formats are shared
with the Android app, the web panel, the CLI and whatever anyone has scripted.

- [ ] this adds to the contract rather than changing it — or the description
      says what breaks and why that is worth it
- [ ] enum values still come back as **names** (`"COOL"`, never `2`)
- [ ] unit ids are still **strings** (they exceed 2^53, so a JSON number is
      silently rounded by every JavaScript client)
- [ ] a bad enum is still `400` and an out-of-range number still `422`

## If it touches packaging

- [ ] the relevant `verify-*.sh` passes
- [ ] nothing hardcodes the package release (the `-N` in `4.0.2-2`) — that has
      been wrong in three places so far

## If it touches the protocol

- [ ] checked against the reference vectors in
      `crates/breeze-proto/tests/vectors.rs` rather than against a reading of
      the spec

## Anything you could not check

Worth saying plainly. "Builds but I have no armv7 to run it on" is useful;
silence is not.
