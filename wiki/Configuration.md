# Configuration

Where Breeze Core keeps its state, and every environment variable that changes
how it behaves. The files are written by `breeze-core pair` and by the server
itself; the variables are read **once at startup**, so a change means a restart.

The variable names are the 3.x names on purpose. An existing unit file, env file
or container configuration keeps working untouched — that is most of what "drop-in
replacement" means in practice.

## The four store files

All four live in one directory. `AC_CONFIG_DIR` names it (default
`/etc/breeze-core`), and each file can be moved individually if you need to.

| File | Mode | Written by | Holds |
|---|---|---|---|
| `config.json` | `0640` | `pair`, and the panel's add/rename/remove | the API key and the unit list |
| `devices.json` | `0600` | the server | enrolled device credentials, hashed |
| `programs.json` | `0600` | the server | favourites, schedules, curves |
| `timers.json` | `0600` | the server | pending one-shot timers |

Those modes are enforced by the store layer on every write, not set once at
install time — so a file restored from a backup with loose permissions is
tightened the next time the server saves it. `config.json` is `0640` because an
admin group needs to read the API key; everything else is owner-only.

### `config.json`

```json
{
  "api_key": "randomly-generated-urlsafe-string",
  "units": [
    { "name": "Living Room", "ip": "192.168.1.73", "port": 6444,
      "id": 153931628470980, "token": null, "key": null }
  ]
}
```

| Field | Notes |
|---|---|
| `api_key` | The **enrolment** secret — needed to *begin* pairing, and not sufficient on its own to control anything. See [Authentication and pairing](Authentication-and-pairing). |
| `units[].name/ip/port/id` | Friendly name, LAN IP, control port (usually `6444`), Midea device id. |
| `units[].token/key` | V3 credentials (`null` for V1/V2). **Back these up** — they cannot be re-derived once the vendor cloud stops issuing them for your unit. |

The format is byte-compatible with 3.x, which is why an upgrade is an upgrade
rather than a migration. The `breeze-store` crate exists to guarantee that: it
reads and writes exactly the shapes the Python line did, including the fields it
does not itself use.

## Runtime settings (environment variables)

Defaults are chosen to be safe on a machine that might get exposed: headers on,
admin approval restricted to the LAN, proxy headers distrusted.

| Var | Default | Purpose |
|---|---|---|
| `AC_CONFIG_DIR` | `/etc/breeze-core` | the directory the four stores default into |
| `AC_CONFIG` | `<dir>/config.json` | config file path |
| `AC_DEVICES` | `<dir>/devices.json` | per-device credential store |
| `AC_PROGRAMS` | `<dir>/programs.json` | favourites/schedules/curves |
| `AC_TIMERS` | `<dir>/timers.json` | one-shot timers ([Timers](Timers)) |
| `BREEZE_HOST` | `127.0.0.1` | address to bind |
| `BREEZE_PORT` | `8420` | port to bind |
| `BREEZE_WORKERS` | `8` | HTTP worker threads |
| `AC_SCHED_TICK` | `30` | seconds between scheduler passes |
| `AC_TIMER_TICK` | `15` | seconds between timer due-checks |
| `AC_STREAM_TICK` | `5` | seconds between state polls **while at least one client is streaming** |
| `AC_HISTORY_SIZE` | `720` | samples kept per unit for `/history` and `/metrics` (~1 h at the 5 s cadence) |
| `AC_MIN_AUTH_VERSION` | `1` | lowest device auth version accepted. `1` accepts legacy bearer **and** Ed25519 v2; `2` refuses v1 with `426`. See [Signed auth (v2) migration](Signed-auth-v2-migration) |
| `AC_SECURITY_HEADERS` | on | emit CSP and the rest. Set `0`/`false`/`no` to turn off when a proxy already sets them — duplicated CSP headers are *intersected*, not deduplicated |
| `AC_BEHIND_PROXY` | off | trust `X-Forwarded-For` for the real client IP. Set `1`/`true`/`yes` **only** behind a proxy you control |
| `BREEZE_CLI_PROFILE` | `$XDG_CONFIG_HOME/breeze-core/cli.json`, else `~/.config/…`, else `%APPDATA%\…` | where the CLI keeps *its own* enrolled credential (client-side only; the server never reads this) |

`serve` also takes `--host`, `--port` and `--behind-proxy` as flags, and **a flag
wins over the environment** — it is the more specific instruction, and the
packaged unit files pass them. `--behind-proxy` is one-way: it can turn proxy
trust on, never off, so a stray flag cannot quietly undo `AC_BEHIND_PROXY=1` and
turn the LAN-only admin check into a check that every proxied request passes.

## Variables that 3.x had and 4.x does not

Not oversights — each one either became a constant or stopped being reachable.

| Gone | What happened |
|---|---|
| `AC_DOCS` | There is no OpenAPI schema to expose, so there is no `/docs` to gate. The [REST API](REST-API) page is the reference. |
| `AC_ENROLL_LAN_ONLY` | Now unconditional. Admin approval and device management are gated to private addresses with no way to switch it off, because the switch existed only to make a bad configuration possible. |
| `AC_CODE_TTL` | Fixed at **60 s**. |
| `AC_TOKEN_TTL_DAYS` | Fixed at **90 days**. |
| `AC_AUTH_SKEW_SECONDS` | Fixed at the signing default. A wider window is a wider replay window, and the self-healing `clock_skew` response makes a client's bad clock fixable without loosening the server. |
| `AC_TRUSTED_HOSTS` | Dropped. Host-header filtering belongs in the reverse proxy that terminates TLS, which is the only place that can do it correctly — see [Reverse proxy and TLS](Reverse-proxy-and-TLS). |
| `AC_COMPRESSION` | Always on, negotiated per response, and **gzip only** — no brotli. Brotli costs several hundred KB of binary for a few percent on documents this size, every client in play sends `gzip`, and its absence removes the trap that made the 3.x SSE route force `Content-Encoding: identity`. Small bodies, `204`/`304`, already-encoded replies and the SSE stream are never compressed. |

If your 3.x env file sets any of them, nothing breaks: unknown variables are
ignored. The behaviour they used to select is either the new default or is
handled a layer up.

## The packaged env file

The packages install `/etc/breeze-core/breeze-core.env`, and what they ship is
deliberately minimal — three lines plus comments:

```sh
BREEZE_HOST=127.0.0.1
BREEZE_PORT=8420
BREEZE_OPTS=
```

Loopback is the default because a fresh install should not appear on the network
before you have decided it should. Editing this file is step two of every
install:

```sh
# Bind the LAN address you actually want to reach, or leave it on loopback and
# put a reverse proxy in front. 0.0.0.0 is never the right answer here.
BREEZE_HOST=192.168.1.10
BREEZE_PORT=8420

# Extra flags for `serve`. This is where --behind-proxy goes if a reverse proxy
# is genuinely in front of it.
BREEZE_OPTS=

# Only once every client signs its requests -- see the v2 migration page.
#AC_MIN_AUTH_VERSION=2
```

`BREEZE_OPTS` is interpolated into the service's `serve` invocation, which is why
`serve` accepts and ignores unknown options rather than refusing to start: the
variable is usually empty, and a service that died on an empty expansion would be
a worse failure than a warning on an unrecognised flag.

## Timezone

Everything scheduled here — schedules, curves, timers — uses the **server's local
clock**. On a normal install that is whatever the machine is set to. In a
container it is UTC unless you set `TZ`, and a schedule that fires an hour or two
off is almost always this and not a bug.
