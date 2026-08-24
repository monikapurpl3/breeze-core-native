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
# The BSDs are not here. Zig bundles no FreeBSD/NetBSD/OpenBSD libc, so those
# are built on the real machine — see packaging/bsd/. This script prints what it
# did not build rather than leaving the omission to be noticed later.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"
OUT="packaging/out/bin"
DIST="packaging/out/dist"
VERSION="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"

# label | rust target | note
TARGETS="
amd64|x86_64-unknown-linux-musl|static musl
arm64|aarch64-unknown-linux-musl|static musl
armv7|armv7-unknown-linux-musleabihf|static musl, hard float (Pi 2/3, routers)
riscv64|riscv64gc-unknown-linux-musl|static musl
ppc64le|powerpc64le-unknown-linux-musl|static musl
s390x|s390x-unknown-linux-gnu.2.17|glibc 2.17 floor (no musl std upstream)
windows|x86_64-pc-windows-msvc|native build, no Zig
"

want=("$@")
selected() {
  [ ${#want[@]} -eq 0 ] && return 0
  for w in "${want[@]}"; do [ "$w" = "$1" ] && return 0; done
  return 1
}

# The commit goes into the binary, so /api/version and /metrics can identify
# exactly what is running. A dirty tree is labelled as such rather than passed
# off as the commit it nearly is.
if git rev-parse --short HEAD >/dev/null 2>&1; then
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
  case "$target" in
    *windows-msvc)
      # The host toolchain already targets this; zigbuild would only get in the
      # way, and the MSVC linker is what produces a binary Windows runs without
      # a shipped libc.
      cargo build --release --target "$target"
      exe=".exe"
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
    tar --zstd -cf "$DIST/breeze-core-$VERSION-linux-$label.tar.zst" \
        -C "$OUT/$label" breeze-core -C "$REPO" LICENSE README.md
    echo "  -> $DIST/breeze-core-$VERSION-linux-$label.tar.zst"
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
echo "not built here (built on the real machine — see packaging/bsd/):"
echo "  netbsd-amd64   freebsd-amd64   freebsd-arm64   openbsd-amd64"
