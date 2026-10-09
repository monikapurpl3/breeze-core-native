### Which file do I need?

**Usually none of them.** Add the [aspic repository](https://aspic.salataputarica.hr.eu.org) once and your package manager installs and updates Breeze Core like anything else. The files below are the same packages, for installing by hand. `@VER@` is this release.

| Your system | Download | Architectures |
|---|---|---|
| **Debian, Ubuntu, Mint, Raspberry Pi OS** | `breeze-core_@VER@-1_<arch>.deb` | `amd64` · `arm64` · `armhf` · `ppc64el` · `riscv64` · `s390x` |
| **Fedora, RHEL, Alma, Rocky, openSUSE** | `breeze-core-@VER@-1.<arch>.rpm` | `x86_64` · `aarch64` · `armv7hl` · `ppc64le` · `riscv64` · `s390x` |
| **Arch, Manjaro, Arch Linux ARM** | `breeze-core-@VER@-1-<arch>.pkg.tar.zst` | `x86_64` · `aarch64` · `armv7h` · `ppc64le` · `riscv64` · `s390x` |
| **Alpine** | `breeze-core_@VER@-r1_<arch>.apk` | `x86_64` · `aarch64` · `armv7` · `ppc64le` · `riscv64` · `s390x` |
| **Void** | `breeze-core-@VER@_1.<arch>.xbps` (`-musl` for musl Void) | `x86_64` · `aarch64` · `armv7l` · `ppc64le` · `riscv64` |
| **OpenWrt 25.12 and newer** | `breeze-core_@VER@-r1_openwrt-<arch>.apk` | `x86_64` · `aarch64_*` · `arm_cortex-a7_neon-vfpv4` · `mips_24kc` · `mipsel_24kc` · `riscv64_riscv64` |
| **OpenWrt 24.10 and older** | `breeze-core_@VER@-1_<arch>.ipk` | the same as above |
| **Termux (Android)** | `breeze-core_@VER@-1_termux-<arch>.deb` | `aarch64` · `arm` · `x86_64` |
| **Windows** | `Breeze-Core-Setup-@VER@.exe` (installer, recommended) or `breeze-core-@VER@-windows-x86_64.zip` | x86-64 |
| **macOS** | `breeze-core-@VER@-macos-<arch>.tar.zst`, or better, [Homebrew](https://github.com/monikapurpl3/breeze-core-native/wiki/Installing-with-Homebrew) | `arm64` (Apple silicon) · `x86_64` (Intel) |
| **FreeBSD** | `breeze-core-@VER@-freebsd-amd64.pkg` | amd64 |
| **NetBSD** | `breeze-core-@VER@-netbsd-amd64.tgz` | amd64 |
| **OpenBSD 7.9** | `breeze-core-@VER@-openbsd-7.9-amd64.tgz` | amd64 |
| **OPNsense** (plugin with a GUI page) | `os-breeze-core-@VER@.pkg` | amd64 |
| **Any other Linux** | `breeze-core-@VER@-linux-<arch>.tar.zst`: one static binary, no dependencies | `amd64` · `arm64` · `armv7` · `riscv64` · `ppc64le` · `s390x` · `mips` · `mipsel` |
| **Building it yourself** | `breeze-core-@VER@.tar.xz` + `breeze-core-@VER@-vendor.tar.xz` (offline crates), or `breeze-core-@VER@-1.src.rpm` | |
| **Docker / Podman** | nothing here: `ghcr.io/monikapurpl3/breeze-core-native:@VER@` | amd64 · arm64 |

Not sure of your architecture? `uname -m` tells you: `x86_64` is amd64, `aarch64` is arm64. The `.tar.zst` files for the BSDs and macOS hold the same binary as their packages. The Windows installer has a `.sha256` beside it; the packages are signed in the aspic repository, so installing from there checks them for you.
