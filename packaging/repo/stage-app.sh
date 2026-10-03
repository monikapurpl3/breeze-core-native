#!/usr/bin/env bash
# Mirror the Breeze Android app's latest release into an aspic tree.
#
#   ./packaging/repo/stage-app.sh [packaging/out/aspic]
#
# build-repo.sh calls this, so every aspic build carries the app. After an app
# release on its own, run it against the last built tree and publish that:
#
#   ./packaging/repo/stage-app.sh && ./site/publish.sh --tree packaging/out/aspic
#
# The app is built and signed in its own repository, and its GitHub release is
# the source of truth. This fetches the latest one (never a prerelease: those do
# not belong on aspic), checks it against the checksum released beside it and
# against the app's signing certificate, and stages
#
#   android/Breeze-<ver>.apk  + .sha256
#   breeze/index.html          site/breeze/index.html, version and size filled in
#
# No breeze-latest.apk, unlike bolero. The vhost serves every *.apk as immutable
# for a year, which is right for packages that never change under a name, and a
# moving name would be pinned in caches at whatever it first was. The page is the
# stable address: it is never cached, and it names the current file.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
OUT="${1:-packaging/out/aspic}"
APP_REPO="${BREEZE_APP_REPO:-monikapurpl3/breeze}"
CACHE="packaging/out/android"
# SHA-256 of the app's release signing certificate, the same for every release
# since 1.0.1. A release signed with anything else is not ours, whatever its
# checksum file says: both would have come from the same place.
APP_CERT="e4ab9cb5c737530b74a1aabea04707f2519a5ee1fd1d110655e1e89896d6949f"

echo "=== Breeze for Android ==="
[ -f "$OUT/index.html" ] || { echo "  !! $OUT is not a built aspic tree"; exit 1; }

# BREEZE_APP_TAG pins a release instead, e.g. to rebuild an older tree.
tag="${BREEZE_APP_TAG:-$(curl -fsSL "https://api.github.com/repos/$APP_REPO/releases/latest" |
  grep -o '"tag_name": *"[^"]*"' | head -1 | sed 's/.*"\([^"]*\)"$/\1/')}"
ver="${tag#v}"
printf '%s\n' "$ver" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$' ||
  { echo "  !! the latest release is '$tag', not a plain x.y.z"; exit 1; }
apk="Breeze-$ver.apk"

mkdir -p "$CACHE"
for f in "$apk" "$apk.sha256"; do
  if [ ! -s "$CACHE/$f" ]; then
    curl -fsSL -o "$CACHE/$f.part" "https://github.com/$APP_REPO/releases/download/$tag/$f"
    mv "$CACHE/$f.part" "$CACHE/$f"
  fi
done
( cd "$CACHE" && sha256sum -c --quiet "$apk.sha256" ) ||
  { echo "  !! $apk does not match its released checksum"; rm -f "$CACHE/$apk"; exit 1; }

# The certificate, with apksigner from the Android SDK when there is one.
signer=""
for sdk in "${ANDROID_HOME:-}" "${ANDROID_SDK_ROOT:-}" "$HOME/Android/Sdk" "${LOCALAPPDATA:-}/Android/Sdk"; do
  [ -n "$sdk" ] || continue
  for c in "$sdk"/build-tools/*/apksigner.bat "$sdk"/build-tools/*/apksigner; do
    [ -e "$c" ] && signer="$c"
  done
done
if [ -n "$signer" ]; then
  got="$("$signer" verify --print-certs "$CACHE/$apk" 2>/dev/null |
    sed -n 's/.*certificate SHA-256 digest: *\([0-9a-f]*\).*/\1/p' | head -1)"
  [ "$got" = "$APP_CERT" ] ||
    { echo "  !! $apk is signed by ${got:-nothing apksigner accepts}, not the app's key"; exit 1; }
  echo "  signed by the app's key"
else
  # Loud rather than fatal: the checksum still holds, and a build machine
  # without the Android SDK should not be unable to publish the server.
  echo "  !! no apksigner found, so the signing certificate was NOT checked"
fi

rm -rf "$OUT/android"
mkdir -p "$OUT/android" "$OUT/breeze"
cp "$CACHE/$apk" "$CACHE/$apk.sha256" "$OUT/android/"
chmod 644 "$OUT/android/"*
mb=$(( ($(wc -c < "$CACHE/$apk") + 524288) / 1048576 ))
sed -e "s/@APP_VER@/$ver/g" -e "s/@APP_MB@/$mb/g" site/breeze/index.html > "$OUT/breeze/index.html"
echo "  $apk ($mb MB), /breeze/ written for $ver"
