# Configuration

Where Breeze Core keeps its state, and every environment variable that changes
how it behaves. The files are written by `breeze-core pair` and by the server
itself; the variables are read **once at startup**, so a change means a restart.

## The four store files

All four live in one directory. `AC_CONFIG_DIR` names it (default
`/etc/breeze-core`), and each file can be moved individually if you need to.

| File | Mode | Written by | Holds |
|---|---|---|---|
| `config.json` | `0640` | `pair`, and the panel's add/rename/remove | the API key and the unit list |
| `devices.json` | `0600` | the server | enrolled device credentials, hashed |
| `programs.json` | `0600` | the server | favourites, schedules, curves |
| `timers.json` | `0600` | the server | pending one-shot timers |

The modes are enforced on every write, not set once at install time — so a file
restored from a backup with loose permissions gets tightened the next time the
server saves it. `config.json` is `0640` because the admin CLIs read the API key
from it without `sudo`; everything else is owner-only.

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
| `BREEZE_WORKERS` | `8` | HTTP worker threads. A request waits on its unit for a round-trip, about 0.75 s, so this is how many fit in flight at once — but requests to one unit are merged or share an answer, so the useful ceiling is roughly the number of units you own |
| `BREEZE_BG_WORKERS` | `1` | how many units the background state poller contacts **at once**. `1` is a sequential walk. Raise it when the walk stops fitting inside `AC_STREAM_TICK`. Not a count of background threads — see below |
| `BREEZE_KEEP_WARM` | `30m` | how long to keep connections to the units open after the server was last used. A bare number is seconds, or `90s`, `30m`, `2h`; `0`/`off` never; `always` from startup. See [Keeping connections warm](#keeping-connections-warm) |
| `AC_SCHED_TICK` | `30` | seconds between scheduler passes |
| `AC_TIMER_TICK` | `15` | seconds between timer due-checks |
| `AC_STREAM_TICK` | `5` | seconds between state polls **while at least one client is streaming** |
| `AC_HISTORY_SIZE` | `720` | samples kept per unit for `/history` and `/metrics` (~1 h at the 5 s cadence) |
| `AC_MIN_AUTH_VERSION` | `1` | lowest device auth version accepted. `1` accepts legacy bearer **and** Ed25519 v2; `2` refuses v1 with `426`. See [Signed auth (v2) migration](Signed-auth-v2-migration) |
| `AC_CODE_TTL` | `60` | seconds a pairing code stays valid. Short on purpose — somebody is typing it |
| `AC_TOKEN_TTL_DAYS` | `3650` | days a device credential lasts (ten years). `0` or less means it never expires |
| `AC_AUTH_SKEW_SECONDS` | `60` | how far a signed request's timestamp may sit from server time, each way. Widening this widens the replay window by exactly as much |
| `AC_SECURITY_HEADERS` | on | emit CSP and the rest. Set `0`/`false`/`no` to turn off when a proxy already sets them — duplicated CSP headers are *intersected*, not deduplicated |
| `AC_BEHIND_PROXY` | off | trust `X-Forwarded-For` for the real client IP. Set `1`/`true`/`yes` **only** behind a proxy you control |
| `BREEZE_CLI_PROFILE` | `$XDG_CONFIG_HOME/breeze-core/cli.json`, else `~/.config/…`, else `%APPDATA%\…` | where the CLI keeps *its own* enrolled credential (client-side only; the server never reads this) |

`serve` also takes `--host`, `--port` and `--behind-proxy` as flags, and **a flag
wins over the environment** — it is the more specific instruction, and the
packaged unit files pass them. `--behind-proxy` is one-way: it can turn proxy
trust on, never off, so a stray flag cannot quietly undo `AC_BEHIND_PROXY=1` and
turn the LAN-only admin check into a check that every proxied request passes.

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

## Concurrency, and the two worker settings

Breeze Core serves requests on a **fixed pool of threads** — `BREEZE_WORKERS`,
default 8 — fed from a queue, plus exactly three background threads: the timer
runner, the scheduler and the state poller.

A pool rather than a thread per request, because a request waits on a unit for
a round-trip — about 0.75 s, whatever it asks — so some concurrency is
essential, and a thread per request would let anyone who can reach the port
exhaust memory.

**Eight is more than it sounds.** Every operation is a LAN round-trip to one
*specific* unit, and each unit is locked while its own request runs. Two
requests for the living room serialise however many workers exist, so the
parallelism that helps is bounded by **how many units you own**, not by this
number. Three units cannot keep eight workers busy.

**Requests to a busy unit do not queue one behind another.** Tapping + ten
times sends ten requests, and in 4.0.2 they ran one after another — the last
one finished half a minute after the tapping stopped, each answered with a
temperature already tapped past, and every waiting request held a worker, so
the other units stalled too. From 4.1.0:

- **controls merge**: a control that arrives while the unit is busy folds into
  the next command, which carries every change that arrived meanwhile and
  answers all of them. Ten taps take a handful of commands and are all answered
  within about a second and a half of the last tap;
- **reads share**: a read that waited behind another read — or behind a
  control — takes that answer instead of asking the unit again;
- **nothing that only needs a name waits on a unit**: `GET /api/units` and the
  diagnostics screen read copies kept beside each unit.

A control is also **one round-trip** where it was two. It is built on the
state the unit reported in the last ten seconds — the panel's own poll, or the
previous command's reply — and only reads the unit first when nothing that
recent is on hand. The ten seconds bound the one trade-off: a setting changed
with the IR remote since that report would be put back.

**`BREEZE_BG_WORKERS` is not a count of background threads.** Those three
threads are singleton roles and have to stay that way — a second scheduler
would fire every program twice. This setting is how many units the *state
poller* contacts at once during one pass:

| Units | `BREEZE_BG_WORKERS=1` (default) | Raised |
|---|---|---|
| 3 | ~3 s per pass, inside the 5 s tick | no benefit worth having |
| 8 | ~8 s per pass — **longer than the tick** | `4` brings it to ~2 s |

Once a pass takes longer than `AC_STREAM_TICK`, every tick starts later than
the last and the stream lags the units it is reporting on. Raising this is the
fix; shortening the tick is not, since that only adds traffic.

Raising it is safe: each unit has its own lock and its own cached connection,
so two units are genuinely independent.

A streaming client never occupies a worker at all — each Server-Sent Events
connection gets its own thread, specifically so a watching browser cannot sit
in the pool. There is a ceiling of 64 simultaneous streams, and the 65th gets a
refusal rather than a dropped socket.

## Keeping connections warm

A unit closes its connection **30 seconds after the last request** it
received, and the next request then pays for a new one: connect, handshake, and
the one-second pause the protocol requires before the unit will listen — about
a second in all. Opening the app a minute after closing it paid that on every
unit.

So after the server was last used — an authenticated request, or an app or
panel with its live stream open — Breeze Core keeps each unit's connection
alive by reading it once it has been quiet for 20 seconds. That is one small
request per unit every 20–25 seconds, and only inside the window:

| `BREEZE_KEEP_WARM` | Behaviour |
|---|---|
| unset, or `30m` | warm for half an hour after the last use, then quiet |
| `2h`, `90s`, `1800` | the same, for that long. A bare number is seconds |
| `0` or `off` | never: no traffic to a unit unless a client asks for something |
| `always` | warm from startup, whether or not anyone has connected |

A unit that is busy is never read for this — whoever is using it is keeping it
warm — and one that has just failed to answer is left alone for five minutes,
so a unit that is off the network is not asked every 20 seconds.

The setting and whether it is active at this moment are on the Nerd panel,
under `settings.keep_warm` and `settings.keep_warm_active`.

## Timezone

Everything scheduled here — schedules, curves, timers — uses the **server's local
clock**. On a normal install that is whatever the machine is set to. In a
container it is UTC unless you set `TZ`, and a schedule that fires an hour or two
off is almost always this and not a bug.
