#!/usr/bin/env bash
# Gather the GitHub release assets for this version into packaging/out/release.
#
#   ./packaging/repo/assemble-release.sh
#   gh release create v<ver> --notes-file NOTES.md packaging/out/release/*
#
# Taken from the SIGNED, VERIFIED repository tree (packaging/out/aspic) rather
# than from each packager's staging directory, so a release asset is byte for
# byte what the repositories serve -- some formats are touched on the way in.
# The standalone tarballs come from packaging/out/dist, which nothing signs.
#
# Until 4.1.0 this was done by hand, 56 files at a time, renaming the BSD
# packages as it went. This does the same, and refuses rather than ships short:
# every family must be present, and nothing from another version may be.
set -euo pipefail
cd "$(dirname "$0")/../.."

VER="$(sed -n 's/^version *= *"\(.*\)"/\1/p' crates/breeze-core/Cargo.toml | head -1)"
TREE=packaging/out/aspic
DIST=packaging/out/dist
OUT=packaging/out/release
[ -d "$TREE" ] || { echo "!! no $TREE -- run packaging/repo/build-repo.sh first"; exit 1; }

rm -rf "$OUT"
mkdir -p "$OUT"
missing=0

# take FAMILY SRC... : copy each source file under its own name.
take() {
  local family="$1"; shift
  local n=0
  for f in "$@"; do
    [ -f "$f" ] || continue
    cp "$f" "$OUT/"
    n=$((n + 1))
  done
  printf '  %-10s %2d\n' "$family" "$n"
  [ "$n" -gt 0 ] || { echo "  !! no $family packages for $VER"; missing=1; }
}

# rename FAMILY SRC DEST : one file that needs its OS and architecture in the
# name, because the repository layout carried them in the path instead.
rename() {
  if [ -f "$2" ]; then
    cp "$2" "$OUT/$3"
    printf '  %-10s  1\n' "$1"
  else
    echo "  !! no $1 package at $2"
    missing=1
  fi
}

echo "=== release assets for $VER"
shopt -s nullglob
take deb      "$TREE"/deb/pool/*/*/*/breeze-core_"$VER"-*_*.deb "$TREE"/deb/pool/*/*/breeze-core_"$VER"-*_*.deb
take rpm      "$TREE"/rpm/*/breeze-core-"$VER"-*.rpm
# From staging, not the tree: the pacman repository serves only the
# architectures Arch has (x86_64, aarch64, armv7h), while the release has
# always carried all six. The repository signs these with a detached .sig
# and leaves the package itself untouched.
take pacman   packaging/out/pkg/breeze-core-"$VER"-*.pkg.tar.zst
take alpine   "$TREE"/alpine/*/breeze-core-"$VER"-r*.apk
take openwrt  "$TREE"/openwrt/*/breeze-core_"$VER"-*.ipk
take xbps     "$TREE"/xbps/breeze-core-"$VER"_*.xbps
take windows  "$TREE"/windows/Breeze-Core-Setup-"$VER".exe "$TREE"/windows/Breeze-Core-Setup-"$VER".exe.sha256
take opnsense "$TREE"/opnsense/os-breeze-core-"$VER".pkg
take tarballs "$DIST"/breeze-core-"$VER"-*.tar.zst "$DIST"/breeze-core-"$VER"-*.zip

# Alpine packages are named <name>-<ver>-r<rel>.apk inside the repository; the
# release has always used nfpm's own spelling, which names the architecture.
for f in "$OUT"/breeze-core-"$VER"-r*.apk; do rm -f "$f"; done
for f in "$TREE"/alpine/*/breeze-core-"$VER"-r*.apk; do
  arch="$(basename "$(dirname "$f")")"
  rel="$(basename "$f" .apk)"; rel="${rel##*-}"
  cp "$f" "$OUT/breeze-core_${VER}-${rel}_${arch}.apk"
done

rename freebsd "$TREE/freebsd/breeze-core-$VER.pkg" "breeze-core-$VER-freebsd-amd64.pkg"
rename netbsd  "$TREE/netbsd/All/breeze-core-$VER.tgz" "breeze-core-$VER-netbsd-amd64.tgz"
obsd="$(ls "$TREE"/openbsd/*/packages/*/breeze-core-"$VER".tgz 2>/dev/null | head -1 || true)"
if [ -n "$obsd" ]; then
  rel="$(echo "$obsd" | sed 's#.*/openbsd/\([^/]*\)/packages/\([^/]*\)/.*#\1-\2#')"
  rename openbsd "$obsd" "breeze-core-$VER-openbsd-$rel.tgz"
else
  echo "  !! no openbsd package"; missing=1
fi

stray="$(ls "$OUT" | grep -v -- "$VER" || true)"
if [ -n "$stray" ]; then
  echo "!! files from another version:"; echo "$stray" | sed 's/^/   /'; exit 1
fi
[ "$missing" -eq 0 ] || { echo "!! refusing: a family is missing (above)"; exit 1; }

echo "=== $(ls "$OUT" | wc -l | tr -d ' ') assets in $OUT"
