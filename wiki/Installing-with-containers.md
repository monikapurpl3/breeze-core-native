# Installing with containers

Two images. One to run, one to debug.

| Tag | Base | Size | For |
|---|---|---|---|
| `4.0.1` | `scratch` | **5.9 MB** | everyone |
| `4.0.1-debug` | `busybox:musl` | 8.4 MB | working out why something is wrong |

**amd64 and arm64**, as one manifest — `docker pull` picks the right one.

**Versioned tags only. There is no `latest`**, deliberately: the Python line's
`latest` points at its Alpine image, and repointing it at a distroless one would
change what `docker pull` gives anybody who has it in a compose file — no shell,
a different entrypoint, `breeze-setup` gone. Ask for the version you want.

> **The package is private, so pulling needs authentication:**
>
> ```sh
> echo $GITHUB_TOKEN | docker login ghcr.io -u <your-github-user> --password-stdin
> ```
>
> A token with `read:packages` is enough. This will change when the project is
> published; until then the images are visible only to accounts that have been
> granted access.

The image is built `FROM scratch`: one static executable, the timezone
database, and an empty config directory. No shell, no package manager, no libc,
nothing to patch. If you are replacing an image from an earlier version, see
[What changed in 4.x](What-changed-in-4).

## Start it

```sh
docker volume create breeze-config

# Pair first. This needs the host's network -- see below.
docker run --rm -it --network host \
  -v breeze-config:/etc/breeze-core \
  ghcr.io/monikapurpl3/breeze-core-native:4.0.1 pair

docker run -d --name breeze-core --restart unless-stopped \
  -e TZ=Europe/Zagreb \
  -p 192.168.1.10:8420:8420 \
  -v breeze-config:/etc/breeze-core \
  --read-only --cap-drop ALL --security-opt no-new-privileges:true \
  ghcr.io/monikapurpl3/breeze-core-native:4.0.1
```

Or use the compose files in `packaging/container/` — `docker-compose.yml` for
plain HTTP, `docker-compose.https.yml` for a Caddy container in front with a
real certificate.

**On a fresh volume it exits immediately**, on purpose, with:

```
breeze-core: /etc/breeze-core/config.json has no api_key — run `breeze-core pair`
```

That is not a crash. There is nothing to serve until units are paired, and
saying so is better than idling and looking healthy.

## `TZ` is not cosmetic

**Set it.** Everything scheduled here — schedules, curves, timers — uses the
server's local clock, and a container's clock is UTC unless you say otherwise.
Get it wrong and every program fires at the wrong hour while the server looks
completely healthy.

The image carries the **full tzdata**, all 1.5 MB of it, rather than a trimmed
set — because guessing which zones somebody needs is how you ship a container
that is correct for you and two hours wrong for them. Verified: `TZ=Europe/Zagreb`
inside the image reports a `+02:00` offset.

Check it after starting, on `GET /api/system`:

```json
"server": { "timezone": "+02:00", "utc_offset_seconds": 7200 }
```

If `utc_offset_seconds` is `0` and you did not ask for UTC, something has
replaced or hidden `/usr/share/zoneinfo` — a volume mounted over it, or a typo
in the zone name. The server does not currently warn about this, which is a
known gap.

## The networking question — read this one

**Discovery is a UDP broadcast**, so `breeze-core pair` needs the host's layer 2.
A bridge network has no route to your air conditioners' broadcast domain, and
pairing will simply find nothing.

- **Pairing:** `--network host`. Not optional.
- **Normal operation:** a bridge network is fine. Once paired, the server only
  makes unicast TCP connections to addresses it already has in `config.json`,
  so it just needs to be able to route to them.

Publish to the LAN address you actually want to reach — `-p 192.168.1.10:8420:8420`
— rather than to every interface. `0.0.0.0` inside the container is correct and
is the image default; the host side is where you should be specific.

## There is no shell

That is the point of a distroless image, and it changes two things.

**The binary is the entrypoint**, so subcommands run directly:

```sh
docker run --rm -it --network host -v breeze-config:/etc/breeze-core \
  ghcr.io/monikapurpl3/breeze-core-native:4.0.1 pair

docker exec breeze-core breeze-core devices
docker exec breeze-core breeze-core approve BONG-W3GN
```

**`docker logs` is your introspection.** Which is why the access log matters
more here than anywhere: one line per request with its status and duration.
`BREEZE_LOG=0` silences it; `BREEZE_DEBUG=1` adds a full trace of every control
and every rejected signature.

When that is not enough, run the `-debug` tag instead. Identical binary,
identical paths, identical uid — plus `sh`, `tar` and `wget`.

> **The `-debug` image is not for normal use.** A shell in a production
> container is a shell an attacker gets too.

## There is no healthcheck, and that is deliberate

A container healthcheck has to run *inside* the container, and this one has no
shell and no `curl` — so the only candidate is the binary itself.
`breeze-core diag` looks like the answer and is not: part of what it checks is
input validation, which it tests by **POSTing a deliberately out-of-range
temperature to a real unit**. As a healthcheck that would send a control
command to an air conditioner every sixty seconds, forever.

Until there is a `breeze-core healthcheck` subcommand that does nothing but
`GET /api/health`, the honest options are no healthcheck or one run from outside
the container. Docker restarts the container if the process dies, which is the
failure that actually happens.

## What is in the image, and what is not

| | |
|---|---|
| the binary | static musl, no libc at runtime, panel compiled in |
| full tzdata | 1.5 MB, and required — see above |
| `/etc/breeze-core` | empty, owned by uid 1001 |
| **no** `ca-certificates` | the TLS roots are compiled into the binary, so cloud pairing works with no trust store on disk |
| **no** `/etc/passwd` | it runs as numeric uid `1001:1001`, which needs no entry |
| **no** shell, package manager, or writable filesystem outside the volume | |

It runs as uid **1001**, which is also what earlier container images used — so
an existing volume is writable without changing anything.

## Backing it up

The whole state is four files in one directory. Back up the **data**, not the
image.

```
/etc/breeze-core/config.json     the API key and the units, INCLUDING V3 credentials
/etc/breeze-core/devices.json    enrolled clients
/etc/breeze-core/programs.json   favourites, schedules, curves
/etc/breeze-core/timers.json     pending one-shot timers
```

**`config.json` is the one that cannot be regenerated.** A paired V3 unit's
token and key were issued once by a cloud that no longer hands them out.

```sh
docker run --rm \
  -v breeze-config:/data:ro \
  -v "$PWD":/out \
  busybox tar czf /out/breeze-backup-$(date +%F).tar.gz -C /data .
```

A sidecar rather than `docker cp`, so it works for a named volume, a bind mount
or a compose project alike — and `:ro` so a backup cannot damage what it is
copying. The archive holds an API key and V3 credentials: `chmod 600`, and keep
it somewhere that is not the same disk.

Stopping the container first is optional. The stores are written with a
create-and-rename, so a backup taken mid-write gets the previous file rather
than a torn one. Before an upgrade, stop it anyway.

### Restore

```sh
docker run --rm -v breeze-config:/data -v "$PWD":/in \
  busybox tar xzf /in/breeze-backup-2026-09-08.tar.gz -C /data
```

Modes come back with the archive: `config.json` `0640`, the rest `0600`, all uid
1001. If they do not, the server tightens them on its next write anyway.

## Moving it to another host

```sh
# streamed, so the credentials never land on either host's filesystem
docker run --rm -v breeze-config:/data:ro busybox tar cz -C /data . \
  | ssh newhost 'docker run --rm -i -v breeze-config:/data busybox tar xz -C /data'
```

Then start the image there. Same units, same key, same enrolled clients — no
re-pairing.

**The unit addresses travel with it.** `config.json` holds each unit's LAN IP, so
moving to a different network means editing them, or re-running `pair`, which
rewrites the addresses while keeping the credentials.

**Architecture does not matter.** The four store files are plain JSON with
nothing endian- or word-size-dependent in them, so amd64 → arm64 is the same
copy plus the right image, which the manifest picks for you.

## Coming from the Python images

No conversion step, because there is nothing to convert — the store files are
the same format.

```sh
docker stop breeze-core && docker rm breeze-core
docker run -d --name breeze-core --restart unless-stopped \
  -e TZ=Europe/Zagreb -p 192.168.1.10:8420:8420 \
  -v breeze-config:/etc/breeze-core \
  ghcr.io/monikapurpl3/breeze-core-native:4.0.1
```

Check, in this order:

1. `docker logs breeze-core` — requests should appear as they arrive.
2. `GET /api/version` — confirms which implementation answered.
3. **The timezone**, as above. An offset of `0` you did not ask for means every
   schedule is wrong while everything looks fine.

What does not come across:

- **`alpine-edge-nginx`'s generated certificate.** It lived in that image's own
  volume and has no equivalent. Use `docker-compose.https.yml`, which pairs the
  server with a real proxy container and a real certificate.
- **`breeze-setup`.** See above.
- **The UBI images.** They existed for a vendor-patched *glibc*, and there is no
  glibc here to patch. If you needed a Red Hat base for policy rather than for
  libc, nothing here replaces it — say so.
