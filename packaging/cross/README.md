# packaging/cross — compiling for systems this isn't

Every binary is built on the workstation (Windows, Git Bash). Most targets need
nothing from here: `cargo zigbuild` lets zig supply the C library and the
linker, which is why zig is in this project at all. This directory is for the
targets where that is not enough.

| file | what for |
|---|---|
| `zcc.js` | `zig cc` for a target cargo-zigbuild will not drive: NetBSD, loongarch64 |
| `xclang.js` | clang + lld against a downloaded sysroot: OpenBSD |
| `openbsd-sysroot.sh` | fetches and prepares an OpenBSD release's own libraries |
| `install-qemu-binfmt.sh` | registers QEMU for running foreign binaries in Docker |

cargo and cc-rs need an executable they can start, so `build-binaries.sh`
writes a one-line `.cmd` (or `sh`) shim per target into
`packaging/out/cross/shims/` that runs these with node. The wrappers exist for
two reasons zig cannot be handed the job directly: cc-rs adds
`--target=<rust triple>` to every clang-like compiler, and zig cannot parse
`x86_64-unknown-netbsd`; and a few Rust targets pass linker flags zig refuses.

## Per target

| target | how | why not the default |
|---|---|---|
| FreeBSD | `cargo zigbuild` | — zig ships FreeBSD 14's libc |
| NetBSD | `cargo build`, linker and CC = zcc.js | cargo-zigbuild's target list lacks NetBSD; zig has its libc (10.1) |
| OpenBSD | `cargo +nightly build -Zbuild-std`, linker = xclang.js | tier 3; zig's OpenBSD libc is stubs, so its binary asks for a bare `libc.so` |
| loongarch64 | `cargo build`, linker = zcc.js, `+crt-static -C link-self-contained=no` | Rust's target links dynamically by default, brings a second `_start`, and passes `--no-rosegment` |
| MIPS, Octeon | `packaging/mips/build-mips.sh` | OpenWrt's toolchains: zig 0.16's 32-bit MIPS musl is broken |

## OpenBSD, in more detail

OpenBSD moves its libc every six months and a binary runs on the release it was
linked against, so `openbsd-sysroot.sh` downloads that release's `base` and
`comp` sets (checksums pinned per release) and builds a sysroot:

- **no symlinks** — clang on Windows cannot open a header through the ones tar
  makes, so each is replaced by a copy;
- **`libX.so` beside each `libX.so.M.N`** — upstream lld does not do OpenBSD's
  versioned library search, finds no `libc.so`, and quietly links `libc.a`
  instead. The copy is found, and since its SONAME is `libc.so.103.0` that is
  still what the binary records;
- **the startup objects and `libcompiler_rt.a` come from base**, not comp.

The result has the same dependencies as a binary built on OpenBSD
(`libc.so.103.0`, `libpthread.so.28.1`, `libc++abi.so.9.0`), the
`PT_OPENBSD_RANDOMIZE` header, and **IBT landing pads**:
`-Zcf-protection=branch` for Rust (std included, via build-std) and
`-fcf-protection=branch` for ring's C. OpenBSD enforces IBT on amd64; a binary
without `endbr64` faults on its first indirect call on a CPU that has it.

## Zig is pinned

`build-binaries.sh` refuses any zig but 0.16.0 (CI's). 0.17.0 once arrived on
the workstation through an automatic update and could not build i686 at all.
`BC_ZIG` points at a pinned copy when the one on PATH is not it.

## Running what was built

i686 runs natively. Docker Desktop's VM has QEMU for arm, aarch64, riscv64,
ppc64le, s390x and mips64; `install-qemu-binfmt.sh ppc64 loongarch64` adds the
two it lacks (and, like MIPS's, does not survive Docker restarting its VM).

**Octeon cannot be run under QEMU**, and it is not a fault in the build: the
OpenWrt Octeon toolchain compiles C with Octeon-only instructions, which QEMU's
generic MIPS64 CPU lacks, while QEMU's Octeon CPU has no FPU — and real Octeon
has none either; the Linux kernel emulates floating point there, which QEMU's
user mode does not. A real EdgeRouter is the only check.
