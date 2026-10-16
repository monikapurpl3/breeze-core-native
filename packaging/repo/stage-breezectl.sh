#!/usr/bin/env bash
# Stage breezectl -- the terminal client -- into an aspic tree.
#
#   ./packaging/repo/stage-breezectl.sh [packaging/out/aspic]
#
# build-repo.sh calls this, so every aspic build carries it. After a breezectl
# release on its own, run it against the last built tree and publish that, as
# stage-app.sh does for the app (and only while that tree still equals what is
# live):
#
#   ./packaging/repo/stage-breezectl.sh && ./site/publish.sh --tree packaging/out/aspic
#
# The files come from the breezectl checkout (BREEZECTL_DIR, by default beside
# this one) through its own stage.sh, which refuses a working tree that differs
# from a commit, so what is published is always one. Staged:
#
#   breezectl/  breezectl  breezectl.ps1  install.sh  install.ps1  LICENSE
#               completions/  SHA256SUMS
#   breezectl/index.html   site/breezectl/index.html, version filled in
#
# The vhost serves everything under /breezectl/ as UTF-8 text and never
# cached, so the scripts and the SHA256SUMS the installers check them against
# cannot be served out of step.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
OUT="${1:-packaging/out/aspic}"
SRC="${BREEZECTL_DIR:-$REPO/../breezectl}"

[ -f "$SRC/stage.sh" ] || { echo "stage-breezectl: no breezectl checkout at $SRC (set BREEZECTL_DIR)" >&2; exit 1; }
[ -d "$OUT" ] || { echo "stage-breezectl: $OUT is not a directory" >&2; exit 1; }

rm -rf "$OUT/breezectl"
sh "$SRC/stage.sh" "$OUT/breezectl" >/dev/null

# The two clients are one release: refuse a pair whose versions differ.
ver=$(sed -n 's/^VERSION=\([0-9][0-9.]*\)$/\1/p' "$OUT/breezectl/breezectl" | head -n 1)
psver=$(tr -d '\r' <"$OUT/breezectl/breezectl.ps1" | sed -n "s/^\$script:Version = '\([0-9.]*\)'$/\1/p" | head -n 1)
if [ -z "$ver" ] || [ "$ver" != "$psver" ]; then
  echo "stage-breezectl: the sh client says '$ver' and the PowerShell one '$psver'" >&2
  exit 1
fi

sed "s/@CTL_VER@/$ver/g" site/breezectl/index.html >"$OUT/breezectl/index.html"
chmod 644 "$OUT/breezectl/index.html"

# Copied, then checked again where it will be published from.
( cd "$OUT/breezectl" && sha256sum -c --quiet SHA256SUMS ) ||
  { echo "stage-breezectl: the staged files do not match their SHA256SUMS" >&2; exit 1; }
echo "  breezectl $ver: $(grep -c . "$OUT/breezectl/SHA256SUMS") files, checked, and its page"
