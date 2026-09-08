# Coming from the Python line

4.x is the same project reimplemented, not a successor with a compatibility
layer. Same REST API, same `config.json` / `devices.json` / `programs.json` /
`timers.json`, same `AC_*` environment variables, same service name, same web
panel, same Android app. **It is meant to be installed *over* a 3.x deployment**,
and the package name does not change, so your package manager treats it as an
upgrade.

Nothing published for 3.x is being switched off. It stays exactly where it is on
[bolero](https://bolero.salataputarica.hr.eu.org/) and keeps installing.

> **Back up your config directory before any upgrade.** Not because this one is
> risky — it keeps the directory — but because a paired V3 unit's token and key
> cannot be obtained again, and that is true of every upgrade of anything.

## The short way

There is a migration script. It works out your OS, architecture and package
manager, backs up your configuration and repository files, swaps bolero for
aspic, and upgrades in place.

```sh
curl -fsSL https://aspic.salataputarica.hr.eu.org/migrate.sh -o migrate.sh
less migrate.sh            # the important step
sudo sh migrate.sh         # prints the plan, changes nothing
sudo sh migrate.sh --yes   # does it
```

**Read it first.** It is a script from the internet that runs as root, and the
[published checksum](https://aspic.salataputarica.hr.eu.org/migrate.sh.sha256)
proves only that the download was not corrupted — it comes from the same server,
so it is no evidence at all about intent. Running with no arguments prints
exactly what it found and what it would change, and stops.

What it does, once you pass `--yes`:

| | |
|---|---|
| detects | your OS, architecture and package manager — apt, dnf, zypper, pacman, apk, opkg, pkgin, `pkg` (FreeBSD) or `pkg_add` (OpenBSD) |
| backs up | the config directory and your repository files and keys, into a timestamped `0700` directory under `/var/backups`, with a **generated `ROLLBACK.sh`** holding the exact commands for *your* machine |
| removes | the bolero repository and its signing key |
| adds | the aspic repository and its key |
| upgrades | `breeze-core` in place — same package name, so config, device tokens, programs and timers are untouched and the service stays enabled |
| leaves alone | a service that was already stopped, and anything it cannot do safely |

**It upgrades rather than removing and reinstalling**, and that is deliberate: a
removal runs the *old* package's preremove script, which stops and disables the
service.

### What it looks like on a real server

From the maintainer's own machine, an x86-64 server with three units paired:

```
Total size of inbound packages is 1 MiB. Need to download 1 MiB.
After this operation, 56 MiB will be freed (install 3 MiB, remove 59 MiB).
```

Resident memory on that host went from **62 MB to 2.4 MB**, same three units,
before and after. The `python312` dependency went with it.

### OpenBSD is the one case it refuses

The Python line only ever installed from source on OpenBSD — a virtualenv full of
absolute paths is not something you can hand to `pkg_add` — so `pkg_add` owns
none of it and cannot replace it. Removing a service and a virtualenv is not
something the script will do behind your back, so it stops and prints the three
commands to do it yourself, keeping your configuration. Run it again afterwards
and it installs the native package.

## The long way

If you would rather do it by hand, it is four steps and no script:

1. **Stop the service.** `systemctl stop breeze-core` (or your init's
   equivalent). Do this first — see the trap below.
2. **Back up the config directory.** `cp -a /etc/breeze-core
   /root/breeze-core-backup`.
3. **Swap the repository.** Remove bolero's entry and key, add aspic's — the
   snippet for your package manager is on
   [the aspic index](https://aspic.salataputarica.hr.eu.org/).
4. **Upgrade.** `apt install --only-upgrade breeze-core`, `dnf upgrade
   breeze-core`, and so on. Then start it again.

## Traps worth knowing

- **Stop the old service before upgrading.** If something else is still holding
  port 8420, the new server fails to bind — and the old one goes on answering
  `/api/health`, so a *failed* start looks like a success. This bit us on FreeBSD
  with an orphaned Python process: the port answered, the version endpoint lied,
  and everything looked fine. Check identity, not liveness: `breeze-core
  --version`, or that the process on that port is the one you just installed.
- **Your env file keeps working, including the variables that no longer exist.**
  Unknown variables are ignored. `AC_DOCS`, `AC_ENROLL_LAN_ONLY`, `AC_CODE_TTL`,
  `AC_TOKEN_TTL_DAYS`, `AC_TRUSTED_HOSTS`, `AC_COMPRESSION` and
  `AC_AUTH_SKEW_SECONDS` have all either become constants or moved a layer up —
  [Configuration](Configuration) says what happened to each.
- **Enrolled devices stay enrolled.** `devices.json` is read as-is, so phones and
  browsers do not have to pair again.
- **`/docs` is gone.** There is no OpenAPI schema, because there is no framework
  generating one. The [REST API](REST-API) page is the reference, and it is
  complete.
- **The zsh tools are gone**, replaced by subcommands — `breeze-core diag`
  instead of `ac-diag.zsh`, `breeze-core approve` instead of `ac-approve.zsh`.
  The flags they took (`--base-url`, `--config`, `--auto`) are still accepted so
  existing scripts keep working. See [Command-line tools](Command-line-tools).
- **Nothing about the wire format changed**, so a mixed fleet is fine. An Android
  app or a browser talking to 4.x cannot tell, other than by
  `GET /api/version`.

## What you gain

- One executable, no interpreter, **no dependencies at all** — every Linux
  package and all three BSD packages declare none.
- Roughly 2 MB resident instead of roughly 60, and 3 MB on disk instead of 59.
- **Three more architectures with real packages** — riscv64, ppc64le and s390x
  were proof-of-concept builds in the Python line and are now published every
  release. See [Ports and architectures](Ports-and-architectures).
- A real OpenBSD package, where there was only a source install.
- `breeze-core control` for setting a unit from the shell without `curl`.
- A `/metrics` endpoint for Prometheus.

## What you lose

Stated plainly, because "drop-in replacement" should not have quiet exceptions:

- **The interactive `/docs` page.** No framework, no generated schema.
- **Three of the five container images.** The Python line published five —
  Alpine, Alpine+nginx, and two UBI variants. The native line publishes **two**:
  a distroless image on `scratch` and a `-debug` variant with a shell. See
  [Installing with containers](Installing-with-containers).

  All five of the old ones existed because an interpreter needs a distribution
  around it, and that reason is gone — so this is a replacement rather than a
  reduction. What genuinely goes away: `docker exec breeze-setup` (there is no
  shell; `docker run <image> pair` replaces it), and the bundled-nginx image,
  which is now a compose file pairing the server with a real proxy container.
  If you relied on a Red Hat base for policy rather than for glibc, say so —
  nothing replaces UBI.
- **Brotli response compression.** gzip only, deliberately — see
  [Configuration](Configuration).
- **The ability to loosen some settings.** LAN-only admin approval, the code TTL,
  the token TTL and the clock-skew window are no longer configurable. If you were
  relying on `AC_ENROLL_LAN_ONLY=0`, there is no equivalent.
