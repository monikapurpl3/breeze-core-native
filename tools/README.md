# Verification against the reference

These are the scripts that decide whether this server is really a drop-in
replacement. They are not unit tests and cannot run in CI: each one needs a
Breeze Core install and, for anything that touches a unit, real hardware on the
LAN.

The premise is the rule in `CLAUDE.md` — *verify against something external,
never against your own fixtures*. Everything here compares this server against
the Python one, running side by side against separate throwaway stores, and every
bug listed in `CLAUDE.md` was found this way rather than by reading code.

## Setting up a side-by-side pair

Never point either server at the live `/etc/breeze-core`. Copy the config out and
give each its own stores:

```bash
mkdir -p /tmp/native /tmp/python
sudo install -m 640 -o "$(whoami)" /etc/breeze-core/config.json /tmp/native/
cp /tmp/native/config.json /tmp/python/
for d in /tmp/native /tmp/python; do
  for f in programs devices timers; do printf '{"%s": []}' "$f" > "$d/$f.json"; done
done
```

Then start both, on ports the real service is not using:

```bash
AC_CONFIG=/tmp/native/config.json AC_DEVICES=/tmp/native/devices.json \
AC_PROGRAMS=/tmp/native/programs.json AC_TIMERS=/tmp/native/timers.json \
BREEZE_PORT=8421 ./breeze-core serve &
```

```bash
AC_CONFIG=/tmp/python/config.json AC_DEVICES=/tmp/python/devices.json \
AC_PROGRAMS=/tmp/python/programs.json AC_TIMERS=/tmp/python/timers.json \
/usr/lib/breeze-core/breeze-core serve --host 127.0.0.1 --port 8422 &
```

The scripts expect the API key in `/tmp/bcn.key` and a paired device token per
server in `/tmp/bcn.token` and `/tmp/bcn-py.token`.

Afterwards, `shred -uz` the copied `config.json` and both token files: they hold
the deployment's real API key.

## The scripts

| Script | What it settles |
|---|---|
| `diff-against-reference.py` | Every endpoint's response, **byte for byte** — key order included, since a client cannot tell the two apart only if the bytes match. Also fetches the panel from both and compares the files. |
| `diff-programs.py` | Favourites, schedules and curves: validation, normalisation, status codes, and the shape of everything accepted or refused. |
| `verify-v2-auth.py` | Ed25519 (`auth_version` 2) end to end — pairing, signing, replay rejection, forged signatures. Needs `pycryptodome`, so run it with Breeze Core's own virtualenv. |

`verify-v2-auth.py` is the one to run after touching authentication. A v1 bearer
token exercises almost none of the v2 path, so a v2-only break passes everything
else: `whoami` re-verified the request inside the route, spending the nonce a
second time and rejecting every Ed25519 client as a replay, and nothing noticed
until this script existed.

## What these do not cover

Firing a schedule, driving a curve and running a timer out all end in a command
to an actual air conditioner. There is no way to check those without hardware, so
they are verified by hand: create the program, watch the log, read the unit's
state back, and put the unit back the way it was. `breeze-core diag --auto`,
pointed at this server, is the closest thing to an automated pass over the whole
surface.
