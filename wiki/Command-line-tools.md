# Command-line tools

There is one, and it is the same executable that serves. `breeze-core` is the
server, the pairing tool, the control client, the diagnostic battery and the
admin interface — because they all need the protocol and the store code anyway,
and shipping one file is the whole point.

```
breeze-core serve      run the API + web panel (foreground)
breeze-core pair       discover units and write config.json
breeze-core fetch      get V3 units' tokens from the Midea account   (4.3.0+)
breeze-core proxy      put nginx, Apache or Caddy in front, with TLS  (4.3.0+)
breeze-core control    set a unit from the shell, positionally
breeze-core diag       run the diagnostic battery against the server
breeze-core units      list the units the server knows about
breeze-core login      enrol this machine's CLI against a server
breeze-core approve    approve a pairing code            (admin, LAN-only)
breeze-core devices    list enrolled device credentials   (admin, LAN-only)
breeze-core revoke     revoke one by id                  (admin, LAN-only)
breeze-core --version  print the version and build commit
```

**With no arguments at all it serves**, so an init script may simply `exec` it.

## `pair`

```sh
sudo breeze-core pair                     # UDP broadcast; finds everything
sudo breeze-core pair --ip 192.168.1.73   # one unit, no discovery
sudo breeze-core pair --no-prompt         # don't ask for friendly names
sudo breeze-core pair --out ./config.json # write somewhere else
```

The one command that needs **no running server**. It writes `config.json`, and
the broadcast needs the same layer-2 network as the units — so in a container
that means host networking. Full walkthrough:
[First run and pairing](First-run-and-pairing).

## `fetch`

New in 4.3.0. A V3 unit answers nothing without a token and a key, and only
Midea hands them out — these days only to the account the unit is paired with.
`fetch` signs in to that account once, asks for the tokens, checks each one
on the unit, and saves it to `config.json`.

```sh
sudo breeze-core fetch                          # every unit still without a token
sudo breeze-core fetch kitchen                  # one, by name, address or id
sudo breeze-core fetch --cloud smarthome --account you@example.com
```

It explains what it is doing at every step, so the short version is enough
here:

1. **It asks each unit what it is.** It sends the same unicast probe as
   `pair --ip`, and skips a unit that turns out not to be V3. It also notices
   when a different unit now answers at that address.
2. **It asks which app the unit is paired with**: MSmartHome (outside China),
   NetHome Plus (the older app many units came with), Meiju (美的美居, in
   mainland China), or *not sure*.
3. **It asks which of Midea's clouds knows your account, by name only.** No
   password is sent for this lookup. So it can tell you "MSmartHome does not
   know this account" before you type a password, and which app does know it.
4. **It signs in** to a cloud that knows the account. The password is read
   without echo. It is used for that one sign-in, then dropped, and it is
   never saved, logged or shown. A mistyped password can be typed again,
   twice.
5. **It lists the units on the account**, so a unit paired to someone else's
   account is explained rather than reported as a bare refusal.
6. **It checks each token on the unit itself**: a V3 handshake and a state
   read, which change nothing on the unit. A token the unit refuses is not
   saved unless you say so.
7. **It backs up `config.json`** to `config.json.before-fetch`, saves the
   tokens, and prints the restart command for your init system.

It exits `0` when every unit it was asked about got a token, `1` otherwise,
and `2` on bad flags. For a script, `--password-stdin` reads the password as
one line of standard input.

### If your units came with NetHome Plus

NetHome Plus still lets you sign in, but **since August 2026 it has refused to
hand out tokens** — for every unit, its owners' included. `fetch` tries once
anyway, in case that changes, and then says what does work:

- **If MSmartHome knows the same account**, it offers to use MSmartHome
  instead. That works if the unit is paired there too.
- **Otherwise the unit has to move to MSmartHome.** A NetHome Plus login is not
  an MSmartHome account: Midea keeps them on separate systems. So you install
  MSmartHome and sign in, or create an account (the same email is fine). Then
  you add the air conditioner there and run `fetch` again.

**Only move the units that need a token now.** Pairing a unit again gives it a
new token, so the one Breeze Core already holds for it will most likely stop
working. `fetch` lists the units that already have their token and key, so you
know which ones to leave alone.

If you already have a unit's token and key from somewhere else (an old Home
Assistant setup, or `msmart-ng`'s output), `breeze-core pair --ip ADDRESS` takes
them typed in, and no cloud is involved.

> **Keep a copy of `config.json` off the machine.** A unit never forgets its
> token, but Midea may stop handing them out altogether. If that happens, your
> copy is the only one there is.

## `control`

New in 4.x, and the reason is that "turn the kitchen off" should not require
`curl` and a JSON body.

```sh
breeze-core control 'kitchen'     cool 25.5 both       auto turbo 15m
breeze-core control 'living room' heat 30   horizontal low  eco
breeze-core control 'back room' dry  26   vertical   high      25m
breeze-core control 'kitchen'     off
```

The grammar is positional — `NAME TYPE TEMPERATURE FLAP FAN EXTRA TIMER` — because
the order is the order somebody thinks in. Every slot after the name is optional
and `none` is legal in any of them, so this is a perfectly good sentence:

```sh
breeze-core control kitchen none none none none none 30m
```

"Switch off in half an hour, change nothing else." A field left out or set to
`none` is **not sent at all**, and the API applies only the fields present, so
nothing else about the unit is disturbed.

`none` is not the only way to say "leave this alone" — `-`, an empty string,
`same` and `keep` all mean the same thing, so whichever one you reach for works.
Temperatures may carry a `C`, a `c` or a `°`, and fan speeds a `%`; they are
trimmed rather than rejected.

The name is matched case-insensitively and **by prefix when that is
unambiguous**, because `'Living Room'` is tedious to type exactly and `liv` is
not. Anything less tidy is an error that tells you what it found rather than
guessing:

```
breeze-core: 'room' is ambiguous -- it could be Living Room or Back Room
breeze-core: no unit matches 'kitchn'. Known units: Kitchen, Living Room, Back Room
```

A unit id works wherever a name does, for scripts and for the case where two
units genuinely share a prefix.

It answers with one line of what the unit now reports. If the unit is off,
the line still says what it is set to, so a temperature changed while it is
off is confirmed:

```
Back Room: off, set to heat, 29.5 °C, swing both, fan auto; indoor 24.0 °C
```

**A change the unit refuses is said in words.** The unit answers either way,
so the line above alone made a refusal look like success:

```
Back Room: the air conditioner didn't accept eco; the unit refused it, not Breeze Core. Many units offer eco only while cooling, not while heating.
```

The exit code is still 0: the request was carried out, and the unit made its
choice. The output is plain text throughout, with no colour codes, so it
reads the same in a screen reader as on screen.

`breeze-core control --help` prints the grammar.

## `proxy`

New in 4.3.0. It puts nginx, Apache or Caddy in front of Breeze Core, with a
name and a Let's Encrypt certificate. It asks before every step and records
every change, so that `--undo` can take all of it back out.

```sh
sudo breeze-core proxy                                   # asks everything
sudo breeze-core proxy --server caddy --domain breeze.example.org
breeze-core proxy --dry-run                              # the plan, changing nothing
sudo breeze-core proxy --undo
```

On Linux it does the whole job. On Windows it opens the installer's Caddy
wizard, and anywhere else it points to the wiki. What it does, step by step,
and what it writes:
[Reverse proxy and TLS](Reverse-proxy-and-TLS#or-let-breeze-core-proxy-do-it).

## `diag`

The closest thing to a test suite: connectivity, auth posture (that a missing key
is refused and the right one accepted), unit-count parity between the API and the
config, per-unit state validity and latency, and enum sanity. It exits non-zero
if anything failed, so it works in a cron job or a monitoring check.

```sh
breeze-core diag                                        # uses the CLI profile
breeze-core diag --config /etc/breeze-core/config.json  # on the server itself
breeze-core diag --base-url http://192.168.1.10:8420    # against another host
```

Each finding is `ok`, `warn` or `fail`, and the distinction is deliberate:

- **A missing outdoor probe is skipped, not failed.** Plenty of units do not have
  one, and reporting that as a fault trains people to ignore the output.
- **A sentinel reading is a warning with an explanation.** Some firmware reports
  an obviously-impossible temperature rather than "unknown".
- **A bare integer where an enum name belongs is a failure.** That is the 3.11
  `IntEnum` bug, and it stays checked because a client seeing `2` instead of
  `COOL` breaks silently.
- **A state missing its fields fails rather than passing quietly.**

### `--nerd`: everything it saw, in a file

New in 4.3.0. This runs the same checks and also writes what `diag` saw to a
JSON file:

- every check, with its verdict;
- the summary counts;
- the server's `/api/version` and `/api/system`;
- how long the round trip took.

```sh
breeze-core diag --nerd                     # breeze-core-nerd-<host>-<date>-<time>.json, here
breeze-core diag --nerd /tmp/report.json    # or a name of your own
```

This is the file to attach to a bug report. It contains **no keys or tokens**:
`/api/system` never contains a secret, and `diag` checks that. It does contain
the server's hostname, LAN addresses and file paths, so look it over before
posting it anywhere public. The file is created readable by you only.

> **`diag` posts a deliberately out-of-range temperature** as part of checking
> input validation. That is a control request, which the unit refuses — but if
> you are working under a "do not touch the air conditioners" rule, this command
> is not exempt from it.

## `units`

```sh
breeze-core units --config /etc/breeze-core/config.json
```

The unit list as the server holds it — name, address and id, one per line, in a
column layout sized to the longest name:

```
Living Room      192.168.1.73     153931628470980
Kitchen          192.168.1.74     153931628470981
```

It answers "is this unit in the config at all", which is the first thing worth
knowing when a client says one is missing. It does **not** contact the units, so
it says nothing about whether they are reachable — `breeze-core diag` is what
does that.

The ids are worth keeping in view, because `control` takes one wherever it takes
a name.

## `login`

```sh
breeze-core login --base-url http://192.168.1.10:8420
```

Enrols *this machine's* CLI as a device: it starts the pairing handshake, prints
the code for an admin to approve, and on success stores its own credential in the
CLI profile (`$XDG_CONFIG_HOME/breeze-core/cli.json`, or `%APPDATA%` on Windows).
After that `diag`, `units` and `control` need no flags.

This is a client-side credential like any other. It appears in `breeze-core
devices` and can be revoked from anywhere.

## `approve` / `devices` / `revoke`

```sh
breeze-core approve BONG-W3GN
breeze-core devices
breeze-core revoke 7f3c9a21
```

Admin operations, and **gated to private addresses with no way to switch that
off**. They read the API key from `--config` when it is given, which is how they
work on the server itself with no profile of their own:

```sh
sudo breeze-core approve --config /etc/breeze-core/config.json BONG-W3GN
```

Behind a reverse proxy these need the real client IP to reach the server, or
every request looks like it came from the proxy — see
[Reverse proxy and TLS](Reverse-proxy-and-TLS).

## Where the key comes from

Every subcommand that talks to a server needs the API key, and it looks in two
places in this order:

1. `--config PATH`, reading the key out of the server's own `config.json`. This
   is the server-side path, and it is why `sudo` appears above.
2. The CLI profile written by `login`. This is the laptop path.

With neither, it says so rather than failing obscurely:

```
breeze-core: no API key: pass --config /etc/breeze-core/config.json, or run
             `breeze-core login` first
```
