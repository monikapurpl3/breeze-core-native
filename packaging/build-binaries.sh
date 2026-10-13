#!/usr/bin/env bash
# Build the published binaries, one static executable per target.
#
#   ./packaging/build-binaries.sh            # every target below
#   ./packaging/build-binaries.sh amd64 arm64
#
# Output: packaging/out/bin/<label>/breeze-core (+ .exe on Windows), a zstd
# tarball per target in packaging/out/dist/, and a SIZES file.
#
# **The executable is always named `breeze-core`.** Not breeze-core-native, not
# breeze-core-amd64: this replaces the Python package of the same name, on the
# same paths, and a user who types `breeze-core` should not have to care which
# implementation answers. The architecture lives in the *archive* name, and the
# file inside it does not carry it.
#
# Linux targets are **static musl**, linked by Zig (`cargo zigbuild`), which
# supplies libc and the linker for every architecture — the whole reason Zig is
# in this project. Static musl also means one binary per architecture rather
# than one per libc: the .deb, .rpm, .apk, .pkg.tar.zst and .ipk for a given
# architecture all wrap the *same* file, and it runs on a distro older than any
# container we could build in.
#
# s390x is the exception, built against glibc: there is no prebuilt musl std for
# it and no musl distro on the platform to want one. Zig pins the glibc floor at
# 2.17, which is what the Python build achieved by compiling inside deliberately
# ancient containers.
#
# The BSDs are cross-built here too, since 4.4.0, each against the libc its own
# release uses (packaging/cross/README.md has the detail):
#
#   FreeBSD   zig, which ships FreeBSD 14's libc; the binary also runs on
#             OPNsense, which is FreeBSD 14 or 15.
#   NetBSD    zig again (NetBSD 10.1's libc), through packaging/cross/zcc.js,
#             because cargo-zigbuild refuses the target.
#   OpenBSD   clang and lld against the release's real libraries, downloaded
#             and checksum-pinned by packaging/cross/openbsd-sysroot.sh: zig's
#             OpenBSD libc is stubs, and its binary asks for a bare libc.so.
#
# Until 4.3.2 each of them was compiled on a VM of that system. The VMs now only
# run the verify scripts.
#
# Zig is pinned: 0.16.0, as in CI. A different zig is refused rather than used,
# because 0.17.0 arrived on the workstation by itself one night and could not
# build i686 at all (its musl cache step fails). Point BC_ZIG at a 0.16.0 if
# the one on PATH is not it.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"
# Files read off disk must have the line endings .gitattributes pins (see it).
bash packaging/check-eol.sh
OUT="packaging/out/bin"
DIST="packaging/out/dist"
VERSION="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
# The OpenBSD release the openbsd-amd64 binary is linked against, and so the
# only one it runs on. It is also the release in the published package path.
OPENBSD_REL="7.9"

ZIG_PIN="0.16.0"
ZIG="${BC_ZIG:-zig}"
zig_have="$("$ZIG" version 2>/dev/null || echo none)"
if [ "$zig_have" != "$ZIG_PIN" ]; then
  echo "!! zig $zig_have at '$ZIG'; this build is pinned to $ZIG_PIN (as CI is)."
  echo "   Install $ZIG_PIN beside it and run with BC_ZIG=/path/to/zig-$ZIG_PIN/zig"
  exit 1
fi
# cargo-zigbuild reads the first, packaging/cross/zcc.js the second.
export CARGO_ZIGBUILD_ZIG_PATH="$ZIG" ZIG

# label | rust target | note
TARGETS="
amd64|x86_64-unknown-linux-musl|static musl
arm64|aarch64-unknown-linux-musl|static musl
armv7|armv7-unknown-linux-musleabihf|static musl, hard float (Pi 2/3, routers)
armv6|arm-unknown-linux-musleabihf|static musl, hard float (Pi Zero/Pi 1; also the armhf .deb)
i686|i686-unknown-linux-musl|static musl, 32-bit x86 with SSE2 (Pentium 4 on)
riscv64|riscv64gc-unknown-linux-musl|static musl
ppc64le|powerpc64le-unknown-linux-musl|static musl
ppc64|powerpc64-unknown-linux-musl|static musl, big-endian (Power Mac G5, POWER)
loongarch64|loongarch64-unknown-linux-musl|static musl (Loongson)
s390x|s390x-unknown-linux-gnu.2.17|glibc 2.17 floor (no musl std upstream)
freebsd-amd64|x86_64-unknown-freebsd|FreeBSD 14+ and OPNsense, zig's FreeBSD libc
netbsd-amd64|x86_64-unknown-netbsd|NetBSD 10.1+, zig's NetBSD libc
openbsd-amd64|x86_64-unknown-openbsd|OpenBSD $OPENBSD_REL only, its own libraries
macos-arm64|aarch64-apple-darwin|Apple silicon, macOS 13+ (see packaging/macos/)
macos-x86_64|x86_64-apple-darwin|Intel, macOS 13+ (see packaging/macos/)
windows|x86_64-pc-windows-msvc|native build, no Zig
"

# A command cargo or cc-rs can run as a linker or compiler: on Windows that has
# to be an executable, so a .cmd; elsewhere a sh script. Paths among the fixed
# arguments are converted for Windows here, once.
#
#   shim NAME PROGRAM [FIXED-ARGS...]      -> prints the shim's path
SHIMS="$REPO/packaging/out/cross/shims"
shim() {
  local name="$1" a line
  shift
  mkdir -p "$SHIMS"
  if command -v cygpath >/dev/null 2>&1; then
    line="@"
    for a in "$@"; do
      case "$a" in /*|[A-Za-z]:/*) a="$(cygpath -w "$a")" ;; esac
      line="$line\"$a\" "
    done
    # The line goes in as an argument, never as the format: it holds Windows
    # paths, and printf would read their backslashes as escapes.
    printf '%s%%*\r\n' "$line" > "$SHIMS/$name.cmd"
    cygpath -w "$SHIMS/$name.cmd"
  else
    { printf '#!/bin/sh\nexec'; printf ' %q' "$@"; printf ' "$@"\n'; } > "$SHIMS/$name"
    chmod +x "$SHIMS/$name"
    echo "$SHIMS/$name"
  fi
}

want=("$@")
selected() {
  [ ${#want[@]} -eq 0 ] && return 0
  for w in "${want[@]}"; do [ "$w" = "$1" ] && return 0; done
  return 1
}

# The commit goes into the binary, so /api/version and /metrics can identify
# exactly what is running. A dirty tree is labelled as such rather than passed
# off as the commit it nearly is.
#
# BREEZE_COMMIT may be given instead -- for adding architectures to a release
# whose code has not changed since its tag, when HEAD is only packaging work
# on top. It is accepted only if the code is byte-identical to that commit's.
if [ -n "${BREEZE_COMMIT:-}" ]; then
  code=(crates static Cargo.toml Cargo.lock)
  if ! git diff --quiet "$BREEZE_COMMIT" -- "${code[@]}" 2>/dev/null; then
    echo "!! BREEZE_COMMIT=$BREEZE_COMMIT, but the code differs from it:"
    git diff --stat "$BREEZE_COMMIT" -- "${code[@]}" | tail -3
    exit 1
  fi
  export BREEZE_COMMIT
elif git rev-parse --short HEAD >/dev/null 2>&1; then
  BREEZE_COMMIT="$(git rev-parse --short HEAD)"
  git diff --quiet HEAD 2>/dev/null || BREEZE_COMMIT="$BREEZE_COMMIT-dirty"
  export BREEZE_COMMIT
fi

mkdir -p "$OUT" "$DIST"
echo "breeze-core $VERSION (${BREEZE_COMMIT:-no git})"
built=0

while IFS='|' read -r label target note; do
  [ -n "$label" ] || continue
  selected "$label" || continue
  printf '\n=== %s (%s) — %s\n' "$label" "$target" "$note"

  exe=""
  upper="$(printf '%s' "$target" | tr 'a-z-' 'A-Z_')"
  under="$(printf '%s' "$target" | tr '-' '_')"
  case "$target" in
    *-unknown-netbsd)
      # zig ships NetBSD's libc; only cargo-zigbuild's target list lacks it.
      cc="$(shim "cc-$target" node "$REPO/packaging/cross/zcc.js" "${target%%-*}-netbsd")"
      ar="$(shim "ar-$target" "$ZIG" ar)"
      env "CARGO_TARGET_${upper}_LINKER=$cc" "CC_${under}=$cc" "AR_${under}=$ar" \
        cargo build --release --target "$target"
      ;;
    *-unknown-openbsd)
      # Tier 3, so std is built from source. IBT: OpenBSD enforces it on amd64,
      # and a binary without endbr64 landing pads faults on its first indirect
      # call on a CPU that has it. OpenBSD's own rustc turns it on; this does
      # the same for the Rust code, std included, and for ring's C.
      sysroot="$("$REPO/packaging/cross/openbsd-sysroot.sh" "$OPENBSD_REL" amd64)"
      cc="$(shim "cc-$target" node "$REPO/packaging/cross/xclang.js" \
             "x86_64-unknown-openbsd$OPENBSD_REL" "$sysroot" --)"
      ar="$(shim "ar-$target" llvm-ar)"
      env "CARGO_TARGET_${upper}_LINKER=$cc" "CC_${under}=$cc" "AR_${under}=$ar" \
          "CFLAGS_${under}=-fcf-protection=branch" \
          "CARGO_TARGET_${upper}_RUSTFLAGS=-Zcf-protection=branch" \
        cargo +nightly build --release --target "$target" -Zbuild-std=std,panic_abort
      ;;
    loongarch64-*)
      # Rust's loongarch64 musl target links dynamically unless told otherwise,
      # brings its own crt1.o (so zig's makes a duplicate _start), and passes
      # -Wl,--no-rosegment, which zig's linker refuses; zcc.js drops that one.
      cc="$(shim "cc-$target" node "$REPO/packaging/cross/zcc.js" loongarch64-linux-musl)"
      ar="$(shim "ar-$target" "$ZIG" ar)"
      env "CARGO_TARGET_${upper}_LINKER=$cc" "CC_${under}=$cc" "AR_${under}=$ar" \
          "CARGO_TARGET_${upper}_RUSTFLAGS=-C target-feature=+crt-static -C link-self-contained=no" \
        cargo build --release --target "$target"
      ;;
    *windows-msvc)
      # The host toolchain already targets this; zigbuild would only get in the
      # way, and the MSVC linker is what produces a binary Windows runs without
      # a shipped libc.
      cargo build --release --target "$target"
      exe=".exe"
      ;;
    *-apple-darwin)
      # zig has no Apple frameworks, and iana-time-zone needs CoreFoundation:
      # link against the stub in packaging/macos/ (its README says why that is
      # enough). For this target only, so no other target's flags change. A
      # Windows path, because cargo hands it to zig without MSYS converting it.
      fw="$REPO/packaging/macos"
      command -v cygpath >/dev/null 2>&1 && fw="$(cygpath -m "$fw")"
      env "CARGO_TARGET_$(echo "$target" | tr 'a-z-' 'A-Z_')_RUSTFLAGS=-C link-arg=-F$fw" \
        cargo zigbuild --release --target "$target"
      ;;
    *)
      # RUSTFLAGS is deliberately not set here: +crt-static is already the
      # default for musl targets, and forcing it on the glibc one produces a
      # binary that dies inside getaddrinfo — which surfaces only when cloud
      # pairing is used, long after any test that would have caught it.
      cargo zigbuild --release --target "$target"
      ;;
  esac

  # A target may carry a glibc floor (`…-gnu.2.17`), which is cargo-zigbuild's
  # syntax and not cargo's: it strips the suffix before invoking cargo, so the
  # output directory is the bare triple.
  triple="$(echo "$target" | sed -E 's/\.[0-9]+\.[0-9]+$//')"
  src="target/$triple/release/breeze-core$exe"
  [ -f "$src" ] || { echo "  MISSING: $src"; exit 1; }
  mkdir -p "$OUT/$label"
  cp "$src" "$OUT/$label/breeze-core$exe"

  # One archive per target, zstd, with the binary and the licence in it. Not a
  # bare download: a stray `breeze-core` in a browser's downloads folder is
  # indistinguishable from any other, and the licence has to travel with it.
  if [ -z "$exe" ]; then
    # A label that names its own OS keeps it; every other one is a Linux
    # architecture.
    case "$label" in
      macos-*|freebsd-*|netbsd-*|openbsd-*) name="$label" ;;
      *) name="linux-$label" ;;
    esac
    "$REPO/packaging/dist-archive.sh" "$DIST/breeze-core-$VERSION-$name.tar.zst" "$OUT/$label"
    echo "  -> $DIST/breeze-core-$VERSION-$name.tar.zst"
  else
    # A zip for Windows, not a zstd tarball: Explorer opens one and not the
    # other, and a Windows user should not need a second tool to unpack the
    # first one.
    #
    # This step exists because it did not. The first 4.0.0 Windows zip was
    # assembled by hand, so when every binary was rebuilt the zip silently kept
    # the old executable -- it was the one artifact with no build step, and it
    # was the one artifact that went stale.
    # Absolute, because the archive is written from inside a subshell that has
    # cd'd into the staging directory, where a relative $DIST means nothing.
    zipfile="$REPO/$DIST/breeze-core-$VERSION-windows-x86_64.zip"
    rm -f "$zipfile"
    stage="$(mktemp -d)"
    cp "$OUT/$label/breeze-core$exe" "$REPO/LICENSE" "$REPO/README.md" "$stage/"
    # bsdtar writes zip too; -a picks the format from the suffix.
    ( cd "$stage" && bsdtar -a -cf "$zipfile" "breeze-core$exe" LICENSE README.md )
    rm -rf "$stage"
    echo "  -> $zipfile"
  fi
  built=$((built + 1))
done <<< "$TARGETS"

[ "$built" -gt 0 ] || { echo "nothing matched: ${want[*]}"; exit 1; }

echo
echo "=== sizes ==="
{
  echo "breeze-core $VERSION"
  for d in "$OUT"/*/; do
    label="$(basename "$d")"
    for f in "$d"breeze-core*; do
      printf '%-12s %8s KB  %s\n' "$label" \
        "$(( ($(wc -c < "$f") + 1023) / 1024 ))" "$(basename "$f")"
    done
  done
} | tee "$OUT/SIZES"

echo
echo "built by their own scripts, not this one:"
echo "  mips mipsel mips64    packaging/mips/build-mips.sh       (OpenWrt's toolchains, in Docker)"
echo "  termux-*              packaging/termux/build-packages.sh (Android NDK)"
