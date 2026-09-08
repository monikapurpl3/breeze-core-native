# Ports and architectures

**This page used to be called "Proof-of-concept architectures", and that is the
change worth describing.** In the Python line it documented an argument: that
emulating exotic architectures had stopped being merely slow and started being
unreliable, and that cross-building was the way out. It ended with a list of
targets that were *built but not shipped* — riscv64, ppc64le, s390x, MIPS — and a
section on a wheel that had been compiled but never run.

There is no proof-of-concept tier any more. Every architecture below either has
a signed package published on every release, or is honestly listed as absent.

## What ships

| Architecture | deb | rpm | pacman | apk | OpenWrt ipk | tarball |
|---|---|---|---|---|---|---|
| x86-64 | ✅ `amd64` | ✅ `x86_64` | ✅ `x86_64` | ✅ `x86_64` | ✅ `x86_64` | ✅ |
| ARM64 | ✅ `arm64` | ✅ `aarch64` | ✅ `aarch64` | ✅ `aarch64` | ✅ 3 targets | ✅ |
| ARMv7 | ✅ `armhf` | ✅ `armv7hl` | ✅ `armv7h` | ✅ `armv7` | ✅ `arm_cortex-a7_neon-vfpv4` | ✅ |
| riscv64 | ✅ | ✅ | — | ✅ | ✅ `riscv64_riscv64` | ✅ |
| ppc64le | ✅ `ppc64el` | ✅ | — | ✅ | — | ✅ |
| s390x | ✅ | ✅ | — | ✅ | — | ✅ |

Plus one native package each for **FreeBSD**, **NetBSD** and **OpenBSD**, an
**OPNsense** plugin, and a **Windows** installer. The three OpenWrt ARM64
targets are `aarch64_generic`, `aarch64_cortex-a53` and `aarch64_cortex-a72` —
OpenWrt's package architecture has to match exactly, so they are separate.

pacman has no official riscv64/ppc64le/s390x, which is why those three columns
stop there rather than because anything failed.

## What changed, mechanically

The Python pipeline had two problems that were really one problem: **something
had to run on the target.**

- `pydantic-core` is Rust behind `maturin`, so any architecture without a
  prebuilt wheel had to compile it.
- **PyInstaller's freeze step is not a cross-compiler.** It imports the
  application to analyse it and embeds *the target's* interpreter and shared
  objects, so it has to run where those live.

That is what made emulation load-bearing, and emulation is where it came apart:

| Target | What emulation did |
|---|---|
| riscv64 | worked, ~2 h, dominated by pydantic-core's Rust compile |
| s390x | worked in ~45 min, but only after **gcc segfaulted in `cc1`** |
| ppc64le | **gcc segfaulted in `collect2`**, then again from rustc's link step |
| Termux arm64 | **deadlocked twice** — qemu-user's futex handling wedges under a parallel cargo build |
| MIPS | not attempted: Rust has no prebuilt `std` for any MIPS target |

Three compiler failures and one emulator failure, none of them failures of the
code being built.

A Rust binary has neither problem. There is no interpreter to embed and no
freeze step, so the whole "must run on the target" constraint disappears —
`cargo-zigbuild` produces a finished executable for every Linux target on the
build machine, in about twenty seconds each, and **no QEMU is involved at any
point.** The gcc ICEs and the futex deadlock cannot happen, because they were
properties of the emulator.

```
packaging/build-binaries.sh          # every target, one static executable each
packaging/build-binaries.sh amd64    # or just one
```

## Static musl, and the one exception

Linux targets are **static musl**, linked by Zig. There is no libc at runtime, so
one binary per architecture serves every distribution — the same file installs on
Debian 12, Alpine and RHEL 8, which is why there is no per-distro build matrix
anywhere in this project.

**s390x is the exception, and it is built against glibc**, with Zig pinning the
floor at 2.17 (`s390x-unknown-linux-gnu.2.17`). Two reasons: there is no prebuilt
musl `std` for it upstream, and there is no musl distribution on the platform to
want one. Worth knowing why *static glibc* is not the answer instead: statically
linking glibc breaks `getaddrinfo`, because NSS loads its backends as shared
objects at runtime. A statically-linked glibc binary resolves nothing, and it
fails at the point where it tries to reach an air conditioner by name — which is
a long way from where you would look.

## MIPS

**Not shipped, and not for the old reason.** The Python line could not ship MIPS
because Rust has no prebuilt `std` for those targets (tier 3), so `pydantic-core`
needed `-Z build-std` — the same wall this project would hit, except that here it
is the *whole program* rather than one dependency.

It is a smaller wall than it was: `-Z build-std` with a nightly toolchain and
either Zig or the OpenWrt SDK is a well-trodden route, and the router argument
is stronger than ever now that the binary is 2 MB rather than a 25 MB frozen
bundle against 8–16 MB of flash. It is absent because nobody has asked for it,
not because it is blocked. If you want it, the OpenWrt SDK notes from the Python
line's page still apply — in particular:

- OpenWrt CPython accepted `.cpython-311-mips-linux-musl**sf**.so` while maturin
  emitted `…-musl.so`. Irrelevant here (no extension modules), but the same
  soft-float triple pedantry applies to the binary.
- **`libc` is not installable from any OpenWrt feed** — musl is baked into the
  firmware image. Assembling a test userland means taking the loader from the SDK
  toolchain, or QEMU stops with `Could not open '/lib/ld-musl-mipsel-sf.so.1'`,
  which reads as a QEMU fault.
- **BE and LE 32-bit MIPS artifacts have identical filenames**, because `uname
  -m` is `mips` on both. Never merge staging directories.
- `mips64_octeonplus` was built and **never verified**: the emulated interpreter
  died with SIGILL before the artifact was reached. `QEMU_CPU=Octeon68XX` was
  necessary but not sufficient, and the static/dynamic difference was never
  explained. It was deliberately not published, on the grounds that an unverified
  artifact for an exotic architecture is worse than none, because it looks like
  the verified ones.

## Termux

**Not shipped either, and this one is nearly free.** Termux was the target that
justified the whole cross-building argument — it deadlocked twice under emulation
and could not be built at all, and the two-stage route got a `pydantic-core`
wheel out of it in 55 seconds.

None of that machinery is needed now. `aarch64-linux-android` is a **tier 2**
Rust target, so `std` is prebuilt; there are no wheels, no ABI tags to match and
no interpreter to satisfy. The one Android-specific trap that remains is not one
this project can hit: Android's linker refuses undefined symbols in a shared
object, so a CPython extension had to link `libpython3.14.so` explicitly. A
static binary has no shared objects to get wrong.

## The honest cost, restated

The Python page ended by admitting that **cross-built binaries are less obviously
trustworthy than natively built ones**, and mitigated it by running
`breeze-core version` on the target under emulation before publishing.

That trade-off has not gone away, and it has not been mitigated the same way.
What is checked instead:

- Every Linux package is installed and started **in a container of that
  distribution** — `verify-repo.sh` does Debian 12, AlmaLinux 9, AlmaLinux 8,
  Arch and Alpine, from the live repository URL, including that an untrusted key
  is refused.
- The three BSDs are built and verified **natively on real machines**, not
  cross-built at all, because Zig bundles no BSD libc.
- The OPNsense plugin's binary is compiled and run **inside a FreeBSD 14 root**,
  since OPNsense is 14 and binaries run forward rather than backward.
- The exotic Linux architectures — riscv64, ppc64le, s390x — are **cross-built
  and not executed anywhere in CI.** That is the remaining gap, and it is the
  same gap the Python line had. The mitigation is that they are now built by the
  same one-step toolchain as amd64, from the same source, with no emulator and no
  per-target workarounds: there is far less that can be subtly wrong about them
  than when each one needed its own compiler flags to get through QEMU.

If you run one of those three, `breeze-core diag` is the check, and a report is
welcome.
