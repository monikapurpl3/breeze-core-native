# Building and releasing

What a release goes through before it reaches you. Useful if you are
contributing, packaging Breeze Core for somewhere new, or want to know what the
published artifacts were checked against.

For building it to *use*, see
[Installing from source](Installing-from-source).

## What is in this repository, and what is not

Everything that ends up **on your machine** is here, so you can read it before
you run it: the server's source, the service files (systemd, OpenRC, sysvinit,
the BSDs' rc.d, launchd, runit, s6, supervisor), every package's install and
removal scripts, the package recipes (nfpm, RPM spec, Debian, PKGBUILD,
ebuild, XBPS, Homebrew), the OPNsense plugin, the Windows installer and its
service scripts, the container images' Dockerfiles, and `migrate.sh`.

The **release machinery** — the scripts that build, sign, verify and publish
the packages, and the setup of the host that serves them — is kept privately
by the maintainer. It runs on one workstation, and the signing keys never
leave it; the web host only ever holds signed files.

`packaging/build-binaries.sh` stays here, because the
[macOS check](Ports-and-architectures#macos) runs it on GitHub's Macs.

## Versions and releases

Two numbers, and the second one matters more than it looks.

The **version** lives in exactly one place, `crates/breeze-core/Cargo.toml`.

The **package release** — the trailing `-N` in `breeze-core-4.0.2-2` — is for
when the packaged bytes change and the version does not. Without it, a rebuilt
4.0.2 is *identical* to a package manager, so no existing install ever upgrades
to the fix. The Windows installer, the BSD packages and the OPNsense plugin
have no such field: downloading again gets the new one.

New architectures with **no change to the code** are not a new version either:
they are added to the release whose code they are, and nothing already
published changes by a byte. 4.3.2 got five that way on 13 October 2026.

Releases are built from a **committed** tree. `breeze-core --version` names
the commit it was built from, and says `-dirty` if the tree was not clean.

## The signing keys

Every repository on aspic is signed, so a compromised web host could serve
something stale or missing but nothing your package manager would accept.
There are five keys because no two package managers agree:

| Key | For | Why its own |
|---|---|---|
| GPG RSA-4096 | apt (Debian and Termux), dnf, pacman | **RSA, not ed25519** — rpm 4.14 (RHEL/Alma/Rocky 8) cannot import an ed25519 public key at all |
| RSA-4096 | apk | apk-tools has no ed25519 option |
| usign ed25519 | opkg | OpenWrt's own signer |
| RSA-4096 | xbps | what `xbps-rindex` signs with |
| ECDSA P-256 | OpenWrt apk (25.12+) | what OpenWrt itself signs its apk indexes with |

FreeBSD's `pkg` catalogue and OpenBSD's packages (`signify`) are signed on the
same workstation, with their own keys. The public half of every key is linked
from [aspic](https://aspic.salataputarica.hr.eu.org).

## What every release is checked against

- `cargo test --workspace`, and clippy and rustfmt as a CI gate.
- **Every package installed from the built repository**, in clean containers
  of its distribution — Debian (amd64, i386, armhf, loong64), Termux,
  AlmaLinux 9 and 8, openEuler, Arch, Alpine, Adélie (big-endian ppc64), Void
  (glibc and musl), Gentoo, and OpenWrt 24.10 and 25.12 — the foreign
  architectures under QEMU.
- **Both directions, every time.** Every one of those clients will install
  from an unsigned source if asked the wrong way, so each check first proves
  the client *refuses* the repository without the key, then that it accepts
  it with the key. Only the pair means anything.
- The same checks against the **published** URL, after publishing: the only
  thing that exercises DNS, TLS and the web server together.
- Packages installed and **started** as a service; source packages rebuilt
  with each distribution's own Rust; the Gentoo ebuild through a real
  `emerge`; the BSD packages on real FreeBSD, NetBSD and OpenBSD machines;
  the OPNsense plugin on OPNsense; `breeze-core proxy` against real nginx,
  Caddy and Apache; the macOS binaries on GitHub's Apple-silicon and Intel
  Macs.

One lesson from all of it: **a test that enrols a credential must revoke
it.** An early harness did not, and thirty-two real, working credentials piled
up in a production `devices.json` before anyone noticed.

## Container images

Two, amd64 and arm64, made from **the same binary the packages ship** rather
than one compiled separately: a container that is a different build from the
`.deb` is a container that can fail differently.

**No `latest` tag**, deliberately: on a daemon that controls heating, `latest`
is an upgrade nobody asked for at a moment nobody chose.
