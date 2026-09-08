# REST API

The one contract all three components share. Unchanged from 3.x — this page is
the reference because there is no generated schema any more: no framework, so no
`/docs`.

Base URL is wherever the server binds, `http://<host>:8420` by default.

## Authentication in one paragraph

Two credentials, and control needs both. The **API key** is an *enrolment*
secret — it gets you as far as starting a pairing and no further. A **device
credential** is what actually authorises control, and each client has its own,
revocable one. Full model: [Authentication and pairing](Authentication-and-pairing).

```
X-API-Key: <the key from config.json>
Authorization: Bearer <device token>          # auth version 1
```

or, for auth version 2, the same key plus a signature:

```
X-API-Key:              <the key>
X-Breeze-Auth-Version:  2
X-Breeze-Key-Id:        <token_id>
X-Breeze-Timestamp:     <unix seconds>
X-Breeze-Nonce:         <base64url, unpadded>
X-Breeze-Signature:     <base64url Ed25519 over the canonical string>
```

## Every endpoint

`open` needs nothing · `key` needs the API key · `full` needs key + device ·
`lan` needs the key **and** a private source address

| | Path | Guard | |
|---|---|---|---|
| `GET` | `/api/health` | open | liveness; the only unauthenticated endpoint |
| `GET` | `/api/version` | key | version, commit, feature list, accepted auth versions |
| `GET` | `/metrics` | key | Prometheus text format |
| `GET` | `/api/units` | full | the configured units |
| `GET` | `/api/units/state` | full | every unit's state in one call |
| `GET` | `/api/units/{id}/state` | full | one unit |
| `POST` | `/api/units/{id}/control` | full | apply the fields present |
| `GET` | `/api/units/{id}/capabilities` | full | what the firmware admits to |
| `GET` | `/api/units/{id}/history` | full | recent samples, in memory |
| `GET` | `/api/units/stream` | full | Server-Sent Events |
| `GET` | `/api/units/scan` | full | discover units on the LAN |
| `POST` | `/api/units` | full | add a unit |
| `PATCH` | `/api/units/{id}` | full | rename |
| `DELETE` | `/api/units/{id}` | full | remove |
| `GET` | `/api/config` | full | sanitised config — never a secret |
| `GET` | `/api/system` | full | the whole deployment as the server sees it |
| `POST` | `/api/auth/enroll/start` | key | begin pairing |
| `POST` | `/api/auth/enroll/poll` | key | wait for approval |
| `POST` | `/api/auth/enroll/approve` | **lan** | approve a code |
| `GET` | `/api/auth/devices` | **lan** | enrolled credentials |
| `DELETE` | `/api/auth/devices/{token_id}` | **lan** | revoke one |
| `GET` | `/api/auth/whoami` | full | which credential am I |
| `POST` | `/api/auth/upgrade` | full | move a v1 device to v2 |
| `GET` | `/api/programs` | full | favourites, schedules, curves |
| `POST` | `/api/programs` | full | create one |
| `GET` | `/api/programs/{id}` | full | one |
| `PUT` | `/api/programs/{id}` | full | replace |
| `DELETE` | `/api/programs/{id}` | full | delete |
| `POST` | `/api/programs/{id}/apply` | full | run it now |
| `GET` | `/api/programs/status` | full | scheduler health |
| `GET` | `/api/timers` | full | pending one-shot timers |
| `POST` | `/api/timers` | full | create one |
| `DELETE` | `/api/timers/{id}` | full | cancel |
| `GET` | `/api/timers/status` | full | runner health |

A known path under the wrong method is `405` with an `Allow` header, so a client
can tell "wrong verb" from "no such thing".

## State

```jsonc
{
  "id": "153931628470980",      // a STRING, always -- see below
  "name": "Living Room",
  "ip": "192.168.1.73",
  "online": true,
  "power_state": false,
  "operational_mode": "COOL",   // the enum NAME, never its number
  "target_temperature": 24.0,
  "indoor_temperature": 27.5,   // null if the unit has no probe
  "outdoor_temperature": 27.0,  // null likewise, which is common
  "fan_speed": 102,             // 102 is "auto"
  "swing_mode": "BOTH",
  "eco": false,
  "turbo": false
}
```

**`id` is a string.** Midea device ids exceed 2^53, so a JSON number would be
silently rounded by every JavaScript client on earth. Send it back as a string
too.

**Enum values are names.** `"COOL"`, not `2`. This was a real bug in the Python
line — `IntEnum.__str__` changed in Python 3.11 and a `str()`-based serialiser
started emitting bare integers — so `breeze-core diag` still checks it.

`indoor_temperature` and `outdoor_temperature` are nullable and frequently
null. Plenty of units have no outdoor probe; treat absence as normal, not as a
fault.

## Control

`POST /api/units/{id}/control` applies **only the fields present**. Everything
else about the unit is left as it is.

```sh
curl -X POST http://server:8420/api/units/153931628470980/control \
  -H "X-API-Key: $KEY" -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"operational_mode": "HEAT", "target_temperature": 22.5}'
```

The response is the unit's own state after the change — its echo, not what you
asked for. That is what lets an optimistic UI be honest: the client reconciles
against what happened.

Fields, bounds and the `beep` rule: [Control schema](Control-schema).

## Status codes

Worth reading, because clients branch on them and the split is not arbitrary:

| Code | When |
|---|---|
| `400` | **an unknown enum member** — `operational_mode`, `swing_mode` |
| `422` | **an out-of-range or invalid number** — temperature, fan speed |
| `401` | missing, wrong or expired credentials |
| `403` | credentials fine, but an admin action came from a non-private address |
| `404` | no such unit, program or timer |
| `405` | the path exists under a different method |
| `426` | the credential's auth version is below `AC_MIN_AUTH_VERSION` |
| `503` | the unit is unreachable, or refused the command |

The `400`/`422` split mirrors the reference exactly, and the reason it looks
inconsistent is that it is inherited: pydantic rejected the numeric bounds while
parsing the body, so FastAPI turned those into its own `422`, while an unknown
mode name survived parsing and was refused by the route with an explicit `400`.
4.0.0 answered `422` for both; 4.0.1 restored the split.

`503` is the one worth handling deliberately: it means the request was valid and
the *unit* did not cooperate. Retrying may work; sending something different
will not help.

## Live state without polling

```sh
curl -N http://server:8420/api/units/stream \
  -H "X-API-Key: $KEY" -H "Authorization: Bearer $TOKEN"
```

Server-Sent Events. Every change is pushed — whether it came from you, another
client, a schedule or a timer — so a panel does not poll.

Two things to know:

- **The poller idles until somebody subscribes.** With no subscribers the server
  makes no LAN traffic at all. The first event after connecting may therefore
  take up to one tick (`AC_STREAM_TICK`, 5 s).
- **Never compress this stream.** A buffering proxy holds events until its
  window fills, which is indistinguishable from a dead connection. The server
  excludes the stream from compression; if you put a proxy in front, give it
  `proxy_buffering off` or Caddy's `flush_interval -1`.

## Timers

You ask for **minutes**, never a time of day.

```sh
curl -X POST http://server:8420/api/timers \
  -H "X-API-Key: $KEY" -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"unit_ids": ["153931628470980"], "minutes": 45}'
```

The server computes the moment from its own clock and every response carries
`seconds_remaining`, computed server-side, so a client counts down from that
rather than from its own idea of the time. That is deliberate: the alternative
puts the burden of knowing the server's timezone on every client, when the phone
may be in another zone and its clock may be wrong.

`settings` defaults to switching the unit off, but it is a full control payload,
so "switch to eco in an hour" needs no second feature. More:
[Timers](Timers).

## Diagnostics

`GET /api/system` returns the whole deployment as the server sees it: OS, init
system, CPU, the four store paths and their modes, the units and their cached
capabilities, enrolled devices, scheduler and stream state, and how *this*
request reached the server.

That last part earns its place — if `connection.client_ip` reads `127.0.0.1`
when you connected from a phone, your proxy is not forwarding the real address,
and that is exactly what silently breaks LAN-only approval.

**It never contains a secret.** Not the API key, not a device credential, not a
unit's V3 token or key. `breeze-core diag` greps the response for
secret-looking values, so the omission is checked rather than trusted.

## Errors have a body

```json
{ "detail": "Unknown mode: TELEPORT" }
```

Auth failures carry more, so a client can react rather than guess:

```json
{ "detail": { "error": "clock_skew", "retryable": true,
              "server_time": 1788889209.7, "max_skew_seconds": 60 } }
```

A `clock_skew` rejection includes the server's own time, which lets a client
correct its offset and retry instead of failing permanently — the one auth error
that is genuinely self-healing.
