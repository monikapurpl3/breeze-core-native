# First run and pairing

Two different things are called "pairing", and confusing them is the usual
first stumble.

| | What it means | How often |
|---|---|---|
| **Pairing a unit** | teaching the server about an air conditioner on your LAN | once per unit |
| **Pairing a client** | authorising a browser, phone or CLI to control units | once per client |

Do them in that order. This page covers both.

## Before you start

You need the server installed and its configuration directory writable — see
[Installing from packages](Installing-from-packages) or the other install
pages. And you need the air conditioners **powered on and on the same subnet**
as the server, because discovery is a LAN broadcast and control is direct TCP
to each unit.

Nothing here needs the internet, with one exception noted below.

## 1. Pair the units

```sh
sudo breeze-core pair
```

It broadcasts, lists what answers, and writes `/etc/breeze-core/config.json` —
minting an API key on the way if there is not one already.

If a unit is missed, name it directly:

```sh
sudo breeze-core pair --ip 192.168.1.73
```

That is a sweep of exactly one host — the same unicast probe, no broadcast — so
a reply can only have come from the address you asked about. Your router's DHCP
lease table is the place to find the address.

**Re-running is safe.** It will not disturb units you have already paired, so
running it again later for the ones that were switched off is the intended
thing to do.

### If nothing is found

The usual causes, in order:

1. The units are on a **different subnet or VLAN** from the server. Broadcast
   does not cross either.
2. The units are **off**, or asleep in a way that stops them answering.
3. A **firewall on the server** is dropping the discovery replies.
4. Something at an address **answered but could not be decoded** — `pair` says
   so explicitly rather than staying silent, because otherwise you would go
   hunting the network for a unit that is right there.

### V3 units and the credential prompt

Some units use the V3 protocol, which needs a per-unit `token` and `key` before
the server can talk to them. `pair` asks for them, and you can press enter to
add the unit anyway and supply them later.

> **These are the irreplaceable part of your configuration.** They were issued
> once by a vendor cloud that no longer hands them out. If you lose
> `config.json`, a V3 unit has to be re-paired through the vendor app to get
> new ones.
>
> **Back up `/etc/breeze-core` before you do anything else**, and again
> whenever you add a unit:
>
> ```sh
> sudo cp -a /etc/breeze-core /root/breeze-core-backup
> ```

Breeze Core can fetch them from the vendor cloud once, as a last resort, and
that is the only step on this page that touches the internet. Everything
afterwards is local.

## 2. Set the bind address and start it

A fresh install listens on `127.0.0.1`, deliberately: it should not appear on
the network before you have decided it should.

```sh
sudo nano /etc/breeze-core/breeze-core.env
```

```sh
# this machine's own LAN address, for direct LAN use
BREEZE_HOST=192.168.1.10
BREEZE_PORT=8420
```

Keep `127.0.0.1` if a reverse proxy will front it. **Never `0.0.0.0`** — see
[Exposing it safely](Exposing-it-safely).

Then start it, with whatever your system uses:

```sh
sudo systemctl enable --now breeze-core     # systemd
sudo rc-update add breeze-core default && sudo rc-service breeze-core start
sudo ln -s /etc/sv/breeze-core /var/service/   # runit (Void)
sudo rcctl enable breeze_core && sudo rcctl start breeze_core   # OpenBSD
```

Check it came up, and check *identity* rather than liveness — if something else
holds the port, the old process answers `/api/health` and a failed start looks
like a success:

```sh
breeze-core --version
```

## 3. Pair a client

Now open `http://<BREEZE_HOST>:8420` in a browser.

The panel asks for the API key first. It is in `config.json`:

```sh
sudo grep -o '"api_key":"[^"]*"' /etc/breeze-core/config.json
```

That key is an **enrolment secret only**. It gets a client as far as *starting*
a pairing and no further, which is why it is safe to type into a phone.

The client then shows a code like **`BONG-W3GN`**, good for 60 seconds. Approve
it from the server, or anywhere on your LAN:

```sh
sudo breeze-core approve BONG-W3GN
```

The client polls, receives its own credential, and stores it. Done — that
browser can now control units and will not ask again.

Repeat for each phone and each browser. Every client gets its **own**
credential, revocable on its own without disturbing any other.

### The code was rejected

A wrong code, an expired code and an already-used code all return the same
error, deliberately — telling them apart would tell a guesser which of the
three they had achieved. In practice it is one of:

- more than 60 seconds passed (`AC_CODE_TTL` if you want longer)
- the code was already used once
- you approved from somewhere that is not a private address — that is a `403`,
  and behind a reverse proxy it is nearly always a missing `--behind-proxy`

Full model: [Authentication and pairing](Authentication-and-pairing).

## 4. Pair the machine's own CLI, optionally

`diag` and `control` need a credential too. Rather than passing flags every
time, enrol the CLI as its own client:

```sh
breeze-core login
```

It stores its credential separately from the server's stores — under
`$XDG_CONFIG_HOME/breeze-core/cli.json` by default — so `breeze-core control
'living room' heat 22` works afterwards with no flags at all.

## 5. Check the whole thing

```sh
breeze-core diag
```

Around thirty checks: connectivity, that a missing key is refused and a correct
one accepted, that the API and `config.json` agree on how many units exist,
that every unit's state is valid, that enums come back as names rather than
integers, and that the diagnostic endpoints leak no secrets.

## You are done. What next

- [The web panel](The-web-panel) — what the interface can do
- [Timers](Timers) — "off in 45 minutes"
- [Programs, schedules and curves](Programs-schedules-and-curves) — recurring
  and time-of-day behaviour
- [Exposing it safely](Exposing-it-safely) — before you make it reachable from
  outside
- [Troubleshooting](Troubleshooting) — when something is wrong

And back up `/etc/breeze-core`, if you have not yet.
