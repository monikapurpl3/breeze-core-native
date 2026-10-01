#!/usr/bin/env bash
# Build the 32-bit MIPS binaries, for OpenWrt routers.
#
#   ./packaging/mips/build-mips.sh              # mipsel and mips
#   ./packaging/mips/build-mips.sh mipsel
#
# Output, beside build-binaries.sh's: packaging/out/bin/<label>/breeze-core and
# packaging/out/dist/breeze-core-<ver>-linux-<label>.tar.zst. The packagers then
# wrap them as for every other architecture (ipk for opkg, and the OpenWrt apk
# packages); there is no deb, rpm or pacman package because no distribution
# that uses those still ships 32-bit MIPS.
#
# Not in build-binaries.sh because it cannot be: MIPS std has to be compiled
# from source on a nightly (-Z build-std), and the linker is OpenWrt's own,
# which is a Linux binary. Both live in the image built from the Dockerfile
# here.
#
# **Static, like every other Linux build.** Rust's mips*-musl targets default
# to linking dynamically against the router's own /lib/ld-musl-*-sf.so.1, which
# would make these the only binaries tied to the firmware's musl.
# +crt-static undoes that, and then two more things are needed, because a
# static musl link normally uses startup files and an unwinder that ship with
# a target's prebuilt std, and tier 3 targets have none:
#
#   -C link-self-contained=no    crt1.o, crti.o and libc.a from the OpenWrt
#                                toolchain's own sysroot instead
#   -L native=/opt/unwind/<arch> a libunwind.a that is the toolchain's
#                                libgcc_eh.a (see the Dockerfile)
#
# -Z build-std-features=llvm-libunwind does NOT build one: the unwind crate has
# no build script, and LLVM's libunwind is built by rustc's own bootstrap, so
# that feature only changes which libunwind.a the link expects to exist.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
OUT="packaging/out/bin"
DIST="packaging/out/dist"
VERSION="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"

# label | rust target | OpenWrt toolchain | note
TARGETS="
mipsel|mipsel-unknown-linux-musl|mipsel_24kc|little-endian, soft float (MT7621, MT76x8)
mips|mips-unknown-linux-musl|mips_24kc|big-endian, soft float (ath79)
"

want=("$@")
selected() {
  [ ${#want[@]} -eq 0 ] && return 0
  for w in "${want[@]}"; do [ "$w" = "$1" ] && return 0; done
  return 1
}

if git rev-parse --short HEAD >/dev/null 2>&1; then
  BREEZE_COMMIT="$(git rev-parse --short HEAD)"
  git diff --quiet HEAD 2>/dev/null || BREEZE_COMMIT="$BREEZE_COMMIT-dirty"
fi

docker build -q -t bc-mips packaging/mips >/dev/null
echo "breeze-core $VERSION for MIPS (${BREEZE_COMMIT:-no git}), builder image ready"
export MSYS_NO_PATHCONV=1
mkdir -p "$OUT" "$DIST"

while IFS='|' read -r label target owrt note; do
  [ -n "$label" ] || continue
  selected "$label" || continue
  printf '\n=== %s (%s) — %s\n' "$label" "$target" "$note"
  under="$(printf '%s' "$target" | tr '-' '_')"
  upper="$(printf '%s' "$under" | tr '[:lower:]' '[:upper:]')"
  gcc="/opt/$owrt/bin/${target%%-*}-openwrt-linux-musl-gcc"

  # The working tree, as build-binaries.sh builds it, streamed in; the binary
  # streamed out. Nothing is mounted writable (see build-packages.sh for why).
  mkdir -p "$OUT/$label"
  tar -cf - Cargo.toml Cargo.lock crates static \
    | docker run --rm -i -e BREEZE_COMMIT="${BREEZE_COMMIT:-unknown}" \
        -e "CARGO_TARGET_${upper}_LINKER=$gcc" -e "CC_${under}=$gcc" -e "AR_${under}=${gcc%-gcc}-ar" \
        -e "CARGO_TARGET_${upper}_RUSTFLAGS=-C target-feature=+crt-static -C link-self-contained=no -L native=/opt/unwind/$owrt" \
        bc-mips sh -euc "
          exec 3>&1 1>&2
          mkdir -p /work/src && cd /work/src && tar -xf -
          cargo build --release --locked -Zbuild-std=std,panic_abort --target $target -p breeze-core
          f=target/$target/release/breeze-core
          file \$f
          # Static means no interpreter at all; anything else is the wrong build.
          if file \$f | grep -q 'interpreter'; then echo '!! dynamically linked'; exit 1; fi
          tar -cf - -C target/$target/release breeze-core >&3
        " | tar -xf - -C "$OUT/$label"

  tar --zstd -cf "$DIST/breeze-core-$VERSION-linux-$label.tar.zst" \
      -C "$OUT/$label" breeze-core -C "$REPO" LICENSE README.md
  printf '  %s KB -> %s\n' "$(( ($(wc -c < "$OUT/$label/breeze-core") + 1023) / 1024 ))" \
      "$DIST/breeze-core-$VERSION-linux-$label.tar.zst"
done <<< "$TARGETS"
