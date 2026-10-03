#!/usr/bin/env bash
# Build the Termux packages: one Android executable per architecture, linked by
# the Android NDK, each wrapped in a Termux .deb.
#
#   ./packaging/termux/build-packages.sh                 # aarch64 arm x86_64
#   ./packaging/termux/build-packages.sh aarch64
#   BC_RELEASE=2 ./packaging/termux/build-packages.sh
#
# Output: packaging/out/termux/breeze-core_<ver>-<rel>_<arch>.deb, with Termux's
# own architecture names.
#
# **A real Android binary, not the static musl one.** The arm64 musl build does
# run on a phone -- the kernel is Linux -- but it looks for the timezone in
# /etc/localtime, which Android does not have, and so believes it is in UTC.
# Schedules and timers are server-local, so every program would fire hours out.
# Built for Android, chrono reads the phone's zone (persist.sys.timezone) and
# Android's own tzdata, and name resolution goes through bionic like any app's.
#
# Android is a tier 2 Rust target, so this needs stable Rust and the targets
# added -- no nightly, no build-std -- plus an NDK for the linker and for ring's
# C. Zig cannot stand in: it ships no bionic.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
# Files read off disk must have the line endings .gitattributes pins (see it).
bash packaging/check-eol.sh
VERSION="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
RELEASE="${BC_RELEASE:-1}"
OUT="packaging/out/termux"
BIN="packaging/out/bin"
# Termux's floor is Android 7.0, and API 24 is what its own packages target.
API=24

# label (Termux's architecture name) | rust target | NDK clang prefix
#
# The clang prefix is not the Rust target for 32-bit ARM: the NDK calls it
# armv7a-linux-androideabi, Rust armv7-linux-androideabi.
TARGETS="
aarch64|aarch64-linux-android|aarch64-linux-android
arm|armv7-linux-androideabi|armv7a-linux-androideabi
x86_64|x86_64-linux-android|x86_64-linux-android
"

want=("$@")
selected() {
  [ ${#want[@]} -eq 0 ] && return 0
  for w in "${want[@]}"; do [ "$w" = "$1" ] && return 0; done
  return 1
}

# --- the NDK ----------------------------------------------------------------
# ANDROID_NDK_HOME if set, else the newest NDK in the usual SDK locations.
find_ndk() {
  if [ -n "${ANDROID_NDK_HOME:-}" ]; then echo "$ANDROID_NDK_HOME"; return; fi
  local sdk
  for sdk in "${ANDROID_HOME:-}" "${ANDROID_SDK_ROOT:-}" "$HOME/Android/Sdk" \
             "${LOCALAPPDATA:-/nonexistent}/Android/Sdk" "$HOME/Library/Android/sdk"; do
    [ -n "$sdk" ] && [ -d "$sdk/ndk" ] || continue
    ls -d "$sdk"/ndk/*/ 2>/dev/null | sort -V | tail -1 | sed 's#/$##'
    return
  done
}
NDK="$(find_ndk)"
[ -n "$NDK" ] && [ -d "$NDK/toolchains/llvm/prebuilt" ] || {
  echo "no Android NDK found: set ANDROID_NDK_HOME"; exit 1; }
HOST_TAG="$(ls "$NDK/toolchains/llvm/prebuilt" | head -1)"
TOOLS="$NDK/toolchains/llvm/prebuilt/$HOST_TAG/bin"
# On Windows the NDK's clang is a .cmd wrapper, and cargo is a Windows program
# that wants a Windows path to it.
case "$HOST_TAG" in
  windows-*) EXT=.cmd; AREXT=.exe; winpath() { cygpath -w "$1"; } ;;
  *)         EXT="";   AREXT="";   winpath() { printf '%s' "$1"; } ;;
esac
echo "breeze-core $VERSION-$RELEASE for Termux, NDK $(basename "$NDK") ($HOST_TAG), API $API"

if git rev-parse --short HEAD >/dev/null 2>&1; then
  BREEZE_COMMIT="$(git rev-parse --short HEAD)"
  git diff --quiet HEAD 2>/dev/null || BREEZE_COMMIT="$BREEZE_COMMIT-dirty"
  export BREEZE_COMMIT
fi

# --- binaries ---------------------------------------------------------------
built=()
while IFS='|' read -r label target clang; do
  [ -n "$label" ] || continue
  selected "$label" || continue
  printf '\n=== %s (%s)\n' "$label" "$target"
  rustup +stable target list --installed | grep -qx "$target" || {
    echo "!! rust target $target missing: rustup target add --toolchain stable $target"
    exit 1; }

  cc="$(winpath "$TOOLS/${clang}${API}-clang$EXT")"
  ar="$(winpath "$TOOLS/llvm-ar$AREXT")"
  under="$(printf '%s' "$target" | tr '-' '_')"
  upper="$(printf '%s' "$under" | tr '[:lower:]' '[:upper:]')"
  # The linker for rustc, and the C compiler and archiver for ring's build
  # script -- cc-rs looks these up by target with underscores.
  env "CARGO_TARGET_${upper}_LINKER=$cc" "CC_${under}=$cc" "AR_${under}=$ar" \
    cargo +stable build --release --locked --target "$target" -p breeze-core

  mkdir -p "$BIN/termux-$label"
  cp "target/$target/release/breeze-core" "$BIN/termux-$label/breeze-core"
  built+=("$label")
done <<< "$TARGETS"
[ ${#built[@]} -gt 0 ] || { echo "nothing selected"; exit 1; }

# --- packages ---------------------------------------------------------------
docker build -q -t bc-nfpm packaging/nfpm >/dev/null
MOUNT="$REPO"
case "$MOUNT" in /[a-z]/*) MOUNT="$(echo "$MOUNT" | sed -E 's#^/([a-z])/#\U\1:/#')" ;; esac
export MSYS_NO_PATHCONV=1
mkdir -p "$OUT"

for label in "${built[@]}"; do
  # Streamed out as a tar rather than written through a mount, for the reason
  # packaging/nfpm/build-packages.sh gives: Docker Desktop here does not
  # reliably see a directory the host has just created.
  docker run --rm -v "$MOUNT:/work:ro" \
    -e BC_VERSION="$VERSION" -e BC_RELEASE="$RELEASE" -e BC_ARCH="$label" \
    -w /work bc-nfpm sh -eu -c "
      exec 3>&1 1>&2
      mkdir -p /stage /tmp/out
      install -m 0755 '/work/$BIN/termux-$label/breeze-core' /stage/breeze-core
      sed 's/@TERMUX_ARCH@/$label/' packaging/termux/nfpm.yaml > /tmp/nfpm.yaml
      nfpm package -f /tmp/nfpm.yaml -p deb -t /tmp/out
      tar -cf - -C /tmp/out . >&3
    " < /dev/null | tar -xf - -C "$OUT"
done

# Repack with dpkg-deb, xz in BOTH members, which is how Termux's own packages
# are built. nfpm compresses data.tar as told but always gzips control.tar, and
# apt on 32-bit arm Termux rejected exactly that member -- "Corrupted archive",
# "Could not read meta data" -- while installing the same files repacked
# like this. nfpm still decides the layout, metadata and scripts; this only
# changes the compression of what it made.
debs=()
for label in "${built[@]}"; do debs+=("breeze-core_${VERSION}-${RELEASE}_${label}.deb"); done
tar -cf - -C "$OUT" "${debs[@]}" \
  | docker run --rm -i debian:bookworm-slim sh -euc '
      exec 3>&1 1>&2
      mkdir /in /out
      tar -xf - -C /in
      for f in /in/*.deb; do
        rm -rf /x
        dpkg-deb -R "$f" /x
        # Debian'"'"'s dpkg calls x86_64 an invalid architecture name (no "_"
        # allowed) and builds the package anyway; Termux'"'"'s dpkg is patched
        # to accept it, and it is the name Termux uses. That one warning is
        # dropped so that any other stays visible.
        dpkg-deb -Zxz --root-owner-group -b /x "/out/${f##*/}" >/dev/null 2>/tmp/warn
        grep -v -e "is not a valid architecture name" -e "parsing file" \
                -e "ignoring 1 warning" /tmp/warn >&2 || true
      done
      tar -cf - -C /out . >&3
    ' \
  | tar -xf - -C "$OUT"

echo
ls -l "$OUT"/breeze-core_"$VERSION"-"$RELEASE"_*.deb
