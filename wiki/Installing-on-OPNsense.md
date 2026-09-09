# Installing on OPNsense

A GUI plugin, `os-breeze-core`. One package, installed by hand, with a settings
page under **Services → Breeze Core**.

> ### Read this before installing
>
> **The GUI page has never run on a real firewall.** The plugin is built in a
> FreeBSD 14 root; its PHP is syntax-checked, its XML parses, the configd
> actions and the rc script are checked, and the binary is executed in that
> root. But nothing has rendered the Volt page, saved a setting through the
> model, or watched configd drive the service — that needs an actual OPNsense
> box, and there is not one here to test on.
>
> Treat this as the least exercised thing in the project. The **binary** is the
> same one every other platform gets and is well tested; it is the eighteen
> files of GUI plumbing around it that are unproven. If you try it, a report
> either way is genuinely useful.

## Install

```sh
pkg add https://aspic.salataputarica.hr.eu.org/opnsense/os-breeze-core-4.0.2.pkg
```

Then: **Services → Breeze Core**, set the listen address, and enable it.

## Why a single package and not a repository

Deliberate. Your firewall already has `pkg` pointed at its own mirrors, with
its own ABI and its own trust, and adding a third-party repository to a
firewall is a much bigger ask than fetching one file. So this is one file.

The trade is that upgrades are manual: `pkg add` the new URL when a release
comes out. Nothing will nag you.

## Why FreeBSD 14 specifically

OPNsense is FreeBSD 14. The ordinary FreeBSD package in this project is built
on 15, and **FreeBSD binaries run forward, not backward** — a 15 build will not
start on 14. So the plugin's binary is compiled in a FreeBSD 14 root, which is
also why it is a separate artifact rather than the same package with a
different wrapper.

If you install the plain FreeBSD package on OPNsense instead, expect it to fail
to execute, and expect the message to be unhelpful about why.

## What it puts on the firewall

Eighteen files. The interesting ones:

| Path | What |
|---|---|
| `/usr/local/bin/breeze-core` | the server itself |
| `/usr/local/etc/rc.d/breeze_core` | the rc script |
| `/usr/local/lib/breeze-core/serve.sh` | the wrapper configd starts |
| `…/mvc/app/models/OPNsense/BreezeCore/` | the settings model and its ACL |
| `…/mvc/app/views/OPNsense/BreezeCore/index.volt` | the page |
| `…/service/conf/actions.d/actions_breezecore.conf` | the configd actions |
| `/usr/local/etc/breeze-core/` | your configuration and state |

**No dependencies.** Nothing is compiled on the firewall, and nothing breaks
when OPNsense changes its own Python — which is the whole reason a plugin like
this can be a reasonable thing to install on a router at all.

## Pairing

The units still have to be found and paired, and the GUI does not do that part.
From a shell on the firewall:

```sh
breeze-core pair
```

Your air conditioners must be reachable from the firewall — usually the LAN
interface, not the WAN side. If the firewall is between VLANs, discovery is a
broadcast and will not cross them; use `breeze-core pair --ip 192.168.1.73` for
each unit instead.

Then pair a client as usual — see
[First run and pairing](First-run-and-pairing).

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

Given the warning at the top, this is the likely path. The parts are separable,
which helps:

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
the repository — lists exactly which eighteen checks pass and states plainly
which do not. It lints the PHP and XML in a `php:8.3-cli` container, because
neither the FreeBSD host nor the build root has PHP on it.
