# Installing on OPNsense

A GUI plugin, `os-breeze-core`. One package, installed by hand, with a settings
page under **Services → Breeze Core**.

> ### What has been tested
>
> The plugin, GUI included, has been installed and driven on **OPNsense 26.7**
> (FreeBSD 15.1): installing and upgrading, saving settings, starting and
> stopping the service, editing units and approving and revoking clients. That
> first real run found five bugs in the 4.1.1 plugin as first published — it
> would not install on 26.7, and on 26.1 its buttons and its Save did nothing —
> and the package now on aspic is the fixed one.
>
> On **OPNsense 26.1** (FreeBSD 14), only the binary and the package have been
> checked: the binary runs on FreeBSD 14.3 and the package admits it, but the
> GUI has not been driven there. A report either way is genuinely useful.

## Install

```sh
pkg add https://aspic.salataputarica.hr.eu.org/opnsense/os-breeze-core-4.1.1.pkg
```

Then: **Services → Breeze Core**, set the listen address, and enable it.

## Why a single package and not a repository

Deliberate. Your firewall already has `pkg` pointed at its own mirrors, with
its own ABI and its own trust, and adding a third-party repository to a
firewall is a much bigger ask than fetching one file. So this is one file.

The trade is that upgrades are manual: `pkg add` the new URL when a release
comes out. Nothing will nag you.

## Why it is built on FreeBSD 14

OPNsense 26.1 is FreeBSD 14.3 and OPNsense 26.7 is FreeBSD 15.1. The ordinary
FreeBSD package in this project is built on 15, and **FreeBSD binaries run
forward, not backward** — a 15 build will not start on 14. So the plugin's
binary is compiled in a FreeBSD 14 root, and that one binary runs on both.

The package says so to `pkg` with the ABI pattern `FreeBSD:1[45]:amd64`: it
installs on 14 and 15, and is refused on anything else. **4.1.1 was stamped
FreeBSD 14 only**, so on 26.7 it fails with

```
pkg: wrong architecture: FreeBSD:14:amd64 instead of FreeBSD:15:amd64
```

which the next release fixes.

The plain FreeBSD package is not the plugin either way — it has no GUI page —
and on 26.1 it will not even execute.

## The page

**Services → Breeze Core** has three tabs:

- **Settings** — enable, listen address, port, and extra environment variables
  for the server, one `NAME=value` per line (see
  [Configuration](Configuration) for what they do). The address, the port and
  the data folder are set by the page and the plugin, so `BREEZE_HOST`,
  `BREEZE_PORT`, `AC_CONFIG` and the other store paths are refused there.
- **Units** — the air conditioners in Breeze Core's own `config.json`: name,
  address, port, id and, for V3 units, the token and key. Saving restarts the
  server. The API key and the V3 credentials are **never sent to the page** — it
  shows only whether each is set, and a new one is sent only when you type it —
  and `config.json` is **not** copied into the firewall's configuration or its
  backups.
- **Devices** — the phones and browsers enrolled with the server: approve a
  pairing code, see when each was last used, revoke one.

Settings are ordinary OPNsense settings and are in `config.xml` and its
backups. **Nothing secret goes there**, which is why the environment box says
not to put secrets in it.

## What it puts on the firewall

Nineteen files. The interesting ones:

| Path | What |
|---|---|
| `/usr/local/bin/breeze-core` | the server itself |
| `/usr/local/etc/rc.d/breeze_core` | the rc script |
| `/usr/local/lib/breeze-core/serve.sh` | the wrapper configd starts |
| `…/mvc/app/models/OPNsense/BreezeCore/` | the settings model and its ACL |
| `…/mvc/app/views/OPNsense/BreezeCore/index.volt` | the page |
| `…/mvc/app/controllers/OPNsense/BreezeCore/Api/` | its API: settings, service, units (`config.json`), devices |
| `…/service/conf/actions.d/actions_breezecore.conf` | the configd actions |
| `/usr/local/etc/breeze-core/` | your configuration and state |

**No dependencies.** Nothing is compiled on the firewall, and nothing breaks
when OPNsense changes its own Python — which is the whole reason a plugin like
this can be a reasonable thing to install on a router at all.

## Pairing

Discovering new units, and fetching V3 credentials from the cloud, is not in
the GUI. From a shell on the firewall:

```sh
breeze-core pair
```

Your air conditioners must be reachable from the firewall — usually the LAN
interface, not the WAN side. If the firewall is between VLANs, discovery is a
broadcast and will not cross them; use `breeze-core pair --ip 192.168.1.73` for
each unit instead.

Then pair a client as usual — see
[First run and pairing](First-run-and-pairing) — and approve its code on the
**Devices** tab instead of in a shell.

## Firewall rules

The one thing worth saying twice: **this is a firewall.** Breeze Core binds
where you tell it to, and the plugin's settings page sets that address. If you
bind it to a LAN address, the LAN reaches it and the WAN does not, which is
almost certainly what you want.

Do not expose it on the WAN side. If you want it from outside, terminate that
somewhere else and read [Exposing it safely](Exposing-it-safely) first —
particularly the part about a VPN being the better answer.

## Removing it

```sh
pkg delete os-breeze-core
```

Your configuration in `/usr/local/etc/breeze-core/` is **kept**, because a
paired unit's V3 credentials cannot be re-issued. Delete it by hand for a full
wipe.

## If the GUI page misbehaves

The parts are separable, which helps:

```sh
# does the binary work at all, independent of the GUI?
breeze-core --version
breeze-core diag

# does the service run, independent of configd?
service breeze_core status
service breeze_core start

# what does configd think it is doing?
configctl breezecore status
tail -f /var/log/configd/latest.log
```

If the binary and the rc script are fine and only the page is wrong, the
problem is in the plugin's PHP/XML, and everything still works from the shell
and the REST API in the meantime.

The plugin's own verification script — `packaging/opnsense/verify-plugin.sh` in
the repository — lists exactly which checks pass and states plainly which do
not. It lints the PHP and XML in a `php:8.3-cli` container, because neither
the FreeBSD host nor the build root has PHP on it.
