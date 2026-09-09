# Programs, schedules and curves

Three kinds of stored behaviour, one endpoint family, one background thread.
All of it lives in the **server**, so it runs whether or not a panel is open or
a phone is on the network.

| Kind | What it is | Fires |
|---|---|---|
| **favourite** | a named set of settings | when you ask it to |
| **schedule** | settings at a time, on chosen days | on the matching minute |
| **curve** | a target temperature that follows the clock | continuously |

For "off in 45 minutes", you want a [Timer](Timers) instead — one-shot and
relative.

## Favourites

A named control payload and nothing more. "Night", "Away", "Film".

```sh
curl -X POST http://server:8420/api/programs \
  -H "X-API-Key: $KEY" -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"name": "Night", "kind": "favourite",
       "unit_ids": ["153931628470980"],
       "favourite": {"operational_mode": "COOL", "target_temperature": 24,
                     "fan_speed": 40}}'
```

Apply it with `POST /api/programs/{id}/apply`. Nothing happens on its own.

## Schedules

Settings at a time of day, on chosen days of the week.

```jsonc
{
  "name": "Weekday mornings",
  "kind": "schedule",
  "unit_ids": [],                       // empty means EVERY configured unit
  "schedule": [
    { "days": [0,1,2,3,4],              // 0 = Monday … 6 = Sunday
      "time": "07:00",                  // HH:MM, 24-hour, server-local
      "settings": {"power_state": true, "operational_mode": "HEAT",
                   "target_temperature": 21} },
    { "days": [0,1,2,3,4], "time": "08:30",
      "settings": {"power_state": false} }
  ]
}
```

Two things to note. **`days` is Monday-zero**, not Sunday-zero — worth checking
if a schedule seems to be a day out. And **an empty `unit_ids` means every
configured unit**, including units you add later, which is usually what you
want for something like "everything off at midnight".

Each entry carries its own `settings`, so one program can hold a whole day's
worth of transitions rather than needing one program per moment.

## Curves

A target temperature that follows the clock, interpolated between points you
give.

```jsonc
{
  "name": "Follow the day",
  "kind": "curve",
  "unit_ids": ["153931628470980"],
  "curve": {
    "operational_mode": "COOL",
    "fan_speed": 102,
    "points": [
      { "time": "08:00", "temperature": 24.0 },
      { "time": "14:00", "temperature": 26.0 },
      { "time": "22:00", "temperature": 23.0 }
    ]
  }
}
```

Between points the setpoint is **linearly interpolated** and rounded to the
nearest 0.5°, which is the resolution the units accept.

`operational_mode` and `fan_speed` apply for the whole curve and may be
omitted — they default to `COOL` and `102` (auto). Only the temperature
follows the clock.

**The curve is cyclic.** A curve with points at 08:00 and 22:00 still has to
say something at 03:00, and the answer is a position on the segment that wraps
through midnight — not "nothing". A single point means that temperature all
day.

**It is only re-applied when the rounded value changes.** A curve does not send
a command every tick; it sends one when the half-degree it wants is different
from the half-degree it last asked for. Over a day that is a handful of
commands, not thousands.

## The endpoints

| | Path | Does |
|---|---|---|
| `GET` | `/api/programs` | everything stored |
| `POST` | `/api/programs` | create |
| `GET` | `/api/programs/{id}` | one |
| `PUT` | `/api/programs/{id}` | replace |
| `DELETE` | `/api/programs/{id}` | delete |
| `POST` | `/api/programs/{id}/apply` | run it now, whatever kind it is |
| `GET` | `/api/programs/status` | scheduler health and last pass |

All need full auth — API key **and** a device credential.

Common to every kind:

| Field | Meaning |
|---|---|
| `name` | yours, for display |
| `kind` | `favourite`, `schedule` or `curve` |
| `enabled` | defaults **true**; set `false` to park one without deleting it |
| `unit_ids` | list of ids as **strings**; empty means every configured unit |
| `id` | assigned by the server |

`enabled: false` is the polite way to stop a schedule for a season. Deleting it
loses the times you worked out.

## How the scheduler runs

One background thread, waking every `AC_SCHED_TICK` seconds — 30 by default.
Each pass:

1. fires any **schedule** entry whose day and `HH:MM` match the current minute;
2. computes each **curve**'s setpoint and applies it if the rounded value moved.

Thirty seconds is coarser than the timer runner's fifteen, deliberately: **a
schedule only has to land inside the right minute**, while a timer is a promise
about a moment.

Both go through `apply_to_unit` — **the same path as an HTTP control
request** — so validation, per-unit locking and error handling are identical. A
schedule cannot do anything to a unit you could not do yourself with `curl`.

There is exactly **one scheduler thread**, and that is a correctness
requirement rather than a tuning choice: two would fire every schedule twice.
This is also why `BREEZE_BG_WORKERS` is the *poller's* fan-out and not a count
of background threads — see [Configuration](Configuration).

## Timezone, and the one real gotcha

Times are **server-local**, in the server's own clock. Not the phone's.

**In a container that is UTC unless you set `TZ`.** A schedule that fires an
hour or two off is almost always this. Check `utc_offset_seconds` on
`GET /api/system`: a `0` on a machine that should not be UTC means the timezone
database is not reachable, and everything time-based is quietly wrong.

There is no daylight-saving cleverness beyond what the system's timezone
database provides. `07:00` means 07:00 local, on both sides of a clock change.

## Where it is stored

`programs.json`, mode `0600`, in your configuration directory — a third store
alongside `config.json` and `devices.json`. It survives restarts and upgrades,
and it is in the directory you should be backing up.

## Troubleshooting

| Symptom | Cause |
|---|---|
| fires an hour off | server timezone, or `TZ` unset in a container |
| fires a day off | `days` is **Monday-zero** |
| never fires | `enabled: false`, or check `GET /api/programs/status` |
| a curve does nothing | fewer than one point, or the rounded setpoint has not changed |
| applies to the wrong units | an empty `unit_ids` means **all** of them |
| `404` on create | an unknown unit id — ids are strings; as JSON numbers they round |
