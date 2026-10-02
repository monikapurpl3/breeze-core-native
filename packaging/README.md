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
| `packaging/out/termux/` | 3 Termux packages (aarch64, arm, x86_64), from `packaging/termux/` |
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
.\packaging\windows\build-installer.ps1
```

That produces a 1.1 MB `Breeze-Core-Setup.exe` with a Simple and an Advanced
path, which registers a hardened NSSM service, installs an update check, and
offers a guided Caddy reverse proxy — with a fail2ban-style tripwire — as an
optional component. The whole server is one file, so unlike the Python
installer there is no interpreter to find and no dependencies to download. It
does download NSSM and the updater during setup, checked against pinned
SHA-256s, because antivirus products flag installers that carry NSSM; an
upgrade reuses the installed copy, and an offline machine can have
`nssm-2.24.zip` put beside the installer. See `packaging/windows/`.

## Termux

`build-binaries.sh` does not build these either: Termux needs an Android
binary, not the static musl one (which would run, and believe it was in UTC,
because Android has no `/etc/localtime`). `packaging/termux/build-packages.sh`
links them with the Android NDK on stable Rust, packages them with nfpm, then
repacks them with `dpkg-deb` as xz, because nfpm always gzips `control.tar` and
32-bit arm Termux's apt rejected that. `verify-termux.sh` installs and runs them
in termux-docker. They go to their own apt repository, `/termux`, signed with
the same key as `/deb`.

## MIPS and OpenWrt

The two MIPS binaries come from `packaging/mips/build-mips.sh`, in an image
with a pinned nightly (every MIPS target is tier 3: `-Z build-std`) and OpenWrt
25.12.5's own toolchains. They are static: `+crt-static`,
`-C link-self-contained=no`, and a `libunwind.a` that is the toolchain's
`libgcc_eh.a`. nfpm wraps them as ipk only. OpenWrt 25.12 uses apk-tools 3, so
`packaging/openwrt-apk/build-apk.sh` also makes apk v3 packages for every
OpenWrt architecture with `apk mkpkg` (Alpine 3.24), and build-repo.sh signs
`/openwrt-apk` with a P-256 key. `packaging/mips/prepare-qemu.sh` makes MIPS
runnable for the verify-repo OpenWrt cases; run it again after Docker Desktop
restarts.

## Source packages

`packaging/source/build-source.sh` makes the SRPM, the Debian source package
and the Arch `makepkg --allsource` tarball from one `git archive` of the commit
and one `cargo vendor` tarball. Every recipe pastes in `packaging/nfpm/scripts/`,
so a rebuilt package is the same package as the binary one. It refuses a dirty
tree, because the tarball is the commit. `verify-source.sh` rebuilds each one
in clean containers and diffs its file list against the nfpm package's.
build-repo.sh signs them into `/rpm/SRPMS`, the apt `Sources` index and
`/arch/sources`.

## `breeze-core proxy`, tested for real

`packaging/test/verify-proxy.sh [nginx|caddy|apache]` installs each web server
in a clean Debian container. It runs the wizard with the answers given on
stdin, as a person would type them. Then it checks four things:

- the configuration serves the name through the proxy;
- Breeze Core ends up on 127.0.0.1 with `--behind-proxy`;
- a forged `X-Forwarded-For: 192.168.1.5` sent from a *second* container is
  logged with that container's real address;
- `--undo` leaves every file as it was.

Apache gets a stand-in `certbot` that writes and enables
`breeze-core-le-ssl.conf` the way the real one does, because that is the file
an undo used to leave behind. The test needs only the amd64 binary, not the
packages.

## The BSDs

`build-binaries.sh` does not build them: Zig bundles no FreeBSD, NetBSD or
OpenBSD libc, so those need a real machine. All three are built, packaged and
verified natively on one each, by `packaging/bsd/build-*.sh`, and published on
aspic. See `packaging/bsd/`.
