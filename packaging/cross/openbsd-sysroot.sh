#!/usr/bin/env bash
# OpenBSD's own libraries and headers, as a sysroot to link against from here.
#
#   packaging/cross/openbsd-sysroot.sh [release] [arch]     # default 7.9 amd64
#
# Prints the sysroot's path on stdout; everything else goes to stderr. The
# result is cached under packaging/out/cross/, so only the first build of a
# release downloads anything (base + comp, about 600 MB).
#
# Why not zig, which links FreeBSD and NetBSD fine: zig's OpenBSD libc is
# generated stubs, so its binary asks for a bare `libc.so` instead of
# `libc.so.103.0`, and has none of the marking OpenBSD's toolchain adds. The
# release's real libraries give a binary identical in its dependencies to one
# built on the machine -- which is what 4.3.2 and earlier were.
#
# OpenBSD moves its libc every six months and a binary is valid for the
# release it was linked against, hence one sysroot per release, the release in
# the published path (/openbsd/<rel>/packages/<arch>/), and checksums pinned
# per release below: a new release means adding a line from its SHA256 file,
# https://cdn.openbsd.org/pub/OpenBSD/<rel>/<arch>/SHA256.
#
# Three things the sysroot needs that a plain extraction does not give:
#
#   * no symlinks. Git Bash's tar makes them, and clang on Windows cannot open
#     a header through one (usr/include/machine -> amd64), so each is replaced
#     by a copy of what it points at.
#   * an unversioned name beside each library. OpenBSD's linker finds
#     libc.so.103.0 for -lc; upstream lld looks for libc.so and, not finding it,
#     silently links libc.a instead -- a static binary. A copy named libc.so is
#     found, and because the library's SONAME is libc.so.103.0 that is still
#     the name the binary records.
#   * the startup objects (crt0.o, crtbegin.o, ...) and libcompiler_rt.a, which
#     are in base, beside the shared libraries -- not in comp with libc.a.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
REL="${1:-7.9}"
ARCH="${2:-amd64}"

case "$REL/$ARCH" in
  7.9/amd64)
    BASE_SHA=923d2e03f06408d50d4848334398c6d04b5514dcac7917badfc178a0eef248de
    COMP_SHA=21a67af20aebcabf85b09f4206fc95b4cae0a35d42b154b976f0159f457724f9 ;;
  *)
    echo "no pinned checksums for OpenBSD $REL/$ARCH -- add them from" >&2
    echo "  https://cdn.openbsd.org/pub/OpenBSD/$REL/$ARCH/SHA256" >&2
    exit 1 ;;
esac

v="${REL/./}"
ROOT="$REPO/packaging/out/cross"
DEST="$ROOT/sysroot/openbsd-$REL-$ARCH"
if [ -f "$DEST/.ready" ]; then echo "$DEST"; exit 0; fi

DL="$ROOT/dl/openbsd-$REL-$ARCH"
mkdir -p "$DL"
fetch() {  # set-file sha256
  local f="$1" sum="$2"
  if [ -f "$DL/$f" ] && echo "$sum  $DL/$f" | sha256sum -c --quiet - >/dev/null 2>&1; then
    return 0
  fi
  echo "  fetching $f" >&2
  curl -fsSL -o "$DL/$f" "https://cdn.openbsd.org/pub/OpenBSD/$REL/$ARCH/$f"
  echo "$sum  $DL/$f" | sha256sum -c --quiet - >&2 \
    || { echo "!! $f does not match its pinned SHA-256" >&2; rm -f "$DL/$f"; exit 1; }
}
echo "=== OpenBSD $REL/$ARCH sysroot" >&2
fetch "base$v.tgz" "$BASE_SHA"
fetch "comp$v.tgz" "$COMP_SHA"

work="$DEST.partial"
rm -rf "$work"
mkdir -p "$work"
bsdtar -xzf "$DL/base$v.tgz" -C "$work" './usr/lib/lib*.so.*' './usr/lib/*.o' ./usr/lib/libcompiler_rt.a
bsdtar -xzf "$DL/comp$v.tgz" -C "$work" ./usr/lib ./usr/include

( cd "$work"
  find . -type l | while IFS= read -r link; do
    target="$(readlink "$link")"
    dir="$(dirname "$link")"
    rm "$link"
    cp -r "$dir/${target%/}" "$link"
  done
  [ -z "$(find . -type l -print -quit)" ] || { echo "!! symlinks left in the sysroot" >&2; exit 1; }

  cd usr/lib
  # Highest version last, so it is the one left under the bare name.
  for lib in $(ls lib*.so.*.* | sort -V); do
    cp -f "$lib" "${lib%%.so.*}.so"
  done
)

[ -f "$work/usr/lib/libc.so" ] && [ -f "$work/usr/lib/crt0.o" ] \
  || { echo "!! incomplete sysroot: no libc.so or crt0.o" >&2; exit 1; }
touch "$work/.ready"
rm -rf "$DEST"
mv "$work" "$DEST"
echo "$DEST"
