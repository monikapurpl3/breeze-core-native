# packaging/bsd — the BSDs

There is no cross-build here, and that is the whole story: Zig bundles a libc for
every Linux target this project ships, and none for FreeBSD, NetBSD or OpenBSD.
So a BSD binary is compiled on a BSD machine, and each one that is not available
is listed as not built rather than quietly skipped.

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

## FreeBSD and OpenBSD — not yet

Both need the same treatment: a machine, a native build, `pkg create` on FreeBSD
and `pkg_create` on OpenBSD. The Python line shipped a signed FreeBSD `pkg`
repository and an OPNsense plugin, so there is a shape to follow — and the
native side should be considerably simpler than the Python one, which had to
vendor an interpreter into the plugin.

They are absent because those virtual machines were not running, not because
anything about them is hard.
