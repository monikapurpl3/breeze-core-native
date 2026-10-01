# Installing on Termux

Breeze Core on an Android phone or tablet, inside the
[Termux](https://termux.dev) app: a package from aspic's Termux repository,
run as a service by termux-services. An old phone that stays at home on the
charger makes a perfectly good server.

> ### What has been tested
>
> The packages are installed and run in **termux-docker**, a real Termux
> userland (bionic, Termux's prefix, its own apt and dpkg, running as an app
> user). That's native on x86_64, and under QEMU for aarch64 and 32-bit arm.
> Each architecture goes through 39 checks:
>
> - installing with apt, which pulls in termux-services;
> - running under termux-services;
> - pairing a client;
> - upgrading in place;
> - removing and purging.
>
> **Not tested: a real phone.** A container has no Doze, no Wi-Fi, no
> phantom-process killer and no Termux:Boot. So staying alive with the screen
> off, starting at boot, and finding units over Wi-Fi are all untested. A
> report either way is genuinely useful.

## Install

Termux needs no root and no `sudo`: everything belongs to the app, under
`$PREFIX`.

```sh
mkdir -p $PREFIX/etc/apt/keyrings $PREFIX/etc/apt/sources.list.d
curl -fsSL https://aspic.salataputarica.hr.eu.org/aspic.asc \
  -o $PREFIX/etc/apt/keyrings/aspic.asc
echo "deb [signed-by=$PREFIX/etc/apt/keyrings/aspic.asc] https://aspic.salataputarica.hr.eu.org/termux stable main" \
  > $PREFIX/etc/apt/sources.list.d/aspic.list
pkg update && pkg install breeze-core
```

There are packages for **aarch64**, **arm** (32-bit) and **x86_64**, and apt
picks the right one. The key is the same one the Debian repository uses.

If termux-services was not installed before, this installs it. **Close and
reopen Termux once** afterwards: termux-services starts with a Termux session,
and until then `sv` has nothing to talk to.

## First run

```sh
breeze-core pair
```

This finds the air conditioners on the network and writes
`$PREFIX/etc/breeze-core/config.json`, as it does anywhere else; see
[First run and pairing](First-run-and-pairing). If it finds nothing over Wi-Fi,
name a unit directly with `breeze-core pair --ip 192.168.1.50`.

To let the rest of the network reach it, put the phone's Wi-Fi address in
`$PREFIX/etc/breeze-core/breeze-core.env`:

```sh
BREEZE_HOST=192.168.1.20
```

The phone's address can change when it reconnects, so give it a fixed one in
your router's DHCP settings. Left at `127.0.0.1`, only the phone itself can
reach the server.

## Running it as a service

The package installs the service **switched off**, so it can't start before
there is a config. Switch it on once it is paired:

```sh
sv-enable breeze-core          # starts now, and with every Termux session
sv status breeze-core
tail -f $PREFIX/var/log/sv/breeze-core/current
```

`sv-disable breeze-core` switches it off again. An upgrade leaves the choice
as it was: an enabled service is restarted onto the new version, and a disabled
one stays disabled.

### Keeping Android from stopping it

Android stops apps it thinks are idle, and a server with the screen off looks
idle. Two things help:

- **`termux-wake-lock`** keeps Termux running with the screen off. Termux shows
  a notification while it holds the lock. Exempting Termux from battery
  optimisation in Android's settings helps too.
- **The Termux:Boot add-on** runs scripts in `~/.termux/boot/` after a reboot.
  This one starts the services and takes the wake lock:

  ```sh
  mkdir -p ~/.termux/boot
  cat > ~/.termux/boot/start-services <<'EOF'
  #!/data/data/com.termux/files/usr/bin/sh
  termux-wake-lock
  . $PREFIX/etc/profile.d/start-services.sh
  EOF
  chmod +x ~/.termux/boot/start-services
  ```

## How it differs from the Linux packages

- **A real Android build.** The static Linux build does run on a phone, but
  Android has no `/etc/localtime`, so it would think it was in UTC, and every
  schedule and timer would fire hours out. Built for Android, it reads the
  phone's own timezone and Android's timezone database, and resolves names
  through bionic like any app.
- **Everything is under the prefix.** The binary is `$PREFIX/bin/breeze-core`,
  the store files are in `$PREFIX/etc/breeze-core`, which is the built-in
  default on Android, so no `AC_CONFIG_DIR` is needed. The service is
  `$PREFIX/var/service/breeze-core`.
- **No service account.** Android gives Termux one user, and the server runs as
  that user. The store directory is `0700`.
- **The Nerd screen says where it is running**: platform `android`, libc
  `bionic`, init `termux-services`, and the Android version.

Everything else is the same program: the same API, the same web panel, the same
four store files. A `config.json` copied from another server works as it is.

## Removing it

```sh
pkg uninstall breeze-core
```

This stops the service and leaves it switched off. As on every other platform,
`$PREFIX/etc/breeze-core` is **kept**, even on a purge: a V3 unit's token and
key cannot be issued again. Delete the directory by hand for a full wipe.
