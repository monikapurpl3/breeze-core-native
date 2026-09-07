# Command-line tools

There is one, and it is the same executable that serves. `breeze-core` is the
server, the pairing tool, the control client, the diagnostic battery and the
admin interface — because they all need the protocol and the store code anyway,
and shipping one file is the whole point.

```
breeze-core serve      run the API + web panel (foreground)
breeze-core pair       discover units and write config.json
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

## What replaced the zsh tools

3.x shipped `tools/ac-diag.zsh` and `tools/ac-approve.zsh`: self-contained zsh
scripts that spoke only HTTP, so they could be copied to a laptop without
installing the Python package. That constraint no longer exists — the binary
*is* copyable, has no dependencies, and runs on every platform the packages
target. So the diagnostics and the approval flow are subcommands now.

If you have scripts written against the old tools, the flags they used are
accepted: `--base-url`, `--config`, and `--auto` (which `diag` takes and ignores,
because it never prompts in the first place).

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

`breeze-core control --help` prints the grammar.

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
kitchen          192.168.1.74     153931628470981
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
