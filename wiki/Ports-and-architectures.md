# Ports and architectures

Which machines Breeze Core is packaged for, how the binaries are produced, and
what is honestly absent.

There is no proof-of-concept tier. Every architecture below either has a signed
package published on every release, or is listed here as absent with the reason.

## What ships

| Architecture | deb | rpm | pacman | apk | xbps | ipk | ebuild | tarball |
|---|---|---|---|---|---|---|---|---|
| x86-64 | ✅ `amd64` | ✅ `x86_64` | ✅ `x86_64` | ✅ `x86_64` | ✅ ×2 | ✅ `x86_64` | ✅ `~amd64` | ✅ |
| ARM64 | ✅ `arm64` | ✅ `aarch64` | ✅ `aarch64` | ✅ `aarch64` | ✅ ×2 | ✅ 3 targets | ✅ `~arm64` | ✅ |
| ARMv7 | ✅ `armhf` | ✅ `armv7hl` | ✅ `armv7h` | ✅ `armv7` | ✅ ×2 | ✅ `arm_cortex-a7…` | ✅ `~arm` † | ✅ |
| riscv64 | ✅ | ✅ | ⬇ | ✅ | ✅ ×2 | ✅ `riscv64_riscv64` | ✅ `~riscv` | ✅ |
| ppc64le | ✅ `ppc64el` | ✅ | ⬇ | ✅ | ✅ ×2 | — | ✅ `~ppc64` † | ✅ |
| s390x | ✅ | ✅ | ⬇ | ✅ | — | — | ✅ `~s390` | ✅ |

✅ in a signed repository · ⬇ built and attached to the release, but not in a
repository · † keyworded with a guard, see below

Plus a native package each for **FreeBSD**, **NetBSD** and **OpenBSD**, an
**OPNsense** plugin, and a **Windows** installer.

**`×2` is glibc and musl.** Void treats the two libcs as separate
architectures with separate package indexes, so each gets its own package — the
same bytes in both, because a static binary does not care which libc the host
has, but the label must match or the package is invisible to half of Void.

**pacman stops at three** because Arch has no official riscv64, ppc64le or
s390x port for a repository to serve. Those packages are still built and
attached to each release, so `pacman -U <url>` works if you run one of the
unofficial ports.

**The two † rows are keyworded but guarded.** Gentoo's `arm` keyword spans
armv4 through armv7 and both float ABIs, and `ppc64` spans both endiannesses,
while these binaries are armv7 hard-float and little-endian only. The ebuild
checks `CHOST` and refuses with an explanation rather than installing something
that cannot run.

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

## MIPS

**Not shipped.** Rust has no prebuilt `std` for any MIPS target — they are all
tier 3 — so the whole program would need `-Z build-std` on a nightly toolchain.

That is a well-trodden route with either Zig or the OpenWrt SDK, and the case
for it is decent: 2 MB against a router's 8–16 MB of flash is a comfortable
fit. It is absent because nobody has asked, not because it is blocked. If you
want it, these are the traps waiting:

- **`libc` is not installable from any OpenWrt feed** — musl is baked into the
  firmware image. Assembling a test userland means taking the loader out of the
  SDK toolchain, or QEMU stops with `Could not open
  '/lib/ld-musl-mipsel-sf.so.1'`, which reads as an emulator fault rather than
  a missing file.
- **Big- and little-endian 32-bit MIPS artifacts have identical filenames**,
  because `uname -m` reports `mips` for both. Never merge staging directories.
- Soft-float triples are pedantic about their suffixes (`…-muslsf`, not
  `…-musl`), and getting one wrong produces a package a router accepts and then
  will not run.
- `mips64_octeonplus` was built once and **never verified**: it died with
  SIGILL under emulation before the artifact was reached, and
  `QEMU_CPU=Octeon68XX` was necessary but not sufficient. It was deliberately
  not published, on the grounds that an unverified artifact for an exotic
  architecture is worse than none — it looks exactly like the verified ones.

## Termux

**Not shipped either, and this one is nearly free.** `aarch64-linux-android` is
a **tier 2** Rust target, so `std` is prebuilt and there is no interpreter or
ABI tag to satisfy. A Termux build is one target line away; nobody has asked
for one.

The one Android-specific trap in the area is not one this project can hit:
Android's linker refuses undefined symbols in a shared object, and a static
binary has no shared objects to get wrong.

## The honest cost

**Cross-built binaries are less obviously trustworthy than natively built
ones.** That is true, and it is not waved away here. What is checked, and what
is not:

- Every Linux package is installed and started **in a container of that
  distribution**. `verify-repo.sh` covers Debian 12, AlmaLinux 9, AlmaLinux 8,
  Arch, Alpine, Void and Gentoo, against the live repository URL, and each case
  also proves that an untrusted key is *refused*.
- The three BSDs are built and verified **natively on real machines**, not
  cross-built at all, because Zig bundles no BSD libc.
- The OPNsense plugin's binary is compiled and run **inside a FreeBSD 14 root**,
  since OPNsense is 14 and FreeBSD binaries run forward rather than backward.
- **riscv64, ppc64le and s390x are cross-built and executed nowhere.** That is
  the remaining gap, and it is stated rather than hidden. The mitigation is that
  they come off the same one-step toolchain as amd64, from the same source, with
  no emulator and no per-target workarounds — there is far less that can be
  subtly wrong with them than when each one needed its own flags to survive an
  emulator.

If you run one of those three, `breeze-core diag` is the check, and a report is
welcome.
