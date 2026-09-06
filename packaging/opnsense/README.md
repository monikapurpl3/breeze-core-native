# packaging/opnsense — the firewall plugin

`os-breeze-core`: a `pkg` package that adds **Services > Breeze Core** to an
OPNsense firewall, alongside the daemon itself. Not a repository — one file,
installed by hand:

```
pkg add https://aspic.salataputarica.hr.eu.org/opnsense/os-breeze-core-4.0.0.pkg
```

A firewall already has `pkg` pointed at its own mirrors with its own ABI and its
own trust. Asking someone to add a third-party repository to that is a much
bigger request than asking them to fetch one file, so this ships as one file.

```bash
./packaging/opnsense/build-plugin.sh   [freebsd-builder-host]
./packaging/opnsense/verify-plugin.sh  [freebsd-builder-host]
```

## Why it is built separately from the FreeBSD package

**OPNsense is FreeBSD 14**, and the ordinary FreeBSD package here is built on 15.
FreeBSD binaries run forward, not backward, so a 15-built binary on a 14 firewall
is a gamble with no upside. `build-plugin.sh` therefore compiles inside a
FreeBSD 14 root on the builder and takes the package's ABI from that chroot, so
`FreeBSD:14:amd64` is true by construction rather than a string that can drift.

That chroot is also why the Python line existed at all here: OPNsense ships
neither rust nor pip nor any of the dependencies, so the Python plugin had to
vendor an entire interpreter and pinned `python311`. This one declares **no
dependencies**, so nothing is ever compiled on the firewall and nothing breaks
when OPNsense changes its Python.

## What is checked, and what is not

`verify-plugin.sh` checks the things that can fail silently:

- the four PHP files parse, and the four XML files are well-formed — in a
  `php:8.3-cli` container, because neither the FreeBSD host nor its FreeBSD 14
  root has php, and installing one on a VM to lint four files is a poor trade;
- the rc script, `serve.sh` and `setup.sh` parse as `sh`;
- every configd action `reconfigureAction()` can call exists — it does stop,
  `template reload`, then start *or* reload, so a missing `reload` fails only at
  the last step of saving a setting;
- `+TARGETS` renders to `/usr/local/etc/rc.conf.d/breeze_core`, the path
  `load_rc_config()` actually sources;
- the package's ABI is `FreeBSD:14:amd64`, it declares no dependencies, and its
  plist has the whole MVC tree in it;
- the binary runs inside the FreeBSD 14 root.

**It does not exercise the GUI, and it says so.** Nothing here renders the Volt
template, saves a setting through the model, or watches configd drive the rc
script. That needs OPNsense itself, and there is none here to test on — so the
GUI page is the least exercised thing this project ships, and the landing page
carries the same warning rather than implying parity with the other platforms.

## Traps

- **The plist is not optional.** Given only `-M` and `-r`, `pkg create` packages
  the manifest and nothing else, exits 0, and produces a ~1 KB "package".
  `build-plugin.sh` generates the plist from the staged tree (so it cannot drift
  from what was staged) and refuses to ship anything under 900 KB.
- **devfs must be mounted in the chroot.** Without it, cargo pipes source to
  `rustc -` on stdin, that read misbehaves, and rustc parses its own error
  message as source: `E0554: #![feature] may not be used on the stable release
  channel`. It looks like a toolchain problem and is not one.
- **Set the execute bits explicitly.** The source tar is produced on Windows,
  which records no POSIX execute bit. A non-executable `serve.sh` fails
  invisibly, because `daemon(8)` runs with `-f` and the permission error goes to
  `/dev/null`.
- **OPNsense caches the MVC/volt tree**, so the menu and the page do not appear
  until that cache is dropped. The post-install script drops it.
- **`${name}_user` is magic in `rc.subr`** — see `packaging/bsd/README.md`; the
  rc script here uses `breeze_core_runas` for the same reason.
- **The config path is an argument, not an export.** `daemon -u` resets the
  environment, so `serve.sh` is the only place that can set `AC_CONFIG_DIR`.
