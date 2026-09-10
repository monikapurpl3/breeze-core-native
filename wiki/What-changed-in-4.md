# What changed in 4.x

4.x is the same project, rewritten in Rust. **This page is the only one that
talks about the difference** — everywhere else on this wiki describes Breeze
Core as it is now. If you are actually moving an installation across, you want
[Upgrading to 4.x](Upgrading-to-4).

The short version: nothing you interact with changed, and quite a lot about what
it costs to run did.

## What did not change

Worth listing first, because it is most of it.

- **The REST API.** Same paths, same JSON, same status codes. A client cannot
  tell which implementation answered except by asking `GET /api/version`.
- **The store files.** `config.json`, `devices.json`, `programs.json` and
  `timers.json` are read and written in the same format, byte for byte.
- **The `AC_*` environment variables.** Your existing env file works untouched.
- **The service name.** Still `breeze-core`.
- **The web panel.** The same panel, the same six colour palettes.
- **The Android app.** Unchanged, and it does not know the difference.
- **The package name**, so upgrades are upgrades.

That is the whole point of the exercise. The rewrite was meant to be invisible
from the outside.

## What it costs to run

Measured on one x86-64 server with the same three units paired, before and after
an in-place upgrade:

| | 3.2.0 | 4.x |
|---|---|---|
| resident memory | ~62 MB | **~2.4 MB** |
| installed on disk | ~59 MB | **~3 MB** |
| package dependencies | `python312` and friends | **none** |
| what installs | an interpreter and ~40 libraries, or a ~25 MB bundle | one ~2.5 MB executable |

What `dnf` said while doing it:

```
Total size of inbound packages is 1 MiB. Need to download 1 MiB.
After this operation, 56 MiB will be freed (install 3 MiB, remove 59 MiB).
```

On a router or a Pi that is the difference between "fits" and "does not".

## Where it runs now

| | 3.2.0 | 4.x |
|---|---|---|
| Linux packages | amd64, arm64, armhf | those **plus riscv64, ppc64le, s390x** |
| FreeBSD | a signed `pkg` repository | the same |
| NetBSD | a `pkgin` feed | the same |
| OpenBSD | source install only | **a signed package** |
| OPNsense | a plugin bundling an interpreter | a plugin with no dependencies |
| containers | five images, 137–291 MB on disk | two, **5.9 MB** |

riscv64, ppc64le and s390x were proof-of-concept builds before — built under
emulation, never published. They are ordinary published packages now. See
[Ports and architectures](Ports-and-architectures).

## New in 4.x

- **`breeze-core control`** — set a unit from the shell without `curl` and a
  JSON body: `breeze-core control 'living room' heat 22`.
- **`breeze-core login`** — enrol a machine's own CLI as a device, so `diag` and
  `control` need no flags afterwards.
- **An access log**, on by default: one line per request with its status and how
  long it took.
- **`BREEZE_DEBUG=1`** — traces every control command from what the client sent
  through to what the unit echoed back.
- **A `-debug` container image** with a shell in it, for when `docker logs` is
  not enough.

## Gone in 4.x

Stated plainly, because "drop-in replacement" should not have quiet exceptions.

- **`/docs`.** The interactive API documentation came from the Python web
  framework; there is no framework now, so there is no generated schema.
  [REST API](REST-API) is the reference and it is complete.
- **`ac-diag.zsh` and `ac-approve.zsh`.** Both are subcommands now
  (`breeze-core diag`, `breeze-core approve`). The flags they took are still
  accepted so existing scripts keep working.
- **Brotli response compression.** gzip only. Every client in play sends `gzip`,
  and brotli was several hundred kilobytes of binary for a few percent on
  documents this size.
- **Three of the five container images.** Two replace them; the bundled-nginx one
  becomes a compose file with a real proxy container, and the Red Hat UBI ones
  had no purpose left once there was no glibc to patch.
- **`docker exec breeze-setup`.** The container has no shell. `docker run <image>
  pair` does the same job.
- **Rate limiting on the enrolment endpoints.** 3.x throttled them per source
  address; 4.x does not. A pairing code is still single-use, 40 bits, and dead
  in 60 seconds, so guessing it is improbable — it is just no longer also slow.
  Put a limiter in your reverse proxy if the server is exposed.
- **The switch for LAN-only admin approval.** Approving a pairing, listing
  credentials and revoking one now always require a private source address.
  It was a setting whose only use was making a worse configuration possible.

## Under the bonnet

Only interesting if you are curious or contributing — none of it changes how you
use Breeze Core. [Architecture](Architecture) has the detail.

- **The Midea LAN protocol is implemented directly**, rather than through the
  `msmart-ng` Python library: the framing, the V3 handshake, the encryption and
  discovery. It is checked against that library's own byte output, so the two
  agree about what goes on the wire.
- **The web panel is compiled into the executable.** There is no directory of
  files to deploy, and no working directory to get wrong.
- **No async runtime, no web framework, no TLS server**, and fourteen external
  libraries in total. A reverse proxy still terminates TLS, exactly as before.

## Why the major version moved

The implementation changed; the contract did not. The bump is a warning about
what you are installing, not about what you will have to change — which is why
4.x can be installed over 3.x with your configuration left alone.

Release-by-release detail: [Version history](Version-history).
