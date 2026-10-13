# Ports and architectures

Which machines Breeze Core runs on, how the binaries are produced, and what is
honestly absent.

**One small static executable, built for 22 targets: thirteen Linux
architectures, from a Raspberry Pi Zero and a 32-bit thin client to a Loongson
desktop, an IBM mainframe and a big-endian Power Mac; Android; Windows; macOS on
both kinds of Mac; FreeBSD, NetBSD and OpenBSD; and the routers and firewalls
in between, OpenWrt and OPNsense.** All of it is built on one workstation, from
one source tree, with no emulator in the build.

There is no proof-of-concept tier. Every architecture below either has a signed
package published on every release, or is listed here as absent with the reason.

## What ships

| Architecture | deb | rpm | pacman | apk | xbps | ipk/OpenWrt | ebuild | tarball |
|---|---|---|---|---|---|---|---|---|
| x86-64 | ✅ `amd64` | ✅ `x86_64` | ✅ `x86_64` | ✅ `x86_64` | ✅ ×2 | ✅ `x86_64` | ✅ `~amd64` | ✅ |
| x86 (32-bit) § | ✅ `i386` | — | — | ✅ `x86` | ✅ `i686` | ✅ `i386_pentium4` | ✅ `~x86` | ✅ |
| ARM64 | ✅ `arm64` | ✅ `aarch64` | ✅ `aarch64` | ✅ `aarch64` | ✅ ×2 | ✅ 3 targets | ✅ `~arm64` | ✅ |
| ARMv7 | ✅ `armhf` ¶ | ✅ `armv7hl` | ✅ `armv7h` | ✅ `armv7` | ✅ ×2 | ✅ `arm_cortex-a7…` | ✅ `~arm` ¶ | ✅ |
| ARMv6 (Pi Zero, Pi 1) | ✅ `armhf` ¶ | — | — | ✅ `armhf` | ✅ ×2 | ✅ `arm_arm1176jzf-s_vfp` | ✅ `~arm` ¶ | ✅ |
| riscv64 | ✅ | ✅ | ⬇ | ✅ | ✅ ×2 | ✅ `riscv64_riscv64` | ✅ `~riscv` | ✅ |
| ppc64le | ✅ `ppc64el` | ✅ | ⬇ | ✅ | ✅ ×2 | — | ✅ `~ppc64` † | ✅ |
| ppc64 (big-endian) | — | — | — | ✅ `ppc64` (Adélie) | — | — | ✅ `~ppc64` † | ✅ |
| s390x | ✅ | ✅ | ⬇ | ✅ | — | — | ✅ `~s390` | ✅ |
| LoongArch64 | ✅ `loong64` | ✅ `loongarch64` | — | — | — | ✅ `loongarch64_generic` | ✅ `~loong` | ✅ |
| MIPS (little-endian) | — | — | — | — | — | ✅ `mipsel_24kc` ‡ | — | ✅ |
| MIPS (big-endian) | — | — | — | — | — | ✅ `mips_24kc` ‡ | — | ✅ |
| MIPS64 (Octeon) | — | — | — | — | — | ✅ `mips64_octeonplus` ‡ | — | ✅ |

✅ in a signed repository · ⬇ built and attached to the release, but not in a
repository · † keyworded with a guard, see below · ‡ OpenWrt only, in both
feeds (see [MIPS](#mips)) · § needs SSE2: Pentium 4, Athlon 64 or later ·
¶ the same armv6 binary, see below

Plus a native package each for **FreeBSD**, **NetBSD** and **OpenBSD**, an
**OPNsense** plugin, a **Windows** installer, **Termux** packages for Android
(aarch64, arm, x86_64), and **macOS** tarballs for Apple silicon and Intel,
below.

**One armhf, two ARMs.** Debian and Raspberry Pi OS call both ARMv6 and ARMv7
hard-float `armhf`, so there can be only one armhf `.deb`, and it holds the
**armv6** binary: ARMv7 runs ARMv6 code, so the one package serves every Pi from
the Zero up and every ARMv7 board. Gentoo's `arm` keyword uses the same binary
for the same reason. Alpine and Void name the two separately, so they get both.

**A format is offered only where it can be proven.** The 32-bit x86 and
LoongArch packages exist for the distributions that have an image to install
into and check: Debian, Alpine, Void and OpenWrt for x86; Debian (`loong64`)
and openEuler (rpm) for LoongArch; and Adélie Linux, which uses apk, for
big-endian ppc64. Arch Linux 32, Alpine's LoongArch port and Debian's ppc64 port
have none, so they get no package — the tarball still runs there.

**`×2` is glibc and musl.** Void treats the two libcs as separate
architectures with separate package indexes, so each gets its own package — the
same bytes in both, because a static binary does not care which libc the host
has, but the label must match or the package is invisible to half of Void.

**pacman stops at three** because Arch has no official riscv64, ppc64le or
s390x port for a repository to serve. Those packages are still built and
attached to each release, so `pacman -U <url>` works if you run one of the
unofficial ports.

**The `~arm` and `~ppc64` keywords are guarded.** Gentoo's `arm` keyword spans
armv4 through armv7 and both float ABIs, and the binary is ARMv6 hard-float; the
ebuild checks `CHOST` and refuses soft-float and pre-ARMv6 with an explanation
rather than installing something that cannot run. `ppc64` spans both
endiannesses, and now both binaries exist: the ebuild fetches both and unpacks
the one `CHOST` names.

**The ipk column is OpenWrt 24.10 and older.** OpenWrt 25.12 uses apk-tools 3
instead, and every architecture in that column is also in the apk v3 feed;
see [OpenWrt: two feeds](#openwrt-two-feeds-and-the-account-that-was-never-made).

**OpenWrt needs an exact match.** Its package architecture is not a family
name, so the three ARM64 targets — `aarch64_generic`, `aarch64_cortex-a53` and
`aarch64_cortex-a72` — are separate packages holding the same binary. Check
yours with `. /etc/openwrt_release && echo "$DISTRIB_ARCH"`.

## Cross-built, and why not emulated

Every Linux binary is cross-compiled on one machine:

```sh
./packaging/build-binaries.sh          # every target, one static executable each
./packaging/build-binaries.sh amd64    # or just one
```

`cargo-zigbuild` produces a finished executable for each target in about twenty
seconds, and **no emulator is involved at any point.** That is a deliberate
choice rather than a convenience: the alternative was tried, and building these
same architectures under QEMU went like this.

| Target | What happened |
|---|---|
| s390x | worked in ~45 minutes, but only after **gcc segfaulted in `cc1`** |
| ppc64le | **gcc segfaulted in `collect2`**, then again from the link step |
| arm64 (Termux) | **deadlocked twice** — qemu-user's futex handling wedges under a parallel build |
| MIPS | not attempted; Rust has no prebuilt `std` for any MIPS target |

Three compiler failures and one emulator failure, none of them faults in the
code being built. They cannot happen here, because they were properties of the
emulator rather than of the targets.

## Static musl, and the one exception

Linux targets are **static musl**, linked by Zig. There is no libc at runtime,
so one binary per architecture serves every distribution — the same file
installs on Debian 12, Alpine and RHEL 8, which is why there is no per-distro
build matrix anywhere in this project.

**s390x is the exception, and it is built against glibc**, with Zig pinning the
floor at 2.17 (`s390x-unknown-linux-gnu.2.17`). Two reasons: there is no
prebuilt musl `std` for it upstream, and there is no musl distribution on the
platform to want one.

Worth knowing why *static glibc* is not the answer instead. Statically linking
glibc breaks `getaddrinfo`, because NSS loads its backends as shared objects at
runtime — a statically linked glibc binary resolves no hostnames at all. It
fails at the point where it tries to reach something by name, which is a long
way from where you would look for the cause.

## The BSDs, cross-built

Since 4.4.0 the BSD binaries are cross-compiled on the same workstation as
everything else, each against the C library of the release it is for, and
packaged and signed there with each BSD's own tools. Until 4.3.2 each was
compiled, packaged and signed on a virtual machine of its own system.

| | linked against | packaged with |
|---|---|---|
| FreeBSD | FreeBSD 14's libc, from zig — so it runs on 14 and 15, and on OPNsense 26.1 and 26.7 | FreeBSD's `pkg`, signed catalogue |
| NetBSD | NetBSD 10.1's libc, from zig | pkgsrc's `pkg_create` |
| OpenBSD | 7.9's own libraries, downloaded from OpenBSD and checksum-pinned | a re-implementation of `pkg_create`, signed with `signify` |

OpenBSD gets its real libraries rather than zig's because zig's OpenBSD libc is
generated stubs, and the result asks for a bare `libc.so` instead of
`libc.so.103.0`. Built against 7.9's own, it has the same dependencies as a
binary compiled on OpenBSD, and the IBT landing pads OpenBSD enforces on amd64.
The tools that package them — FreeBSD's `pkg`, pkgsrc's `pkg_install` and
`signify` — run in a Linux container, so the signing keys never leave the
workstation. Every package is still installed and run on a real machine of its
system before a release; see [the honest cost](#the-honest-cost).

## MIPS

**Shipped: mipsel_24kc and mips_24kc**, the two architectures most OpenWrt
routers are. mipsel_24kc is MediaTek (MT7621, MT76x8); mips_24kc is Qualcomm
Atheros (ath79). They are in both OpenWrt feeds: opkg for 24.10 and older, and
apk for 25.12 and later. No other packager gets them, because no distribution
that uses deb, rpm or pacman still ships 32-bit MIPS. There are also plain
tarballs.

Every MIPS target is **tier 3** in Rust, with no prebuilt `std`.
`packaging/mips/build-mips.sh` therefore builds `std` from source
(`-Z build-std`), in an image with a pinned nightly and OpenWrt 25.12.5's own
toolchains, checked against OpenWrt's SHA-256s. It links with the OpenWrt
toolchain, not Zig: Zig 0.16's bundled mipsel musl emits references to its own
std internals and then fails to provide them.

They are **static, like every other Linux build**, which took three things a
tier 3 target does not get from a prebuilt std:

- `+crt-static`, because Rust's mips*-musl targets default to linking
  dynamically against the router's own `/lib/ld-musl-*-sf.so.1`;
- `-C link-self-contained=no`, so `crt1.o`, `crti.o` and `libc.a` come from
  the toolchain's sysroot;
- a `libunwind.a` that is the toolchain's `libgcc_eh.a`. gcc's static unwinder
  has the same `_Unwind_*` API these targets use as `libgcc_s` when linked
  dynamically. `-Z build-std-features=llvm-libunwind` does **not** build one:
  the unwind crate has no build script, and LLVM's libunwind is built by
  rustc's own bootstrap.

One code change, too. 32-bit MIPS has no 64-bit atomics, and the SSE
subscriber counter was an `AtomicU64`. It is a mutex now; a client subscribes
once per connection.

**Verified in OpenWrt's own root filesystems under QEMU**, both endiannesses,
both feeds (`packaging/repo/verify-repo.sh openwrt-mipsel openwrt-mips
openwrt-mipsel-opkg`). Each is refused unsigned, then installed, gets its
service account and serves. Docker Hub has big-endian `mips_24kc` OpenWrt
images but no little-endian MIPS at all, so `packaging/mips/prepare-qemu.sh`
imports OpenWrt's malta-le root filesystem, whose package architecture is
mipsel_24kc. It also registers `qemu-mips` and `qemu-mipsel` with binfmt_misc.
**That registration does not survive Docker Desktop's VM restarting**, so run
the script again after a restart.

The traps still worth knowing:

- **Big- and little-endian 32-bit MIPS report the same `uname -m`** (`mips`).
  Packages and binaries are named by OpenWrt architecture, never by uname.
- Soft-float triples are pedantic about their suffixes, and getting one wrong
  produces a package a router accepts and then cannot run.
- **`mips64_octeonplus` (Cavium Octeon: EdgeRouter, and OpenWrt's octeon
  target) ships from 4.4.0, built with OpenWrt's own octeon toolchain — and it
  is the one binary here that nothing on this side can run.** That toolchain
  uses Octeon-only instructions, which QEMU's generic MIPS64 lacks, while QEMU's
  Octeon model has no FPU. Neither has a real Octeon: there the kernel emulates
  floating point, as every hard-float program on an EdgeRouter relies on, and
  QEMU's user mode does not. It runs until its first floating-point
  instruction. A report from a real EdgeRouter is the check it is missing.

## OpenWrt: two feeds, and the account that was never made

**OpenWrt 25.12 replaced opkg with apk-tools 3.** Its packages are apk v3,
in the ADB format, which is neither the `.ipk` opkg reads nor the apk v2
Alpine uses, and nfpm cannot write it. So `packaging/openwrt-apk/build-apk.sh`
makes them with apk-tools 3 itself (`apk mkpkg`), from Alpine 3.24; OpenWrt's
own apk is built without `mkpkg` and `mkndx`. They hold the same binaries,
procd script and maintainer scripts as the `.ipk`.

The index is signed with a P-256 key, which is OpenWrt's own algorithm. The
packages are not signed, which is how OpenWrt's own are made: the index
records each package's hash, and the index is what is signed. `/openwrt` (opkg)
stays as it was, for 24.10 and older.

Building the first OpenWrt verification for either feed found a bug in every
OpenWrt package published until then. **OpenWrt has no `addgroup`, `adduser`,
`useradd` or `getent`.** The pre-install script exited 127, so the package
never had its `breeze` account, and procd, told to start the service as
`breeze`, could not. The scripts now check `/etc/passwd` where there is no
`getent`, and create the account with `/lib/functions.sh`, as OpenWrt's own
packages do (uid from 32768). Its helpers lock `/var/lock/passwd`, which only
a booted system has, so they create `/var/lock` first.

## Termux

**Shipped: aarch64, arm and x86_64**, as Termux packages in their own signed
apt repository at `/termux`. See [Installing on Termux](Installing-on-Termux).

These are **Android builds**, linked by the NDK, not the static musl binaries.
The musl build does run on a phone, because the kernel is Linux, but it looks
for its timezone in `/etc/localtime`, which Android doesn't have. So it believes
it is in UTC, and every schedule and timer fires hours out. Built for Android,
chrono reads the phone's zone and Android's own timezone database.

Android is a **tier 2** Rust target, so `std` is prebuilt and stable Rust is
enough. Zig can't link it, because it ships no bionic. The NDK's clang does,
both for rustc and for ring's C, at API 24, Termux's own floor.

Traps found getting there:

- **The config directory had to move.** No app may write to `/etc` on Android,
  and the server, `pair` and the admin commands each had their own copy of
  `/etc/breeze-core`. There is one default now, and on Android it is Termux's
  prefix.
- **The packages are xz, not zstd like every other deb here.** nfpm compresses
  `data.tar` as asked but always gzips `control.tar`. apt on 32-bit arm Termux
  rejected that member, and a zstd one, as `Corrupted archive`, while `dpkg -i`
  installed the same file. xz is what Termux's own packages use, so the build
  repacks with `dpkg-deb`.
- **nfpm renames architectures.** It turns Termux's `aarch64` and `x86_64` into
  Debian's `arm64` and `amd64`, which Termux's dpkg refuses as foreign. Debian's
  dpkg in turn warns that `x86_64` is not a valid architecture name; Termux's
  dpkg is patched to accept it.
- **32-bit bionic calls `personality()` first thing**, and Docker's default
  seccomp profile refuses the value it asks for. The arm verification
  container runs without seccomp. Only that architecture needs it.

## macOS

Two tarballs, **`macos-arm64`** (Apple silicon) and **`macos-x86_64`** (Intel),
about 2 and 2.4 MB, for **macOS 13 or newer**. They are cross-built on the same
Windows machine as everything above, with the same `cargo zigbuild`, from 4.3.1
on, and installed with [Homebrew](Installing-with-Homebrew).

zig carries macOS's C library but no Apple frameworks, and one dependency
needs one: the local time zone (`chrono`, through `iana-time-zone`) asks
CoreFoundation for it, using seven functions. `packaging/macos/` holds a link
stub naming just those, written here rather than copied from Apple's SDK. At
run time macOS loads its own CoreFoundation. The binaries need nothing a Mac
does not have: `libSystem`, `libiconv`, `libcharset` and CoreFoundation.

**They run on real Macs on every change** that could affect them:
`.github/workflows/macos.yml` cross-builds them on Linux, then runs them on
Apple silicon (macOS 26) and Intel (macOS 15). Each run unpacks with macOS's
own `tar`, serves, pairs the CLI, runs `diag --nerd`, and checks the server's
UTC offset against macOS's own in a zone that is not UTC. The time zone is the
one thing CoreFoundation is there for.

They are **not notarised**. Apple silicon runs nothing unsigned, so zig's
linker gives the arm64 binary an ad-hoc signature. That is enough to run it,
not to satisfy Gatekeeper. Homebrew and `curl` downloads run as they are. A
tarball downloaded with a browser is quarantined, and Gatekeeper refuses it
until the quarantine is cleared:

```sh
xattr -d com.apple.quarantine breeze-core
```

**The Homebrew tap on aspic is the way to install them**, and the same
formula installs the static Linux binary under Homebrew on Linux. The workflow
installs it on both Macs, and on x86-64 and ARM64 Linux, each time it changes:
see [Installing with Homebrew](Installing-with-Homebrew). Without Homebrew, the
launchd template is in `deploy/init/com.breeze.core.plist`.

## The honest cost

**Cross-built binaries are less obviously trustworthy than natively built
ones.** That is true, and it is not waved away here. What is checked, and what
is not:

- Every Linux package is installed and started **in a container of that
  distribution**. `verify-repo.sh` covers Debian 12, AlmaLinux 9, AlmaLinux 8,
  Arch, Alpine, Void and Gentoo, against the live repository URL, and each case
  also proves that an untrusted key is *refused*.
- **Every Linux binary is run, not only built**: `packaging/cross/smoke-linux.sh`
  starts each one under QEMU, fetches `/api/health` and the panel, and passes
  `diag` against it. Only Octeon cannot be (see [MIPS](#mips)).
- The three BSDs are **cross-built and verified on real machines** — FreeBSD
  15.1, NetBSD 11.0 and OpenBSD 7.9 — which install the package from the signed
  repository or with the signature checked, start the service and query it.
- The OPNsense plugin's binary is the FreeBSD one, linked against FreeBSD 14,
  and is run both in a FreeBSD 14 root and on FreeBSD 15, since OPNsense 26.1 is
  14 and 26.7 is 15, and FreeBSD binaries run forward rather than backward.
- The two macOS binaries are cross-built, and **run on real Macs** in CI on
  every change that could affect them; see [macOS](#macos).
- **Emulation is not hardware.** riscv64, ppc64 in both endiannesses, s390x,
  LoongArch and the 32-bit ARMs run under QEMU here, not on the machines
  themselves. QEMU is strict about instructions, so a binary that uses one the
  CPU lacks fails here too — but it is not a real POWER9, mainframe or Loongson.

If you run one of those, `breeze-core diag` is the check, and a report is
welcome. An EdgeRouter owner's report would close the last gap.
