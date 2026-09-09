# Timers

A timer is **one-shot**: do this once, in *n* minutes, then forget about it.
"Switch off in 45 minutes" is the whole idea.

For anything recurring or time-of-day, you want
[Programs, schedules and curves](Programs-schedules-and-curves).

## You ask in minutes, never a time of day

```sh
curl -X POST http://server:8420/api/timers \
  -H "X-API-Key: $KEY" -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"unit_ids": ["153931628470980"], "minutes": 45}'
```

That is deliberate, and it is the one design decision on this page worth
understanding.

The server computes the moment from **its own clock**, and every response
carries **`seconds_remaining`**, computed server-side, so a client counts down
from that rather than from its own idea of the time. The alternative — a client
sending a wall-clock time — puts the burden of knowing the server's timezone on
every client, when the phone may be in another zone and its clock may simply be
wrong.

`minutes` has **no default**. "In zero minutes" and "you forgot to say when"
are different things, and a request without it is refused rather than guessed
at. It must be **1 to 1440** — a timer is a nap, not a calendar, and anything
longer belongs in a schedule. At most **64 units** in one timer.

## What it does when it fires

Nothing but switch the unit off, unless you say otherwise. `settings` defaults
to `power_state: false`.

But it is a **full control payload**, so "switch to eco in an hour" needs no
second feature:

```sh
-d '{"unit_ids": ["1539…"], "minutes": 60,
     "settings": {"eco": true, "target_temperature": 26}}'
```

Anything [Control schema](Control-schema) accepts goes in there, and the
bounds are checked when you **create** the timer, not when it fires. A timer
that could never work is refused up front rather than failing silently in an
hour.

## Several units, one timer

`unit_ids` is a list. One timer can switch off the whole house, and it is one
object to cancel rather than four.

## The endpoints

| | Path | Does |
|---|---|---|
| `GET` | `/api/timers` | pending timers, each with `seconds_remaining` |
| `POST` | `/api/timers` | create one |
| `DELETE` | `/api/timers/{id}` | cancel |
| `GET` | `/api/timers/status` | is the runner alive, and when did it last look |

All four need full auth — API key **and** a device credential.

A stored timer looks like this:

```jsonc
{
  "id": "9f3c1ab27d04",                     // 12 hex chars
  "unit_ids": ["153931628470980"],
  "minutes": 45,
  "created_at": "2026-09-09T14:02:11",     // server-local, naive
  "fires_at":   "2026-09-09T14:47:11",
  "settings": { "power_state": false },
  "label": ""
}
```

`created_at` and `fires_at` are **naive server-local** strings — the same clock
the scheduler works in. They are there to be read by a human; a client should
use `seconds_remaining` and not parse them.

## How it actually fires

A single background thread wakes every `AC_TIMER_TICK` seconds (15 by default),
looks for anything due, and applies it.

Fifteen seconds is finer than the scheduler's thirty, on purpose: **a schedule
only has to land inside the right minute, but a timer is a promise about a
moment.** "Off in 45 minutes" landing 40 seconds late is a worse answer than a
schedule firing at 07:00:20.

The timer goes through **exactly the same code path** as a control request from
the panel — the same validation, the same per-unit lock, the same retry
behaviour. A timer cannot do anything to a unit that you could not do yourself
over HTTP.

## Timers survive a restart

They live in `timers.json` alongside your other state, so restarting the
service — or upgrading it — does not lose a pending timer.

A timer whose moment passed while the server was **down** fires on the next
tick after it comes back, rather than being dropped. Worth knowing if a
machine was off for an hour: "switch off in 20 minutes" will happen shortly
after boot, which is usually what you wanted and occasionally a surprise.

## Timezone, once

Everything scheduled here uses the **server's local clock**. On a normal
install that is whatever the machine is set to. **In a container it is UTC
unless you set `TZ`**, and a timer that fires an hour or two off is almost
always that and not a bug — check `utc_offset_seconds` on `GET /api/system`.

## Troubleshooting

| Symptom | Cause |
|---|---|
| `422` on create | a missing or out-of-range `minutes` (1–1440), more than 64 units, or a `settings` value outside [Control schema](Control-schema) bounds |
| `404` on create | an unknown unit id — ids are **strings**, and sending one as a JSON number silently rounds it |
| fires at the wrong time | server timezone; see above |
| never fires | check `GET /api/timers/status` for the runner's last pass |
| fired but nothing happened | the unit was unreachable. The attempt is real; the air conditioner declined |
