# Timers

A timer is **one-shot**: do this once, then forget about it. There are two
kinds, and a unit can have one of each:

- a **sleep timer** — "switch off in 45 minutes";
- a **scheduled start** *(4.2.0)* — "switch on in 3 days at 07:30".

For anything recurring, you want
[Programs, schedules and curves](Programs-schedules-and-curves).

## A client never sends a moment

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
sending a moment it worked out — puts the burden of knowing the server's
timezone on every client, when the phone may be in another zone and its clock
may simply be wrong.

A scheduled start keeps to that: it is asked for as a number of days from the
**server's** today and a time on the **server's** clock, and the server works
out the moment. See [Switching on later](#switching-on-later).

## Switching off: a sleep timer

`minutes` has **no default**. "In zero minutes" and "you forgot to say when"
are different things, and a request without it is refused rather than guessed
at. It must be **1 to 1440** — a timer is a nap, not a calendar, and anything
longer belongs in a schedule. At most **64 units** in one timer.

### What it does when it fires

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

## Switching on later

*New in 4.2.0, advertised as the `timer_at` feature.*

```sh
-d '{"unit_ids": ["1539…"], "days": 3, "at": "07:30"}'
```

It fires on the server's date plus `days`, at `at` on the server's clock.

- **`days`** is 0 for today, 1 for tomorrow, up to **30** — a long holiday.
  Left out, it means today.
- **`at`** is `HH:MM`, 24-hour. A time already gone **today** is refused with a
  `422` that says so, on the server's clock:
  `"17:29 on 2026-09-27 has already passed on the server's clock"`.
- **It only switches the unit on**, and the unit comes back in whatever mode
  and at whatever temperature it last had. `settings` is refused on a start.
  "Heat to 22 at seven every weekday" is a schedule, not a timer.
- `minutes` in the reply is how long until it fires, rounded up, for display.
  Clients should still count down from `seconds_remaining`.

Give **either** `minutes` **or** `at`. Both at once, or `days` without `at`,
is refused rather than read one way or the other.

### If the server was down at the moment

A start more than **15 minutes** late — the server was not running when it was
due — is **dropped, not fired**, with a line in the log. "On at 07:30" should
not become "on at 14:00" in an empty house. Fifteen minutes rides out a
restart or an upgrade. A schedule skips a minute it missed in the same way.

A sleep timer is different; see [Timers survive a restart](#timers-survive-a-restart).

## One of each per unit

Setting a timer **replaces** any pending timer of the **same kind** that covers
one of its units. Asking for "off in 30" when one is already pending means you
changed your mind, not that you want two competing promises about one unit.

But only within a kind. "Off tonight, on on Monday" is a sleep timer and a
start, and neither displaces the other.

The replacement is of the **whole** timer. A sleep timer for the whole house,
replaced by one for a single unit, is gone for the other units too.

## Several units, one timer

`unit_ids` is a list. One timer can switch off the whole house, and it is one
object to cancel rather than four. An empty list means every unit.

## The endpoints

| | Path | Does |
|---|---|---|
| `GET` | `/api/timers` | pending **sleep** timers, each with `seconds_remaining` |
| `GET` | `/api/timers?kind=all` | every pending timer; `?kind=start` for starts only |
| `POST` | `/api/timers` | create one |
| `DELETE` | `/api/timers/{id}` | cancel, whichever kind |
| `GET` | `/api/timers/status` | is the runner alive, and when did it last look |

All of them need full auth — API key **and** a device credential.

**Why `GET /api/timers` hides starts.** A client written before 4.2.0 treats
every timer as a sleep timer, and would show a start three days out as
"switching off in 53 h". So a listing includes starts only when it asks for
them. A client that knows about starts sends `?kind=all`, and can tell whether
the server has them from `timer_at` in `GET /api/version`. An unknown `kind` is
a `422`.

A timer on the wire looks like this:

```jsonc
{
  "id": "9f3c1ab27d04",                     // 12 hex chars
  "kind": "start",                          // or "sleep"
  "unit_ids": ["153931628470980"],
  "minutes": 2310,
  "created_at": "2026-09-27T17:00:11",     // server-local, naive
  "fires_at":   "2026-09-29T07:30:00",
  "settings": { "power_state": true },
  "label": "",
  "seconds_remaining": 138589
}
```

`created_at` and `fires_at` are **naive server-local** strings — the same clock
the scheduler works in. They are there to be read by a human; a client should
use `seconds_remaining` and not parse them against its own clock.

In `timers.json` a sleep timer has no `kind` at all, exactly as before 4.2.0,
so a file written by 4.1 reads and writes unchanged. Only a start is stored
with `"kind": "start"`.

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

A **sleep timer** whose moment passed while the server was **down** fires on
the next tick after it comes back, rather than being dropped. Worth knowing if a
machine was off for an hour: "switch off in 20 minutes" will happen shortly
after boot, which is usually what you wanted and occasionally a surprise.

A **start** is only fired up to 15 minutes late; see
[If the server was down at the moment](#if-the-server-was-down-at-the-moment).

## Timezone, once

Everything scheduled here uses the **server's local clock**. On a normal
install that is whatever the machine is set to. **In a container it is UTC
unless you set `TZ`**, and a timer that fires an hour or two off is almost
always that and not a bug — check `utc_offset_seconds` on `GET /api/system`.

That matters more for a start than a sleep timer: `"at": "07:30"` is 07:30 on
the server's clock, so on a UTC container it is 07:30 UTC.

## Troubleshooting

| Symptom | Cause |
|---|---|
| `422` on create | a missing or out-of-range `minutes` (1–1440), more than 64 units, or a `settings` value outside [Control schema](Control-schema) bounds |
| `422` on a start | `at` is not `HH:MM`, `days` is over 30, the time has already gone today, `settings` was given, or `minutes` and `at` were both given |
| `404` on create | an unknown unit id — ids are **strings**, and sending one as a JSON number silently rounds it |
| a start is not in `GET /api/timers` | it is there with `?kind=all`; see [the endpoints](#the-endpoints) |
| an older app shows no start the panel set | as intended: an app from before starts does not see them, rather than mistaking one for a sleep timer |
| fires at the wrong time | server timezone; see above |
| never fires | check `GET /api/timers/status` for the runner's last pass. A start is dropped if the server was down more than 15 minutes past it |
| fired but nothing happened | the unit was unreachable. The attempt is real; the air conditioner declined |
