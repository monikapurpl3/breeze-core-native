# Installing with Homebrew

On a **Mac** (Apple silicon or Intel, macOS 13 or newer), or under **Homebrew
on Linux** (x86-64 or ARM64). One formula covers both, from a tap on aspic.
New in 4.3.1.

```sh
brew tap aspic/breeze https://aspic.salataputarica.hr.eu.org/homebrew/breeze.git
brew trust aspic/breeze
brew install breeze-core
```

**`brew trust` is Homebrew's step, asked once of any tap outside its own.**
Homebrew loads no formula from a third-party tap until you trust it. Without
it, `brew install` refuses with "Refusing to load formula
aspic/breeze/breeze-core from untrusted tap". It trusts the tap, not a
version, so `brew upgrade` picks up each release from then on.

## Setting it up

```sh
# 1. find the units on your network and write the config
breeze-core pair

# 2. choose the address it serves on
"${EDITOR:-vi}" "$(brew --prefix)/etc/breeze-core/breeze-core.env"

# 3. start it, now and at every login
brew services start breeze-core
```

Then open `http://<that address>:8420` and pair a browser —
[First run and pairing](First-run-and-pairing) from step 3.

- **The files** — `config.json`, the paired devices, programs and timers — are
  in `$(brew --prefix)/etc/breeze-core`, which is `/opt/homebrew/etc/breeze-core`
  on Apple silicon, `/usr/local/etc/breeze-core` on an Intel Mac and
  `/home/linuxbrew/.linuxbrew/etc/breeze-core` on Linux. The `breeze-core`
  command and the service both use it, so `pair` writes the config the service
  reads. Set `AC_CONFIG_DIR` to use another one.
- **`breeze-core.env` is the same file the Linux packages use**: `BREEZE_HOST`,
  `BREEZE_PORT`, `BREEZE_OPTS` and the rest, explained inside it. It is
  installed once and never overwritten by an upgrade. After changing it:
  `brew services restart breeze-core`.
- **`BREEZE_HOST=127.0.0.1`**, the default, is this machine only. Put the
  machine's LAN address there for phones to reach it, or keep 127.0.0.1 behind
  a reverse proxy — see [Reverse proxy and TLS](Reverse-proxy-and-TLS).
- **The log** is `$(brew --prefix)/var/log/breeze-core.log`.

## What `brew services` does, and does not

It runs Breeze Core **as you, while you are logged in**: a launchd agent on
macOS, a systemd user unit on Linux.

- On a Mac that stays logged in, that is all it needs.
- On a **Linux server**, prefer the [distribution packages](Installing-from-packages).
  They run it from boot as its own `breeze` account, which a user unit does
  not. For a user unit that survives logging out, run
  `loginctl enable-linger $USER` once.
- On a **Mac nobody logs in to**, `sudo brew services start breeze-core` runs it
  from boot as root. Pick one or the other: files the root service writes are
  then root's, and `breeze-core pair` as you cannot change them.

## What it installs

A prebuilt binary, the same one the GitHub release carries:

- **on a Mac**, cross-built for Apple silicon or Intel, with an ad-hoc
  signature. It is not notarised, which does not matter here: Homebrew
  downloads with `curl`, so nothing is quarantined. See
  [Ports and architectures](Ports-and-architectures#macos) for how it is built
  and tested;
- **on Linux**, the static musl binary every other Linux package carries, so it
  runs on any distribution, whatever its libc.

The formula pins each download by SHA-256, and the formula itself arrives over
aspic's HTTPS. The tap is a plain static git repository, served the same way as
the [Gentoo overlay](Installing-from-packages); there is no git server.

Every change to the formula, and every release, is installed by CI on
Apple-silicon and Intel Macs and on x86-64 and ARM64 Linux:

- `brew install` and `brew test`;
- `brew services start`, on a port set in `breeze-core.env`, so it shows the
  file is read;
- the CLI reaching the service's own config;
- a reinstall keeping the settings, and an uninstall keeping the store.

## Removing it

```sh
brew services stop breeze-core
brew uninstall breeze-core
brew untap aspic/breeze
```

**`$(brew --prefix)/etc/breeze-core` is kept**, as every package here keeps its
store: a V3 unit's token and key may not be issued again. Delete it by hand for
a full wipe.
