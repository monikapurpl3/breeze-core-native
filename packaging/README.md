# packaging/ — how a release is built

Four steps, each with its own verification, and every one of them runs on the
maintainer's workstation. Nothing is signed on the server that serves it.

```bash
./packaging/build-binaries.sh          # one static binary per architecture
./packaging/nfpm/build-packages.sh     # .deb .rpm .pkg.tar.zst .apk .ipk
./packaging/nfpm/verify-packages.sh    # install each one in a clean container
./packaging/repo/build-repo.sh         # the signed aspic tree
./packaging/repo/verify-repo.sh        # install FROM it, with checking on
./site/publish.sh --tree packaging/out/aspic
```

## What comes out

| | |
|---|---|
| `packaging/out/bin/<arch>/breeze-core` | the executable, always under that name |
| `packaging/out/dist/*.tar.zst` | one archive per architecture, for people who do not want a package |
| `packaging/out/pkg/` | 30 packages: 6 architectures × deb/rpm/arch/apk, plus ipk per OpenWrt target |
| `packaging/out/aspic/` | the whole published tree — pages, keys, five signed repositories |

## Decisions worth knowing before changing anything here

**One binary per architecture, not one per libc.** The Linux builds are static
musl, so there is no dynamic loader and no `libc.so` to satisfy: the same file
runs on Debian, Fedora, Alma, Arch, Alpine and OpenWrt, and no package declares
a libc dependency. The inverse is the trap — *glibc*-static breaks
`getaddrinfo` — so s390x, which has no musl std upstream, stays dynamic against
a glibc 2.17 floor that Zig pins.

**The executable is `/usr/bin/breeze-core`.** The Python package put it in
`/usr/lib` because it was a directory of interpreter and dependencies, and paid
for that with a `semanage` call in its postinstall: `/usr/lib` is `lib_t` on
SELinux, which is not a domain-transition entrypoint, so the service ran as
`init_t` and its writes to `/etc/breeze-core` were denied silently. One file in
`/usr/bin` is `bin_t` and needs none of that. The old path is kept as a symlink
because the reference's unit file, its OpenRC script and a year of people's own
scripts name it.

**zstd where the client can read it.** dpkg has understood zstd since 1.21.18
(Debian 12) and rpm since 4.14 (RHEL 8); `.pkg.tar.zst` is what Arch has always
used. apk and ipk stay gzip — apk-tools 2.x has no zstd at all and opkg's
support depends on the OpenWrt build, so choosing it there would produce a
package the target cannot open. `build-packages.sh` asks each finished package
what it actually used rather than trusting the config, because nfpm accepts
unknown keys in silence.

**One repository per package-manager family, at the root of the host.** Not one
per project: somebody who has added aspic gets everything published there, and a
second project needs no second sources entry. Hence one signing key for the host
rather than a key per project — trust is a thing you ask a person for once.
Projects are told apart by package name, and each has its own page.

**Three key types, because three clients disagree.** GPG ed25519 for
apt/dnf/zypper/pacman, RSA-4096 for apk (apk-tools requires RSA), and usign for
opkg (OpenWrt's own ed25519 signer, which cannot read GPG). They live in
`packaging/repo/keys/`, git-ignored, generated on first run.

> **Back that directory up.** Losing it means every user has to re-trust a new
> key before they can update, which is the one failure here that cannot be fixed
> from this repository.

**Verification is two-directional.** `verify-repo.sh` serves the tree from a
container and, for each client, first proves the client *refuses* the repository
with no key and then proves it accepts it with one. A repository that installs
is not necessarily a repository that verifies: every one of these clients will
install from an unsigned source if asked the wrong way, and only the pair of
results means anything.

## Windows

`build-binaries.sh` produces the `.exe` natively — no Zig, because the MSVC
linker is what makes a binary Windows runs without a shipped libc. The installer
around it is a separate step, since `makensis` needs Windows and cannot be
cross-built:

```powershell
powershell -ExecutionPolicy Bypass -File .\packaging\windows\fetch-vendor.ps1
.\packaging\windows\build-installer.ps1
```

That produces a 1.2 MB `Breeze-Core-Setup.exe` which registers a hardened NSSM
service and offers a guided Caddy reverse proxy — with a fail2ban-style
tripwire — as an optional component. It needs no internet at all: the whole
server is one file, so unlike the Python installer there is no interpreter to
find and no dependencies to download. See `packaging/windows/`.

## The BSDs

`build-binaries.sh` does not build them: Zig bundles no FreeBSD, NetBSD or
OpenBSD libc, so those need a real machine. NetBSD is done and verified on one;
FreeBSD and OpenBSD are not. They are listed as not built rather than quietly
omitted. See `packaging/bsd/`.
