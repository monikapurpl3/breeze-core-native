# Control schema

Used identically by the API, the web panel, the app, and the diagnostic CLI —
and unchanged from 3.x, because every existing client depends on it.

| Field | Values |
|---|---|
| `operational_mode` | `AUTO` `COOL` `DRY` `HEAT` `FAN_ONLY` |
| `swing_mode` | `OFF` `VERTICAL` `HORIZONTAL` `BOTH` — two physical flaps; an unsupported one is silently ignored by firmware |
| `target_temperature` | `16.0`–`30.0` in `0.5°` steps (Celsius on the wire; clients may display °F) |
| `fan_speed` | `20` / `40` / `60` / `80` / `100`, plus `102` = auto |
| `beep` | optional `bool` on `POST /control` only — whether the unit chirps on accept. Omitted ⇒ silent. Not part of the returned state. |

`POST /control` applies only the fields present in the body. `beep` defaults
to `false` when omitted (so schedules, curves and older clients stay quiet); a
client sends `beep: true` to make the unit chirp on accept.

## Where the bounds are enforced

In one place, and it is worth knowing which, because it is the reason a bad value
gets a `400` rather than reaching the unit: the request body is deserialised into
a typed struct and the ranges are checked there, before any LAN traffic happens.
The same struct is used by the HTTP route, the scheduler, the timer runner and
`breeze-core control`, so none of them can apply a value the others would refuse.

Out-of-range or unknown values are refused with `400` and a message naming the
field. An unknown *enum member* is a `400` too — not silently coerced to a
default, which is the failure mode that makes a client look like it worked.

## Reading enum values back

State comes back with the enum **names**, exactly as above — never the underlying
integers. This is load-bearing for clients and was a real bug in the Python line:
`IntEnum.__str__` changed in Python 3.11, so a `str(...)`-based serialiser
started returning bare numbers on newer interpreters. The Rust implementation has
no equivalent trap, since the wire form is written out explicitly, but the
contract is the same one and `breeze-core diag` still checks it.

## Temperatures on the wire

Always Celsius, always in 0.5° steps. °F is a display choice made by each client
and stored per browser or per phone; the server neither knows nor cares which one
you are looking at. A client sending Fahrenheit is a client sending a value
outside 16–30, which is a `400`.
