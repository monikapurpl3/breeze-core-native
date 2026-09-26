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

**OPNsense spans two FreeBSD majors**: 26.1 is FreeBSD 14.3 and 26.7 is
FreeBSD 15.1. The ordinary FreeBSD package here is built on 15, and FreeBSD
binaries run forward, not backward, so a 15-built binary on a 26.1 firewall is a
gamble with no upside. `build-plugin.sh` therefore compiles inside a FreeBSD 14
root on the builder - the oldest base it must run on - and that one binary
serves both; it links only `libc`, `libthr`, `libgcc_s` and (on 15) `libsys`.

The package's ABI is the pattern **`FreeBSD:1[45]:amd64`**, not the root's
`FreeBSD:14:amd64`. `pkg` fnmatches a package's ABI against the host's, so the
pattern installs on 14 and 15 and refuses 13 (where a 14 binary cannot run) and
16 (where nobody has run it) with its ordinary "wrong architecture" message.
4.1.1 was stamped with the root's own ABI, and every OPNsense 26.7 refused it:
`wrong architecture: FreeBSD:14:amd64 instead of FreeBSD:15:amd64`. Widen the
pattern only after running the binary on the new major. The build refuses a
root that is not FreeBSD 14, since the binary has to run on the oldest one.

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
- the package's ABI is `FreeBSD:1[45]:amd64`, it declares no dependencies, and
  its plist has the whole MVC tree in it;
- the binary runs inside the FreeBSD 14 root (OPNsense 26.1) **and** on the
  builder's own FreeBSD 15 (26.7);
- `[status]` is `script_output` with `errors:no`, and the post-install restarts
  configd;
- every form id is `<model name>.<section>.<field>`, names a node that exists,
  and is the id the page script reads;
- the model has no credential-shaped field, since the model is `config.xml`.

**It does not drive the GUI.** That was done by hand, on **OPNsense 26.7**
(FreeBSD 15.1, `pkg` 2.3.1, a VM on a host-only network), through Chrome's
DevTools: install and upgrade in place; the menu entry; loading and saving the
settings, including a refused environment line; configd rendering
`rc.conf.d/breeze_core` and `service.env`, and the running server receiving
that environment; start, restart, stop and the status buttons; adding, editing
and removing units, with a V3 unit's credentials surviving a rename and never
appearing in a response; approving a real pairing code and revoking the client.
The PHP and XML were linted with the firewall's own PHP 8.5. **OPNsense 26.1**
(FreeBSD 14) has only the checks above: the binary runs on 14.3 and the ABI
admits it, but its GUI has not been driven.

That session found five bugs 4.1.1 shipped with, all fixed here: 26.7 refused
the package; configd was never restarted, so every button failed until a
reboot; the page could not tell running from stopped; Save saved nothing; and
an upgrade left the service stopped.

## The page

Three tabs under **Services → Breeze Core**:

- **Settings** — enable, listen address, port, and extra environment variables
  (one `NAME=value` per line, validated by the model; the names the plugin sets
  itself are refused). These are in the OPNsense model, so in `config.xml`.
  configd renders them to `rc.conf.d/breeze_core` and
  `/usr/local/etc/breeze-core/service.env`, which `serve.sh` reads line by line
  and exports as data — it never sources it.
- **Units** — Breeze Core's own `config.json`, edited in place by
  `Api/UnitsController.php`. **Never through the model**: the API key and V3
  tokens and keys would land in `config.xml`, and so in every config backup, HA
  sync and cloud backup. **Never to the browser**: the page is told only whether
  each secret is set, a new one is sent only when typed, and V3 credentials are
  cleared only when asked. The server keeps `config.json` in memory and writes
  it itself, so a save stops the service, writes the file (atomically,
  `breeze:breeze` 640) and starts it again. Writing needs full admin.
- **Devices** — enrolled clients, approve a pairing code, revoke: through the
  running server's own admin API with the key from `config.json`, from the
  firewall itself, as `breeze-core devices`/`approve`/`revoke` do. Pending codes
  live only in the server's memory, so there is no other way to approve one.

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
- **The post-install must do what upstream's does.** opnsense/plugins'
  `Mk/plugins.mk` appends to every plugin: restart configd (it reads
  `actions.d` only at startup), run the model's migrations,
  `rc.configure_plugins` (which also drops the MVC cache) and a template
  reload. Without the configd restart every button answers "Action not
  allowed or missing" until a reboot.
- **`[status]` must be `script_output` with `errors:no`.** `statusAction()`
  reads its text; `script` returns only "OK", and without `errors:no` a stopped
  service returns "Execute error".
- **`$internalModelName` names the whole model.** `getAction()` returns
  `{name: model}` and `setAction()` applies `POST[name]` at the model's root,
  so the form ids are `breezecore.general.*`. Named after a section, every save
  reports success and sets nothing.
- **API responses are HTML-escaped** by OPNsense's framework
  (`Response.php`). The page inserts them as text, so it decodes once first, or
  a unit called `A & B` would be saved back as `A &amp; B`.
- **`${name}_user` is magic in `rc.subr`** — see `packaging/bsd/README.md`; the
  rc script here uses `breeze_core_runas` for the same reason.
- **The config path is an argument, not an export.** `daemon -u` resets the
  environment, so `serve.sh` is the only place that can set `AC_CONFIG_DIR`.
