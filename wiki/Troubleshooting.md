# Troubleshooting

Ordered by how often each one is the answer.

## Start here: three commands

```sh
breeze-core --version                  # which build is actually running
systemctl status breeze-core           # or rc-service / sv / rcctl
breeze-core diag                       # 30-odd checks against a running server
```

`diag` is the fastest way to a diagnosis. It checks connectivity, that a
missing key is refused and a correct one accepted, that the API and
`config.json` agree about how many units exist, that every unit's state is
valid, that enum values come back as names rather than integers, and that the
diagnostic endpoints leak no secrets.

> **One thing `diag` does that is worth knowing about.** Its last check POSTs
> `target_temperature: 99` to your first unit and expects a `422` — it is
> proving that a bad value is refused *before* it reaches the firmware. Under
> normal operation nothing is sent to the unit. But if that bounds check were
> ever broken, this is the check that would find out by sending 99 °C to a real
> air conditioner. If you are being careful with a live system, that is the one
> line to know about.

## The service will not start

**Check identity, not liveness.** If something else still holds port 8420, the
new server cannot bind — and the old one goes on answering `/api/health`, so a
*failed* start looks like a success:

```sh
breeze-core --version        # not curl /api/health
sudo ss -lntp | grep 8420    # who actually has the port
```

### "`… has no api_key`"

The server refuses to start without one, deliberately: a server with no key and
no units can only produce confusing `401`s. Run `sudo breeze-core pair`.

**Unless you are upgrading from an old install.** Releases before 2.5.0
defaulted to `/etc/meow-ac/config.json`, and 4.x looks in `/etc/breeze-core`.
The startup message says so if it spots one. **Do not run `pair` in that
situation** — pairing writes a fresh `config.json` and cannot recover a paired
V3 unit's token and key, which are not re-issuable. Copy the old file across,
or point `AC_CONFIG_DIR` at the old directory.

### "`cannot bind 192.168.1.10:8420`"

`BREEZE_HOST` names an address this machine does not have — a common result of
copying an env file between hosts, or of a DHCP lease changing. Either set it to
an address `ip -br addr` actually lists, or use `127.0.0.1` and put a reverse
proxy in front.

## A unit is unreachable — `503`

`503` means the request was valid and the *unit* did not cooperate. It is the
one status worth handling deliberately: retrying may work, sending something
different will not.

In order of likelihood:

1. **The unit is on a different subnet or VLAN** from the server. Control is
   direct TCP to port 6444 on the unit; nothing routes it for you.
2. **The unit's IP changed.** DHCP moved it. Give the unit a static lease, or
   find it again: `breeze-core pair` sweeps the LAN and reports what answers,
   and `GET /api/units/scan` does the same over HTTP. Then correct the address
   with `PATCH /api/units/{id}`, or in the panel.
3. **Something else is holding the unit's single session.** Midea firmware
   accepts one control connection at a time, so the vendor app in the
   foreground on a phone can lock out the server, and vice versa. Close it.
4. **The V3 credentials are stale.** Rarer, and it looks like a unit that
   authenticates and then refuses everything.

`GET /api/units/{id}/state` on an unreachable unit returns its **last known
values with `online: false`** rather than an error, because clients render it
straight into a card. So a card that looks stale and says offline is this.

## Nobody can pair — the code is rejected

A wrong code, an expired code and an already-used code all return the same
`404`. That is deliberate: telling them apart would tell a guesser which of the
three they had achieved.

- The code lives **60 seconds** by default (`AC_CODE_TTL`). Type it promptly.
- It is **single-use**. A second attempt with the same code fails even if the
  first succeeded.
- Approval must come from a **private source address**. The failure is a `403`,
  not a `401` — the credentials were fine, the *location* was not.

### Behind a reverse proxy, this is almost always the proxy

Without `--behind-proxy` (or `AC_BEHIND_PROXY=1`) the server sees the proxy's
own address, which is itself private — so every proxied request passes the
LAN-only check and it stops meaning anything at all. With it set, the proxy
must actually forward the real client address.

**The test.** `GET /api/system` reports how *this very request* reached the
server, so it answers the question directly rather than by inference. Read
`client_ip` and `behind_proxy_enabled` as a pair: a `client_ip` of `127.0.0.1`
while you are connecting from a phone means the real address is not arriving,
and a populated `forwarded_for` with `behind_proxy_enabled: false` means the
proxy is sending an address nobody is reading. Either way the LAN-only check is
deciding on the wrong address.

```sh
curl -s -H "X-API-Key: $KEY" -H "Authorization: Bearer $TOKEN" \
  http://server:8420/api/system \
  | grep -oE '"(client_ip|client_is_private|forwarded_for|behind_proxy_enabled)":[^,}]*'
```

See [Reverse proxy and TLS](Reverse-proxy-and-TLS).

## Auth fails and I cannot tell why

A rejected signature deliberately tells you nothing — except for clock skew,
which is the one genuinely self-healing case:

```json
{ "detail": { "error": "clock_skew", "retryable": true,
              "server_time": 1788889209.7, "max_skew_seconds": 60 } }
```

The server includes its own time so a client can compute its offset and retry.
If a device keeps failing this way, fix its clock (or the server's) rather than
widening `AC_AUTH_SKEW_SECONDS` — a wider window is a wider replay window.

`426 Upgrade Required` means the credential's auth version is below
`AC_MIN_AUTH_VERSION`. See [Signed auth (v2) migration](Signed-auth-v2-migration).

For a stubborn case there is a diagnostic that dumps what the server thought it
was verifying — the canonical string, byte for byte:

```sh
sudo systemctl stop breeze-core
sudo -u breeze BREEZE_DEBUG_AUTH=1 /usr/bin/breeze-core serve \
  --host 127.0.0.1 --port 8420
```

Compare it with what your client signed. A mismatch is nearly always the path
(the query string is part of it) or the body digest (**SHA3-512**, not SHA-2).

## Schedules and timers fire at the wrong time

**Everything scheduled uses the server's local clock.** Not the phone's, not
UTC.

In a container that means **UTC unless you set `TZ`**, and a schedule an hour
or two off is almost always this. Check what the server thinks:

```sh
curl -s -H "X-API-Key: $KEY" -H "Authorization: Bearer $TOKEN" \
  http://server:8420/api/system | grep -o '"utc_offset_seconds":[0-9-]*'
```

`0` on a machine that should not be UTC means the timezone database is not
reachable. The container images ship the whole of `/usr/share/zoneinfo` for
this reason; a hand-built image without it reports `0` and fires everything
wrong, silently.

## The stream shows nothing, or freezes

**The poller idles until somebody subscribes**, so with no subscribers the
server makes no LAN traffic at all and the first event after connecting can take
up to one tick (`AC_STREAM_TICK`, 5 s by default).

If events arrive in bursts, or stop while the connection stays open, a proxy is
buffering them. **Never compress or buffer this stream** — give nginx
`proxy_buffering off` or Caddy `flush_interval -1`.

If you have many units, one poll pass can outlast the tick: at roughly a second
per unit, more than about five means each tick starts later than the last. Raise
`BREEZE_BG_WORKERS` — see [Configuration](Configuration).

## Pairing fails with a 500, or nothing is saved

The service could not write its state directory. It runs as `breeze` and needs
to own `/etc/breeze-core`:

```sh
sudo ls -la /etc/breeze-core
sudo chown -R breeze:breeze /etc/breeze-core
sudo chmod 750 /etc/breeze-core
```

`config.json` is `0640`, the other three are `0600`, and the modes are enforced
on every write — so a file restored from a backup with loose permissions gets
tightened the next time the server saves it.

## Turning up the logging

An **access log is on by default**: one line per request with its status and
how long it took. `BREEZE_LOG=0` silences it.

`BREEZE_DEBUG=1` adds a full trace of every control command — what the client
sent, what was applied, and what the unit echoed back:

```sh
sudo systemctl stop breeze-core
sudo -u breeze BREEZE_DEBUG=1 /usr/bin/breeze-core serve --host 127.0.0.1 --port 8420
```

Run it in the foreground like that rather than editing the unit file: you get
the output where you can read it, and stopping is Ctrl-C. Nothing is written to
the config directory that a normal start would not write.

For the whole picture in one response — OS, init system, CPU, the four store
paths and their modes, the units and their cached capabilities, enrolled
credentials, scheduler and stream state, and how *this* request reached the
server — use `GET /api/system`. It never contains a secret, and `diag` checks
that omission rather than trusting it.

## Getting help

Include, at minimum:

- `breeze-core --version`
- the output of `breeze-core diag`
- `GET /api/system` (safe to paste — no secrets in it)
- the relevant lines from the access log

**Never paste `config.json`.** It holds the API key in plaintext and your
units' V3 credentials.
