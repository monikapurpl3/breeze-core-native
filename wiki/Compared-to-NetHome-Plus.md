# Compared to NetHome Plus

NetHome Plus is the vendor app that came with your air conditioner — or one of
its many rebadges: Midea Air, SmartHome, Comfee, Toshiba Home AC Control, and
others that are the same app with a different logo.

This page is about what changes if you run Breeze Core instead, honestly in
both directions.

## The one difference that matters

**Where the command goes.**

NetHome Plus sends it to a vendor server, which sends it to your air
conditioner. Your phone, on your sofa, three metres from the unit, talks to a
datacentre and back.

Breeze Core sends it straight to the unit over your LAN. Nothing leaves the
house.

Everything below follows from that.

| | NetHome Plus | Breeze Core |
|---|---|---|
| the path a command takes | phone → vendor cloud → unit | client → unit |
| works with the internet down | no | **yes** |
| works if the vendor shuts the service off | no | **yes** |
| latency | seconds, variable | ~1–2 s, and it is the unit's own speed |
| who can see your usage | the vendor | you |
| accounts | one, with the vendor | none |
| your data | their servers | four files on your machine |

## What you give up

Stated first, because a comparison page that only lists wins is an
advertisement.

- **You have to run a server.** A Raspberry Pi, an old laptop, a NAS, a router
  — it needs about 3 MB of disk and 2.5 MB of memory, so almost anything does.
  But it is a thing you maintain, and if it is off, nothing works.
- **Away-from-home access is your problem.** NetHome Plus works from anywhere
  by default, because everything goes via the cloud anyway. Here you need a VPN
  or a reverse proxy — see
  [Exposing it safely](Exposing-it-safely).
- **Setup is longer.** Discovering units, pairing clients, editing a
  configuration file. Call it twenty minutes rather than five.
- **No vendor support.** If it breaks, you read
  [Troubleshooting](Troubleshooting).
- **You still need the vendor app once**, for some units. V3 units need a
  `token` and `key` that only the vendor cloud issues. Breeze Core can fetch
  them for you during pairing, but they originate there.
- **Voice assistants and vendor integrations stop working**, because those go
  through the vendor's account. There is a REST API instead, which is more
  capable and more work.

## What you get

- **It keeps working.** No account to be suspended, no service to be
  discontinued, no app update that drops your model. The protocol is
  implemented locally and cannot be switched off remotely.
- **Every unit on one screen.** The vendor app is built around one unit at a
  time; the panel shows all of them, and `GET /api/units/state` returns the lot
  in one request.
- **Server-side scheduling that does not need a phone.** Schedules, curves and
  timers run in the server. NetHome Plus timers depend on the app and the cloud
  being reachable when they fire.
- **Temperature curves**, which the vendor app has no concept of: a setpoint
  that follows the time of day, interpolated. See
  [Programs, schedules and curves](Programs-schedules-and-curves).
- **A real API.** Anything you can script, you can automate. No cloud API keys,
  no rate limits, no terms of service.
- **Per-client credentials you can revoke.** Each phone and browser has its
  own, revocable individually. There is no shared password.
- **It runs on the hardware you have.** Six Linux architectures, three BSDs,
  Windows, OPNsense, OpenWrt, containers.
- **No telemetry**, and nothing to opt out of.

## Can I use both?

Yes, mostly — and it is worth knowing the one catch.

Midea firmware accepts **one control connection at a time**. So the vendor app
in the foreground on a phone can lock Breeze Core out, and the other way round.
The symptom is a `503` from the API, or the vendor app saying the unit is
offline. Close the other one and it clears.

Keeping the vendor app installed is sensible anyway: it is how you re-pair a V3
unit if you ever lose `config.json`.

## What Breeze Core does not do

- **It does not control anything but Midea-protocol air conditioners.** Not
  lights, not heat pumps, not other brands. It is one appliance family, done
  properly.
- **It is not a Home Assistant integration.** No dependency on it, and no
  intention. If you already run Home Assistant, its own Midea integrations may
  suit you better; this is for people who want the appliance and not the
  platform.
- **It does not touch the vendor cloud after pairing.** Which means it cannot
  do anything that only exists there.

## Is my unit supported?

If NetHome Plus (or a rebadge) controls it over Wi-Fi, very probably. The
protocol is the same across V1, V2 and V3 units, and all three are handled.

The test costs nothing:

```sh
sudo breeze-core pair
```

It broadcasts and lists what answers. If your unit appears, it works. If
something answers but cannot be decoded, `pair` says so explicitly rather than
staying silent — that report is useful, so please open an issue with it.

## Getting started

[First run and pairing](First-run-and-pairing), which assumes nothing.
