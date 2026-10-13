# packaging/bsd — the BSDs

Everything here runs on the workstation. Since October 2026 the three BSD binaries are
cross-compiled by `packaging/build-binaries.sh` (see `packaging/cross/README.md`)
and packaged and signed by `build-bsd.sh`, using each BSD's own packaging tools
inside the `bc-bsdpkg` container. The VMs only run the verify scripts.

```bash
./packaging/build-binaries.sh freebsd-amd64 netbsd-amd64 openbsd-amd64
./packaging/bsd/build-bsd.sh                      # package + sign all three
./packaging/bsd/verify-netbsd.sh  [user@host]     # install, upgrade, run, stop
./packaging/bsd/verify-openbsd.sh [user@host]     # refuse an untrusted key, install, run, stop
./packaging/bsd/verify-freebsd.sh [user@host] [base-url]   # refuse a bad key, install from the repo
```

Until 4.3.2 each BSD was compiled, packaged and signed on a VM of that system,
and two signing keys travelled to those VMs to be used and shredded with
`rm -P`. Now the keys never leave this machine: `build-bsd.sh` streams them
into a throwaway container per run and nothing writes them anywhere else.

## What builds what

| | binary (build-binaries.sh) | package (build-bsd.sh, in bc-bsdpkg) |
|---|---|---|
| FreeBSD | zig, FreeBSD 14's libc | FreeBSD's `pkg create`, signed catalogue from `pkg repo` (RSA) |
| NetBSD | zig, NetBSD 10.1's libc | pkgsrc's `pkg_create`; `pkg_info -X` writes `pkg_summary` |
| OpenBSD | clang + lld against the release's own libraries | `mkpkg-openbsd.py`, signed with `signify -zS` |
| OPNsense | the FreeBSD binary | `packaging/opnsense/build-plugin.sh`, same image |

`bc-bsdpkg` (the `Dockerfile` here) is Ubuntu 24.04 with FreeBSD's pkg 2.7.5
built from source the way pkg's own CI builds it, a pkgsrc bootstrap for
pkg_install, and Debian's signify. OpenBSD's `pkg_create` is Perl that only runs
on OpenBSD, so `mkpkg-openbsd.py` writes the package itself; it was checked
against the package `pkg_create` made for 4.3.2 and differs only where 4.3.2 was
wrong (that LICENSE had CRLF line endings, from `git archive` with autocrlf on
the way to the VM).

## FreeBSD — a signed repository

`pkg repo` builds the catalogue and signs it, so `pkg install breeze-core`
verifies before it installs and a tampered catalogue fails. The package declares
**no dependencies**; the Python one needed `python312`.

**The ABI is a pattern, `FreeBSD:1[45]:amd64`.** pkg fnmatches a package's ABI
against the host's, and the binary links against FreeBSD 14's libc, which 15
carries forward. 4.3.2's package said `FreeBSD:15:amd64` and so refused 14.
Widen the pattern when 16 ships, after running the binary there.

Worth not rediscovering (from the rc script and the verify):

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
  `breeze` running `/usr/local/bin/breeze-core serve`.
- `pkg` built for Linux still records files as `root:wheel`, as on FreeBSD.

`verify-freebsd.sh` installs from a repository URL. Before publishing, serve
`packaging/out/bsd` locally and tunnel it to the VM:

```bash
(cd packaging/out/bsd && python -m http.server 8099 --bind 127.0.0.1) &
ssh -N -R 8099:127.0.0.1:8099 monika@192.168.122.131 &
./packaging/bsd/verify-freebsd.sh monika@192.168.122.131 http://127.0.0.1:8099
# then kill both by their PIDs
```

## NetBSD — a pkgin feed, unsigned

The feed is **unsigned**, and the landing page says so: pkgin has no signature
verification to offer, and `pkg_add`'s GPG support wants a signed-package format
this does not use. Every other feed on aspic verifies a signature; this one has
HTTPS and nothing more.

- **`+BUILD_INFO` names the platform the binary is for, not the one it was
  packaged on:** `OPSYS=NetBSD`, `OS_VERSION=10.1` (zig's libc), `MACHINE_ARCH=x86_64`.
  pkg_add refuses another OPSYS or arch outright, but only *warns* when
  OS_VERSION differs, so an 11.x machine prints "was built for a different
  platform" and installs. That warning is true.
- **`PKGTOOLS_VERSION` is deliberately absent**: a value that does not match the
  host's exact pkg_install release makes pkg_add reject the package.
- **`pkg_create`, `pkg_add` and `pkg_info` are not on a NetBSD login PATH** —
  they live in `/usr/pkg/sbin` and `/usr/sbin`. The verify sets PATH for that.

## OpenBSD — a signify-signed package

**The first OpenBSD *package* this project ever had** arrived with the native
line: a virtualenv full of absolute paths is not something you can hand to
`pkg_add`. One binary is.

The packing list does the work a script would otherwise do badly: `@newgroup` /
`@newuser` declare the service account so `pkg_delete` removes it again, and the
configuration is `@sample`, so `pkg_add` seeds it once and never touches an
edited copy — which is what you want for a file holding an API key and a unit's
V3 credentials.

- **The key's filename is part of the format.** The signature records the key's
  name and `pkg_add` then opens `/etc/signify/<name>.pub`, so the secret key is
  signed with as `aspic-pkg.sec`, and renaming it renames what every user must
  install.
- **`pkg_info -e` wants a pkgspec**, not a stem: `breeze-core` gets you
  "Invalid spec", `breeze-core-*` works. NetBSD's takes the bare name, which is
  why `migrate.sh` has two lines for one command.
- **`@comment pkgpath=`** (FULLPKGPATH) silences "Use of uninitialized value in
  hash element at PkgAdd.pm line 304" on every install — a hand-built package
  has no pkgpath and the security-quirks check keys a hash on it.
- **The binary is valid for one release.** OpenBSD moves its libc every six
  months; this one links against 7.9's `libc.so.103.0`. `OPENBSD_REL` in
  `build-binaries.sh` is the one place that says which, and a new release needs
  its sets' checksums added to `packaging/cross/openbsd-sysroot.sh`.

## Published layout

FreeBSD is a repository, so a client points at one URL and `pkg update` finds
what is there. OpenBSD has **no index file at all** — `pkg_add` fetches the
directory and scrapes `<A HREF="....tgz">` out of the HTML — so `autoindex on`
in `site/aspic.conf` is not a convenience for humans browsing the tree, it *is*
the OpenBSD index. Turn it off and every OpenBSD install reports the package as
nonexistent.

```
/freebsd/                          meta.conf, packagesite.pkg, data.pkg, the package, aspic-freebsd.pub
/netbsd/All/                       the package and pkg_summary (unsigned)
/openbsd/<release>/packages/<arch>/  the signed package, plus aspic-pkg.pub one level up
/opnsense/                         os-breeze-core-<ver>.pkg and its sha256
```

## OPNsense

See `packaging/opnsense/`. OPNsense is FreeBSD 14 (26.1) or 15 (26.7); the
FreeBSD binary, linked against 14's libc, serves both, and `verify-plugin.sh`
runs it in the FreeBSD VM's 14 root and on its 15 userland to prove it. The GUI
page itself has still never been exercised on a real firewall by these scripts.
