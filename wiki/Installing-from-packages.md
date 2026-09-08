# Installing from packages

Add the repository once, then install and update through your package manager
like anything else. Snippets per distribution live on
[the aspic index](https://aspic.salataputarica.hr.eu.org/); this page is what
they do and what to check afterwards.

**Every package declares no dependencies at all.** Not "few" — none. There is
one static executable with the panel compiled into it, so there is no
interpreter, no virtualenv, no wheels and no `python3-*` to satisfy. The same
file installs on Debian 12, Alpine and RHEL 8.

## What you get

| Family | Manager | Architectures |
|---|---|---|
| Debian · Ubuntu · Mint · Pop!_OS · Raspberry Pi OS · Devuan | `apt` | amd64, arm64, armhf, riscv64, ppc64el, s390x |
| Fedora · RHEL · AlmaLinux · Rocky · CentOS Stream | `dnf` | x86_64, aarch64, armv7hl, riscv64, ppc64le, s390x |
| openSUSE Leap · Tumbleweed · SLES | `zypper` | the same rpms |
| Arch · Manjaro · Artix · EndeavourOS | `pacman` | x86_64, aarch64, armv7h |
| Alpine | `apk` | x86_64, aarch64, armv7, riscv64, ppc64le, s390x |
| OpenWrt | `opkg` | x86_64, 3 × aarch64, arm_cortex-a7, riscv64 |

Plus [the BSDs](Installing-on-the-BSDs), [OPNsense](Installing-on-OPNsense),
[Windows](Installing-on-Windows) and [containers](Installing-with-containers).

## The repository is signed, and that is the point

Five signing keys, because five package managers disagree about what a
signature is:

| Manager | Key |
|---|---|
| apt, dnf, zypper, pacman | GPG (RSA-4096) |
| apk | its own RSA key — apk has no ed25519 option |
| opkg | `usign`, OpenWrt's own signer |
| FreeBSD `pkg` | RSA, verifying the catalogue |
| OpenBSD `pkg_add` | `signify` |

**Nothing is signed on the host that serves them.** Packages and indexes are
signed on a workstation and pushed as static files, so a compromise of the
server can serve you something stale or missing but cannot forge a package your
package manager will accept. That is what the key in each snippet is for, and it
is why the snippets add a key rather than passing `--allow-unsigned` or the
equivalent.

The repository key is **RSA-4096 and not ed25519** for one concrete reason: rpm
4.14, which is what RHEL/AlmaLinux/Rocky 8 ship, cannot import an ed25519 GPG
key at all. Every release is verified against AlmaLinux 8 as well as 9 for
exactly that.

## After installing

The package installs the binary, a service unit, and an editable env file. It
does **not** start the service, because there is nothing to serve yet.

```sh
# 1. find the units on your network and write the config
sudo breeze-core pair

# 2. bind somewhere useful -- the default is loopback
sudoedit /etc/breeze-core/breeze-core.env

# 3. start it
sudo systemctl enable --now breeze-core
```

On other init systems, step 3 is:

| Init | Command |
|---|---|
| systemd | `systemctl enable --now breeze-core` |
| OpenRC (Alpine) | `rc-update add breeze-core && rc-service breeze-core start` |
| procd (OpenWrt) | `/etc/init.d/breeze-core enable && /etc/init.d/breeze-core start` |

**Loopback is the default on purpose.** A fresh install should not appear on the
network before you have decided it should. Edit `BREEZE_HOST` to the LAN address
you want to reach, or leave it and put a reverse proxy in front — see
[Reverse proxy and TLS](Reverse-proxy-and-TLS). `0.0.0.0` is not the right
answer; pick the interface.

Full walkthrough of pairing, including what to do about V3 units:
[First run and pairing](First-run-and-pairing).

## Upgrading

Whatever your package manager already does. The package name does not change
between 3.x and 4.x, so an upgrade is an upgrade: your config, device tokens,
programs and timers are untouched and the service stays enabled.

Coming from the Python line, there is one extra step — swapping the repository —
and a script that does it: [Coming from the Python line](Coming-from-the-Python-line).

**Stop the service before a manual upgrade.** If something else still holds port
8420 the new server cannot bind, and the old one goes on answering
`/api/health` — so a *failed* start looks like a success. Check identity, not
liveness: `breeze-core --version`.

## Where everything lands

| Path | What |
|---|---|
| `/usr/bin/breeze-core` | the executable, ~2.5 MB |
| `/etc/breeze-core/` | the four store files ([Configuration](Configuration)) |
| `/etc/breeze-core/breeze-core.env` | the editable service configuration |
| `/usr/lib/systemd/system/breeze-core.service` | the unit (deb/rpm/pacman) |
| `/etc/init.d/breeze-core` | OpenRC (apk) or procd (OpenWrt) |
| `/usr/lib/breeze-core/breeze-core` | a **symlink** to the above, kept for compatibility |

That symlink is where the Python package put its executable, and it is kept
because the reference's own unit file, its OpenRC script and a year of people's
scripts all name that path. A drop-in replacement should not break a path it can
keep for the cost of one link.

`/usr/bin`, not `/usr/lib` — and on a SELinux system that is not cosmetic:
`/usr/lib` is labelled `lib_t`, which gets no domain transition, so a service
started from there runs in the wrong context.

The packages create a `breeze` system user and the service runs as it, never as
root. `/etc/breeze-core` is `0750` owned by that user, `config.json` is `0640`
and the other three stores are `0600` — enforced by the server on every write,
so a file restored from a loose backup gets tightened rather than staying loose.

## Removing it

```sh
sudo apt remove breeze-core     # or dnf/zypper/pacman/apk/opkg equivalent
```

**Your configuration is kept.** `/etc/breeze-core` survives removal, on every
packager, because a paired V3 unit's token and key cannot be re-issued and
deleting them on `remove` would be unrecoverable. Purge it by hand if you
genuinely mean to:

```sh
sudo rm -rf /etc/breeze-core
```

Back it up first if there is any chance you will want those units again.

## Verifying a package by hand

If you would rather check before installing:

```sh
# deb
dpkg-deb -I breeze-core_*_amd64.deb        # no Depends line
dpkg-deb -c breeze-core_*_amd64.deb        # what it will write

# rpm -- note -K, not --checksig alone
rpm -K breeze-core-*.x86_64.rpm            # signature and digest
rpm -qpR breeze-core-*.x86_64.rpm          # requires: nothing of ours
```

For rpm specifically: query with `rpm -K`. An RSA signature lands in the
package's RSAHEADER and an ed25519 one in DSAHEADER, so a tool looking in the
wrong place reports an unsigned package that is in fact signed.
