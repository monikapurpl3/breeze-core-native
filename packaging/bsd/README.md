# packaging/bsd — the BSDs

There is no cross-build here, and that is the whole story: Zig bundles a libc for
every Linux target this project ships, and none for FreeBSD, NetBSD or OpenBSD.
So every BSD binary here is compiled on a BSD machine. All three are built, and
all three carry the complete binary with TLS — `ring` compiles on each of them
without help, so none of these is a reduced build.

## NetBSD — done, and verified on the machine

```bash
./packaging/bsd/build-netbsd.sh   [user@host]   # compile, package, fetch back
./packaging/bsd/verify-netbsd.sh  [user@host]   # install, upgrade, run, stop
```

`build-netbsd.sh` sends the source, builds with the pkgsrc `cargo`, packages with
base-system `pkg_create(1)`, generates the `pkg_summary` a pkgin feed needs, and
brings all three back. Nothing is emulated.

**The full build works, TLS included.** `ring` compiles on NetBSD 11 without
help, so the NetBSD package is the same ~2.5 MB binary as everywhere else rather
than a reduced build — and it declares **no dependencies at all**, where the
Python package needed `python312`.

Two things that cost time and are worth not rediscovering:

- **`~/.cargo` on that machine is root-owned** (pkgsrc built rust as root), so a
  normal user's build dies while downloading its first crate with a bare
  "Permission denied". `build-netbsd.sh` sets `CARGO_HOME` inside its own work
  directory instead of fixing the machine.
- **`pkg_create`, `pkg_add` and `pkg_info` are not on a login PATH** — they live
  in `/usr/pkg/sbin` and `/usr/sbin`. The failure is `command not found` from
  inside a script that looks like it should work.

The feed it produces is **unsigned**, and the landing page says so: pkgin has no
signature verification to offer, and `pkg_add`'s GPG support wants a
signed-package format this does not use. Every other feed on aspic verifies a
signature; this one has HTTPS and nothing more.

## FreeBSD — done, signed repository, verified on the machine

```bash
./packaging/bsd/build-freebsd.sh   [user@host]   # compile, package, sign, fetch back
./packaging/bsd/verify-freebsd.sh  [user@host]   # refuse a bad key, install, run, stop
```

A real RSA-signed `pkg` repository, not a loose package: `pkg repo` builds the
catalogue and signs it, so `pkg install breeze-core` verifies before it installs
and a tampered catalogue fails. Like NetBSD it declares **no dependencies** —
the Python package needed `python312`.

**This is the one repository on aspic that is not assembled on the workstation.**
`pkg repo` signs locally, so the RSA key is copied to the build machine, used,
and destroyed with `rm -P`; `build-freebsd.sh` does that and then checks the file
is gone. Everything else on aspic is signed on the Dell and pushed as static
files.

Three things worth not rediscovering:

- **`daemon -p` is the child's pid, `-P` is the supervisor's**, and `rcvar`
  matching wants whichever one `procname` names. Point them at different
  processes and `service breeze_core status` reports "not running" while the
  service is happily serving.
- **`${name}_user` is magic in `rc.subr`**: it wraps the command in
  `su -m <user>`, so privileges are dropped *before* `daemon(8)` runs, and
  `daemon -u` then calls `setusercontext()` as a non-root user and fails with
  "failed to set user environment". The rc script uses `breeze_core_runas`, a
  name `rc.subr` does not claim.
- **Verify identity, not liveness.** An orphaned Python service was still
  holding 8420 and answering `/api/health`, which made a *failed* start look
  like a success. `verify-freebsd.sh` checks that the process on that port is
  `breeze` running `/usr/local/bin/breeze-core serve`. The migration script
  avoids the same trap by stopping the old service before it upgrades.

## OpenBSD — done, signify-signed package, verified on the machine

```bash
./packaging/bsd/build-openbsd.sh   [user@host]   # compile, package, sign, fetch back
./packaging/bsd/verify-openbsd.sh  [user@host]   # refuse an untrusted key, install, run, stop
```

**The first OpenBSD *package* this project has ever had.** The Python line could
only manage a source install here, because a virtualenv full of absolute paths is
not something you can hand to `pkg_add`. One static binary is.

The packing list does the work a script would otherwise do badly: `@newgroup` /
`@newuser` declare the service account so `pkg_delete` removes it again, and the
configuration is `@sample`, so `pkg_add` seeds it once and never touches an
edited copy — which is what you want for a file holding an API key and a unit's
V3 credentials.

- **The key's filename is part of the format.** `pkg_sign` records the key's name
  in the signature and `pkg_add` then opens `/etc/signify/<name>.pub`, so
  renaming the key renames what every user must install. It is `aspic-pkg`.
- **`pkg_info -e` wants a pkgspec**, not a stem: `breeze-core` gets you
  "Invalid spec", `breeze-core-*` works. NetBSD's takes the bare name, which is
  why `migrate.sh` has two lines for one command.
- **`-D FULLPKGPATH=`** silences "Use of uninitialized value in hash element at
  PkgAdd.pm line 304" on every install — a hand-built package has no pkgpath and
  the security-quirks check keys a hash on it. Setting it as a `@comment` in the
  list is rejected as "can't be set explicitly".
- **Builds go in `/home`.** The default `/` is under a gigabyte and was 99% full;
  a rust build does not fit. `ulimit -d` also needs raising, because the default
  login class caps the data segment below what `rustc` wants when linking.

## Published layout

FreeBSD is a repository, so a client points at one URL and `pkg update` finds
what is there. OpenBSD has **no index file at all** — `pkg_add` fetches the
directory and scrapes `<A HREF="....tgz">` out of the HTML — so `autoindex on`
in `site/aspic.conf` is not a convenience for humans browsing the tree, it *is*
the OpenBSD index. Turn it off and every OpenBSD install reports the package as
nonexistent.

```
/freebsd/                          meta.conf, packagesite.pkg, the package, aspic-freebsd.pub
/netbsd/All/                       the package and pkg_summary (unsigned)
/openbsd/<release>/packages/<arch>/  the signed package, plus aspic-pkg.pub one level up
/opnsense/                         os-breeze-core-<ver>.pkg and its sha256
```

The OpenBSD path carries the release because OpenBSD moves its libc every six
months and a package is valid for the release it was built on. `build-openbsd.sh`
reads that release off the build machine rather than hardcoding it.

## OPNsense

See `packaging/opnsense/`. It is FreeBSD **14**, not 15, so its binary is built
in a FreeBSD 14 chroot on the FreeBSD builder — binaries run forward, not
backward. `verify-plugin.sh` checks everything that can be checked from here and
says plainly what cannot: the GUI page itself has never run on a real firewall.
