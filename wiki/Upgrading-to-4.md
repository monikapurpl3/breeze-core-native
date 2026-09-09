# Upgrading to 4.x

From any earlier release. The package name has never changed, so for most people
this is an ordinary upgrade — your units, your API key, your enrolled phones and
your programs all stay where they are.

**Which route you take depends on how the old version got installed**, not on
which version it is:

| You have | Route |
|---|---|
| a package from the repository (2.5.0 and later) | [the short way](#the-short-way) — one command |
| a package, but you want to do it by hand | [the long way](#the-long-way) |
| a `pip` / virtualenv install (1.x up to 2.4.x) | [from a source install](#from-a-source-install) |
| a container | [Installing with containers](Installing-with-containers) |
| Windows | [Installing on Windows](Installing-on-Windows) |

> **Back up your config directory first.** Not because the upgrade is risky — it
> keeps the directory — but because a paired V3 unit's token and key were issued
> once by a cloud that no longer hands them out. If you lose `config.json`, those
> units have to be re-paired through the vendor app.
>
> ```sh
> sudo cp -a /etc/breeze-core /root/breeze-core-backup
> ```

## The short way

```sh
curl -fsSL https://aspic.salataputarica.hr.eu.org/migrate.sh -o migrate.sh
less migrate.sh            # the important step
sudo sh migrate.sh         # prints the plan, changes nothing
sudo sh migrate.sh --yes   # does it
```

**Read it first.** It is a script from the internet that runs as root, and the
[published checksum](https://aspic.salataputarica.hr.eu.org/migrate.sh.sha256)
only proves the download was not corrupted — it comes from the same server, so
it is no evidence about intent. Run with no arguments and it prints exactly what
it found and what it would change, then stops.

What `--yes` does:

| | |
|---|---|
| detects | your OS, architecture and package manager — apt, dnf, zypper, pacman, apk, opkg, pkgin, `pkg` or `pkg_add` |
| backs up | the config directory, your repository files and keys, into a timestamped root-only directory, with a **generated `ROLLBACK.sh`** holding the exact commands for *your* machine |
| switches | the repository over |
| upgrades | in place — same package name, so nothing in your config directory is touched and the service stays enabled |
| leaves alone | a service that was already stopped, and anything it cannot do safely |

It **upgrades rather than removing and reinstalling**, deliberately: a removal
runs the old package's own preremove script, which stops and disables the
service.

### It refuses on OpenBSD

There was never an OpenBSD *package* before 4.x — only a source install, because
a virtualenv full of absolute paths is not something you can hand to `pkg_add`.
So `pkg_add` owns none of it and cannot replace it. The script stops and prints
the three commands to remove the old tree yourself, keeping your configuration.
Run it again afterwards and it installs the package.

## The long way

Four steps.

1. **Stop the service.** `sudo systemctl stop breeze-core`, or your init's
   equivalent. Do this first — see the traps below.
2. **Back up the config directory**, as above.
3. **Switch the repository.** The snippet for your package manager is on
   [the aspic index](https://aspic.salataputarica.hr.eu.org/).
4. **Upgrade and start.** `sudo apt install --only-upgrade breeze-core`,
   `sudo dnf upgrade breeze-core`, and so on.

## From a source install

If you installed with `pip` or from a tarball into a virtualenv — anything
before 2.5.0, when the self-contained packages arrived — there is no package for
your package manager to upgrade. The old tree has to go by hand, and the
migration script refuses to do it for you rather than guessing at what it can
delete.

```sh
# 1. stop and disable whatever runs it
sudo systemctl stop breeze-core meow-ac 2>/dev/null
sudo systemctl disable breeze-core meow-ac 2>/dev/null

# 2. find where your configuration actually is -- see the warning below
ls -la /etc/breeze-core /etc/meow-ac 2>/dev/null

# 3. back it up somewhere outside both
sudo cp -a /etc/meow-ac /root/breeze-config-backup    # adjust the path

# 4. install the package, which brings its own service unit
#    (repository snippet on the aspic index, then)
sudo apt install breeze-core

# 5. put the configuration where 4.x looks for it
sudo cp -a /root/breeze-config-backup/. /etc/breeze-core/
sudo chown -R breeze:breeze /etc/breeze-core

# 6. remove the old tree and its unit
sudo rm -rf /opt/breeze-core /opt/meow-ac
sudo rm -f /etc/systemd/system/meow-ac.service
sudo systemctl daemon-reload

sudo systemctl enable --now breeze-core
```

> ### If your config is in `/etc/meow-ac`, read this before running `pair`
>
> Releases before 2.5.0 defaulted to **`/etc/meow-ac/config.json`**. 4.x looks in
> `/etc/breeze-core` and will tell you there is no API key and to run
> `breeze-core pair`.
>
> **Do not run `pair` in that situation.** Pairing writes a fresh
> `config.json` — it cannot recover a V3 unit's existing token and key, and those
> are not re-issuable. Copy the old file across, or point `AC_CONFIG_DIR` at the
> old directory, and the units come back untouched.
>
> 4.x prints a note when it spots a config in the old location, but check for
> yourself before typing `pair`.

## Traps worth knowing

- **Stop the old service before upgrading.** If something still holds port 8420
  the new server cannot bind — and the old one goes on answering `/api/health`,
  so a *failed* start looks like a success. Check identity, not liveness:
  `breeze-core --version`.
- **Your `breeze-core.env` keeps working**, including any variable 4.x no longer
  reads. Unknown variables are ignored, so nothing needs editing to start.
- **Enrolled phones and browsers stay enrolled.** Nobody re-pairs.
- **Coming from before 3.0.0?** Your clients use the older bearer credential,
  which 4.x accepts. Nothing to do. If you want them all signing their requests,
  see [Signed auth (v2) migration](Signed-auth-v2-migration) — but that is
  optional and separate.
- **Coming from before 2.6.0?** The `ac-diag.zsh` and `ac-approve.zsh` scripts
  are gone. Everything they did is a subcommand now — `breeze-core diag`,
  `breeze-core approve` — and the flags they took are still accepted, so
  existing scripts keep working. See [Command-line tools](Command-line-tools).
- **Coming from a container?** The image names and tags changed. See
  [Installing with containers](Installing-with-containers).

## What if it goes wrong

The migration script writes a `ROLLBACK.sh` next to its backup, with the actual
commands for your machine rather than "restore from your backup":

```sh
sudo sh /var/backups/breeze-core-migration-*/ROLLBACK.sh
```

If you upgraded by hand, downgrading is whatever your package manager calls it,
pointed back at the old repository. Your config directory is untouched either
way.

Something not working rather than not installing?
[Troubleshooting](Troubleshooting), and a detailed log is one environment
variable away:

```sh
sudo systemctl stop breeze-core
sudo -u breeze BREEZE_DEBUG=1 /usr/bin/breeze-core serve --host 127.0.0.1 --port 8420
```

That logs every request with its status, and traces every control command from
what the client asked for through to what the unit sent back.
