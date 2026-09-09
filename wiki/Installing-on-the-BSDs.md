# Installing on the BSDs

Native packages for **FreeBSD**, **NetBSD** and **OpenBSD**, amd64. Each is
built and verified on a real machine of that system — nothing here is
cross-compiled, because Zig bundles no BSD libc and there is no honest way to
fake one.

None of the three declares a single dependency.

## FreeBSD

A proper RSA-signed `pkg` repository, built and verified on FreeBSD 15.1.

```sh
sudo mkdir -p /usr/local/etc/pkg/keys /usr/local/etc/pkg/repos
sudo fetch -o /usr/local/etc/pkg/keys/aspic.pub \
  https://aspic.salataputarica.hr.eu.org/freebsd/aspic-freebsd.pub

sudo tee /usr/local/etc/pkg/repos/aspic.conf <<'EOF'
aspic: {
  url: "https://aspic.salataputarica.hr.eu.org/freebsd",
  signature_type: "pubkey",
  pubkey: "/usr/local/etc/pkg/keys/aspic.pub",
  enabled: yes
}
EOF

sudo pkg update && sudo pkg install breeze-core
sudo sysrc breeze_core_enable=YES
sudo service breeze_core start
```

Configuration lives in `/usr/local/etc/breeze-core/` — the FreeBSD prefix, not
`/etc`. Set the bind address in `breeze-core.env` there, then
`sudo breeze-core pair`.

> **`pkg update` exits 0 even when it rejects the signature.** It prints
> "Invalid signature, removing repository" and then cheerfully says the
> repository is up to date — about the repository it just dropped. So do not
> trust the exit status: read the output, and check `pkg search breeze-core`
> actually finds something.

The service is `breeze_core` with an **underscore**, because that is what
FreeBSD's rc system requires of a variable name. The rc script is at
`/usr/local/etc/rc.d/breeze_core`.

For real isolation on FreeBSD, put it in a thin **jail** — there is no systemd
sandbox to lean on. See [Exposing it safely](Exposing-it-safely).

## NetBSD

A `pkgin` feed, built on a real NetBSD 11.0 machine.

```sh
echo "https://aspic.salataputarica.hr.eu.org/netbsd/All" \
  > /usr/pkg/etc/pkgin/repositories.conf
pkgin -y update && pkgin -y install breeze-core
```

Or without pkgin:

```sh
pkg_add https://aspic.salataputarica.hr.eu.org/netbsd/All/breeze-core-4.0.2.tgz
```

> **This is the one feed here that is not signed.** pkgin has no signature
> verification to offer, and `pkg_add`'s GPG support wants a signed-package
> format this does not use. So what you get is HTTPS and nothing more.
>
> Every other repository on this project verifies a signature made off the web
> host, so that a compromised host can serve stale or missing files but cannot
> forge a package. This one cannot make that claim, and it would be dishonest
> to imply otherwise. If that matters to you, take the tarball from the GitHub
> release instead and check its checksum.

Enable it with `breeze_core=YES` in `/etc/rc.conf`, then
`service breeze_core start`.

## OpenBSD

A **signify**-signed package, built and verified on OpenBSD 7.9.

```sh
# the key's filename is not cosmetic -- see below
doas ftp -o /etc/signify/aspic-pkg.pub \
  https://aspic.salataputarica.hr.eu.org/openbsd/aspic-pkg.pub

doas pkg_add https://aspic.salataputarica.hr.eu.org/openbsd/7.9/packages/amd64/breeze-core-4.0.2.tgz
doas rcctl enable breeze_core && doas rcctl start breeze_core
```

Optionally, so `pkg_add -u` finds this repository as well as the system one:

```sh
export PKG_PATH="https://aspic.salataputarica.hr.eu.org/openbsd/%c/packages/%a/:$(cat /etc/installurl)/%c/packages/%a/"
```

> **The key's filename is load-bearing.** `pkg_add` reads the signing key's
> *name* out of the package's own signature and then opens
> `/etc/signify/<that name>.pub`. Save it as anything else and every install
> fails with `signify: can't open /etc/signify/aspic-pkg.pub` — which is also
> exactly what it says when you have not fetched the key at all, so the two
> mistakes are indistinguishable from the message.

OpenBSD has **no `service(8)`** — use `rcctl`. The rc script is
`/etc/rc.d/breeze_core`.

## Things worth knowing on all three

**Upgrades keep your configuration.** All three packages leave the config
directory alone on deinstall, so an upgrade over an older version keeps your
`config.json` and your enrolled clients.

**`pkg_info -e` wants different things.** On OpenBSD it needs a pkgspec —
`breeze-core-*`, with the glob — and on NetBSD a bare `breeze-core`. Worth
knowing if you are scripting around it.

**The package tools are not on a non-login PATH.** `ssh host 'pkg_add …'`
often fails with "command not found" where the same command works after
logging in, because a non-interactive shell gets a shorter PATH. Use absolute
paths in scripts.

**Only amd64 is published.** The binaries are built on real machines, and
these are the machines that exist. If you want a BSD on ARM, the source build
works — see [Installing from source](Installing-from-source).

## The migration script refuses on OpenBSD, deliberately

If you are coming from an older release, `migrate.sh` handles FreeBSD and
NetBSD but **stops on OpenBSD** and prints the three commands to do it
yourself.

That is not an oversight. Earlier releases only ever shipped a source install
on OpenBSD — a virtualenv full of absolute paths is not something you can hand
to `pkg_add` — so `pkg_add` owns none of the old tree and cannot replace it.
The script will not guess at what it is allowed to delete. Remove the old tree
by hand, keeping your configuration, then run it again and it installs the
package.

See [Upgrading to 4.x](Upgrading-to-4).

## Verifying what you installed

```sh
breeze-core --version
breeze-core diag
```

And check the service really came up — `rcctl check breeze_core` on OpenBSD,
`service breeze_core status` on FreeBSD and NetBSD.

If something is wrong, [Troubleshooting](Troubleshooting) applies unchanged;
the only BSD-specific entries are the ones above.
