# Plan: distroless containers, and moving one to another host

**Status: built, tested, and published privately to
`ghcr.io/monikapurpl3/breeze-core-native`.** This began
as a plan written before any Dockerfile existed; the decisions below were then
made by the maintainer and the images built against them. Sizes and behaviour
in here are measured on the real images, not projected.

## Decisions

| Question | Answer |
|---|---|
| Registry | **ghcr.io**, as the Python line used |
| Images | **distroless + debug**, two tags |
| tzdata | **the entire set**, not trimmed |
| Architectures | **amd64 and arm64 only** |

The full tzdata turns out to cost far less than feared -- Alpine's `/usr/share/
zoneinfo` is **1.5 MB** with no `posix/` or `right/` duplicate trees -- so
"ship all of it" removes a whole class of wrong answer for about a third of the
binary's size. That was the right call and the trimming discussion below is
kept only because it records why the size mattered.

## What the Python line shipped, and why it had to

Five images:

| Tag | Base | libc | Arch | Why |
|---|---|---|---|---|
| `latest` = `alpine-edge` | Alpine Edge | musl | amd64 + arm64 | the default |
| `alpine-edge-x86_64` / `-aarch64` | Alpine Edge | musl | one each | pinned |
| `alpine-edge-nginx-x86_64` | Alpine Edge | musl | amd64 | HTTPS out of the box |
| `ubi9-x86-64-v2` | Red Hat UBI 9 | glibc | amd64 | vendor-patched glibc base |
| `ubi9-x86-64-v3` | Red Hat UBI 9 | glibc | amd64 | the same, AVX2-era CPUs |

Every one of those exists because **an interpreter needs a distribution around
it**. The choice of base was a real choice: musl or glibc changed which wheels
would install, `-v2` versus `-v3` changed which CPUs the compiled extensions
would run on, and the whole matrix was downstream of "Python must be able to
import a `.so`".

None of that is true any more. The Linux binary is **static musl, ~2.7 MB, with
no libc at runtime** — the same file already installs on Debian 12, Alpine and
RHEL 8 from one package. So the base image is no longer a compatibility
decision. It is only a question of what else you want in the container.

## What the binary actually needs at runtime

Measured, not assumed:

| Need | Answer | Consequence |
|---|---|---|
| libc | none — static musl | `FROM scratch` is viable |
| TLS roots for cloud pairing | **bundled into the binary** (`webpki-roots`, and zero `rustls-native-certs`) | no `ca-certificates` layer |
| the web panel | compiled in | no asset layer |
| `/etc/passwd` | not needed if run as a numeric UID | no user layer |
| **timezone data** | **required, and its absence is silent** | see below |

### The one that bites: tzdata

Everything scheduled here uses the server's local clock — schedules, curves,
timers. With `TZ=Europe/Zagreb` set and no `/usr/share/zoneinfo`, a scratch
container reports:

```
timezone='+00:00'  utc_offset_seconds=0
```

It does not fail, warn, or fall back loudly. **It runs every schedule two hours
off** and looks perfectly healthy doing it. This is the single most important
thing this plan has to get right, because it is a silent wrong answer rather
than a crash.

Three options:

1. **Copy the whole tzdata** into the image (~3 MB zoneinfo, i.e. it would
   *double* the image to save one variable). Rejected on those grounds alone.
2. **Copy only the zone the operator asks for.** Cannot be done at build time —
   the zone is a runtime choice.
3. **Copy the whole thing anyway**, because measuring it first showed the
   premise of (1) was wrong: it is 1.5 MB, not 3 MB, and there are no duplicate
   `posix/`/`right/` trees on Alpine. ← **chosen, and verified working.**

With the full set in the image, `TZ=Europe/Zagreb` reports
`utc_offset_seconds: 7200` from a `scratch` container. The trap is closed for
every zone rather than for the ones somebody guessed at.

**A startup warning is still worth having**, and is now the only outstanding
item from this section: `TZ` set to something unresolvable -- a typo, or a
volume mounted over `/usr/share/zoneinfo` -- still silently means UTC. It is a
two-line check and converts a silent wrong answer into a log line.

## Proposed image matrix

Two images, not five.

| Tag | Base | Contents | Measured |
|---|---|---|---|
| `4.0.1`, `latest` | `scratch` | the binary, full zoneinfo, `/etc/breeze-core` | **5.88 MB** |
| `4.0.1-debug` | `busybox:1.36-musl` | the same, plus a shell and `tar` | **8.36 MB** |

Multi-arch manifest for **amd64 and arm64**. Both build, and the arm64 image was
run under qemu on the build host to confirm it starts and reports its version --
which is more than the Python line's per-architecture images ever got.

Against the images this replaces, still present on the build host to compare:

| | Size |
|---|---|
| `4.0.1` distroless | **5.88 MB** |
| `4.0.1-debug` | 8.36 MB |
| `alpine-edge-x86_64` | 137 MB |
| `alpine-edge-nginx-x86_64` | 140 MB |
| `ubi9-x86-64-v2` / `-v3` | 291 MB |
| `v3.0.5` | 305 MB |

**23× smaller than the Alpine image it replaces, 49× smaller than UBI.**

### What happens to the other three

- **`alpine-edge`** → replaced by `distroless`. Nothing it provided is needed.
- **`ubi9-*-v2` / `-v3`** → **dropped.** They existed for a vendor-patched
  *glibc*, and there is no glibc here to patch. Confirmed as a libc decision
  rather than a Red Hat policy one, so nothing replaces them.
- **`alpine-edge-nginx`** → **dropped, with a replacement suggested rather than
  bundled.** Bundling nginx put two processes in one container, needed a
  supervisor, generated a certificate on first boot, and was the one image that
  could not run with a read-only rootfs. A compose file pairing the distroless
  image with an official `nginx` or `caddy` image does the same job with each
  process in its own container. Ship `docker-compose.https.yml`, not a second
  image.

### What is lost, honestly

- **`docker exec breeze-setup` stops working.** There is no shell. The
  replacement is `docker run --rm -it --network host -v breeze-config:/etc/breeze-core
  <image> pair`, which runs the pairing tool as the container's entrypoint
  argument. That is arguably better — it is one command instead of an
  interactive wrapper — but it is a documented change, not a transparent one.
- **No `sh` for poking around.** Hence the `debug` tag. Same binary, same
  digest for the binary layer, a shell added.
- **`docker logs` is the only introspection.** Which is why the access log
  added in 4.0.1 matters more here than anywhere else.

## Pairing needs the LAN, and containers do not have one

Unchanged from the Python line and worth restating because it is the most
common container problem: **discovery is a UDP broadcast**, so it needs the
host's layer 2. `--network host` for the pairing run at minimum. After pairing,
the server only makes unicast TCP connections to known addresses, so a bridge
network works for normal operation — as long as the units are routable from it.

## Backing up a container deployment

The whole state is four files in one directory. That is the entire backup story
and it is worth saying plainly, because "back up your containers" invites people
to snapshot images instead of data.

```
/etc/breeze-core/config.json     the API key and the units, INCLUDING V3 credentials
/etc/breeze-core/devices.json    enrolled clients
/etc/breeze-core/programs.json   favourites, schedules, curves
/etc/breeze-core/timers.json     pending one-shot timers
```

**`config.json` is the one that cannot be regenerated.** A paired V3 unit's
token and key are issued once by a vendor cloud that no longer hands them out.
Lose it and the unit has to be re-paired through the vendor app.

### The backup command

```sh
docker run --rm \
  -v breeze-config:/data:ro \
  -v "$PWD":/out \
  busybox tar czf /out/breeze-backup-$(date +%F).tar.gz -C /data .
```

A sidecar rather than `docker cp`, so it works whether the volume is named, a
bind mount, or inside a compose project — and `:ro` so a backup cannot corrupt
what it is copying. The archive holds an API key and V3 credentials: it wants
`chmod 600` and somewhere that is not the same disk.

**Stopping the container first is optional but tidier.** The stores are written
with a create-and-rename, so a backup taken mid-write gets the old file rather
than a torn one — but a scheduler tick landing between two files can still give
you a `timers.json` from after the `programs.json`. For a nightly cron that does
not matter. Before an upgrade, stop it.

### Restore

```sh
docker run --rm -v breeze-config:/data -v "$PWD":/in \
  busybox tar xzf /in/breeze-backup-2026-09-08.tar.gz -C /data
```

Then check the modes: `config.json` should be `0640` and the rest `0600`. The
server tightens them on its next write anyway, which is deliberate — a restore
from a loose archive gets fixed rather than staying loose.

## Migrating a container to another host

Three cases, in increasing order of how much anyone should trust them.

### 1. Same architecture, new host — copy the volume

```sh
# on the old host
docker run --rm -v breeze-config:/data:ro busybox tar cz -C /data . \
  | ssh newhost 'docker run --rm -i -v breeze-config:/data busybox tar xz -C /data'
```

Streamed rather than staged, so the credentials never touch either host's disk
outside the volume. Then start the image on the new host and it comes up with
the same units, the same key, and the same enrolled clients.

**The unit addresses travel with it.** `config.json` holds each unit's LAN IP,
so moving to a different network means editing them — or re-running `pair`,
which rewrites the addresses while keeping the credentials.

### 2. Different architecture — the same volume, a different image

The store files are architecture-independent JSON, so an amd64 → arm64 move is
case 1 plus pulling the arm64 image. Nothing in the format is endian- or
word-size-dependent. This is worth stating because the Python line's images were
per-architecture in a way that made people nervous about it.

### 3. From the Python images to distroless

The interesting one, and the reason the format compatibility in `breeze-store`
exists at all.

```sh
docker stop breeze-core
# the volume is unchanged -- 3.x and 4.x read and write the same four files
docker run -d --name breeze-core \
  -v breeze-config:/etc/breeze-core \
  -e TZ=Europe/Zagreb \
  --network host \
  ghcr.io/monikapurpl3/breeze-core-native:distroless
```

No conversion step, because there is nothing to convert. What to check
afterwards, in this order:

1. `docker logs breeze-core` — the access log should show requests arriving.
2. `GET /api/version` — confirms which implementation answered.
3. **The timezone.** `GET /api/system` and look at `server.timezone` and
   `utc_offset_seconds`. If the offset is `0` and you did not ask for UTC, the
   image is missing tzdata for your zone and **every schedule is wrong** while
   everything looks fine.
4. `breeze-core diag` — but note it posts a deliberately out-of-range
   temperature to test validation, so not while you mind the units being
   touched.

### What does not migrate

- **The nginx image's generated certificate.** If you used
  `alpine-edge-nginx`, its self-signed certificate lived in the image's own
  volume and has no equivalent here. The replacement is a real proxy container
  with a real certificate.
- **`breeze-setup`.** See above.

## Order of work

1. The **tzdata startup warning** in the server. Independent of containers,
   fixes a silent wrong answer, cheap.
2. `Dockerfile` for the distroless image, multi-arch, built from the existing
   `packaging/out/bin/<arch>/breeze-core` so the container and the package ship
   the identical binary.
3. `docker-compose.yml` (plain) and `docker-compose.https.yml` (with a proxy
   container).
4. A `debug` variant.
5. The backup/restore/migrate commands above, verified on a real container
   rather than written down and hoped for — the same standard the BSD packages
   were held to.
6. Decide the UBI question deliberately: dropped, or `ubi9-micro` kept for
   policy reasons.

## What was verified, and how

Every claim above that could be tested, was:

| Claim | How |
|---|---|
| `FROM scratch` runs the binary | `docker run <image> --version` → `breeze-core 4.0.1` |
| No shell in the plain image | `--entrypoint /bin/sh` fails to start |
| The debug tag has one | `sh -c` works, `tar` is at `/bin/tar` |
| Runs as uid 1001 | `docker inspect .Config.User` → `1001:1001` |
| **TZ resolves** | `TZ=Europe/Zagreb` → `utc_offset_seconds: 7200` |
| Cloud TLS needs no cert layer | `webpki-roots` in the lockfile, zero `rustls-native-certs` |
| arm64 works | built, then run under qemu → reports its version |
| Backup → restore round-trips | archived a live volume, destroyed it, restored into a fresh one, served with the original API key |
| Restore keeps modes | `config.json` `0640`, `devices.json` `0600`, both uid 1001 |
| Both compose files parse | `docker compose config` |

One thing found while writing the compose file, worth recording because it was
nearly shipped: **the obvious healthcheck is actively harmful.** A container
healthcheck must run inside the container, this image has no shell or curl, so
the only candidate is the binary — and `breeze-core diag` tests input validation
by POSTing a deliberately out-of-range temperature to a real unit. As a
healthcheck that sends a control request to an air conditioner every 60 seconds
forever. There is no healthcheck in the compose file, with that reason written
next to where it would go.

## Still to do

1. **A `breeze-core healthcheck` subcommand** — a bare `GET /api/health` and an
   exit code, nothing else. It is what makes a container healthcheck possible at
   all, and the absence of one is why there isn't a healthcheck above.
2. **The tzdata startup warning.** Full zoneinfo closes the common case; a typo
   in `TZ` still silently means UTC.
3. **`latest` is the open question that remains.** Nothing is tagged `latest`
   on the native package. The Python line's `latest` points at `alpine-edge` and
   stays there. Whether the native package ever gets one is a decision for
   whenever this goes public — a versioned tag costs a reader nothing and a
   moving one can surprise them.

## A note on where these are published

**`ghcr.io/monikapurpl3/breeze-core-native`, not `…/breeze-core`.** The first
push went to the latter and that was a mistake worth recording: the
`breeze-core` package already existed from the Python line and was **public**,
attached to the public Python repository, so the push inherited that visibility
and made the native images anonymously pullable. GHCR's private-by-default
applies to a *new* package, not to one that already exists.

The lesson is the check, not the default: **ask what visibility the package
already has before pushing to it.** The images were removed (13 versions, none
of them the Python line's) and re-pushed to a package of their own, which is
also where they belong regardless of visibility.

Verified private the same way anyone else would see it -- an anonymous token
from `ghcr.io/token` and a manifest request, which returns 403 for both native
tags while `breeze-core:alpine-edge` returns 200. Note that
`docker manifest inspect` with an empty `DOCKER_CONFIG` is **not** an anonymous
test: Docker Desktop's credential helper answers anyway, and it reported those
same private tags as readable.
