#!/usr/bin/env bash
# Build the OpenWrt apk packages, for OpenWrt 25.12 and later.
#
#   ./packaging/openwrt-apk/build-apk.sh
#   BC_RELEASE=2 ./packaging/openwrt-apk/build-apk.sh
#
# Output: packaging/out/openwrt-apk/<openwrt arch>/breeze-core-<ver>-r<rel>.apk
#
# **OpenWrt 25.12 replaced opkg with apk-tools 3**, and its packages are apk
# v3: the ADB format, which is neither the .ipk the opkg feed carries nor the
# apk v2 that Alpine's repository uses, and which nfpm cannot write. So these
# are made by apk-tools 3 itself (`apk mkpkg`), from Alpine 3.24, whose apk is
# 3.0.8 -- OpenWrt's own apk is built without mkpkg and mkndx. The opkg feed
# stays as it is, for 24.10 and older.
#
# The contents are the .ipk's, from the same binaries: the executable, the
# procd init script, the settings file, and the four maintainer scripts. The
# index is built and signed in packaging/repo/build-repo.sh.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
VERSION="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
RELEASE="${BC_RELEASE:-1}"
BIN="packaging/out/bin"
OUT="packaging/out/openwrt-apk"

# OpenWrt architecture | binary label (packaging/out/bin/<label>)
#
# The same labels as the opkg feed: OpenWrt's package architecture is not a
# family name, so each of the three ARM64 targets is its own package holding
# the same binary.
ARCHES="
x86_64|amd64
aarch64_generic|arm64
aarch64_cortex-a53|arm64
aarch64_cortex-a72|arm64
arm_cortex-a7_neon-vfpv4|armv7
riscv64_riscv64|riscv64
mipsel_24kc|mipsel
mips_24kc|mips
"

MOUNT="$REPO"
case "$MOUNT" in /[a-z]/*) MOUNT="$(echo "$MOUNT" | sed -E 's#^/([a-z])/#\U\1:/#')" ;; esac
export MSYS_NO_PATHCONV=1

# Every binary first: a missing one is a missing package, said out loud.
while IFS='|' read -r owrt label; do
  [ -n "$owrt" ] || continue
  [ -f "$BIN/$label/breeze-core" ] || {
    echo "!! no $label binary for $owrt -- run packaging/build-binaries.sh $label (MIPS: packaging/mips/build-mips.sh $label)"
    exit 1; }
done <<< "$ARCHES"

rm -rf "$OUT"
mkdir -p "$OUT"
echo "breeze-core $VERSION-r$RELEASE for OpenWrt 25.12+ (apk v3)"

# One container for all of them. The arch table goes in as the script's first
# lines; the repository is mounted read-only and the packages stream out.
{ printf 'VERSION=%s\nRELEASE=%s\nARCHES="%s"\n' "$VERSION" "$RELEASE" "$ARCHES"
  cat <<'APK'
exec 3>&1 1>&2
mkdir -p /out
for line in $ARCHES; do
  owrt="${line%%|*}"; label="${line#*|}"
  R=/root/$owrt
  mkdir -p "$R/usr/bin" "$R/usr/lib/breeze-core" "$R/etc/init.d" "$R/etc/breeze-core" "$R/usr/share/doc/breeze-core"
  install -m 0755 "/work/packaging/out/bin/$label/breeze-core" "$R/usr/bin/breeze-core"
  ln -s /usr/bin/breeze-core "$R/usr/lib/breeze-core/breeze-core"
  install -m 0755 /work/packaging/nfpm/breeze-core.init "$R/etc/init.d/breeze-core"
  chmod 0750 "$R/etc/breeze-core"
  install -m 0640 /work/packaging/nfpm/breeze-core.env "$R/etc/breeze-core/breeze-core.env"
  install -m 0644 /work/README.md /work/LICENSE "$R/usr/share/doc/breeze-core/"
  mkdir -p "/out/$owrt"
  # apk names the release r<N>: 4.3.0-r1, as Alpine and OpenWrt both do.
  apk mkpkg \
    --info "name:breeze-core" \
    --info "version:$VERSION-r$RELEASE" \
    --info "arch:$owrt" \
    --info "description:Self-hosted, LAN-first control for Midea air conditioners" \
    --info "license:AGPL-3.0-or-later" \
    --info "origin:breeze-core" \
    --info "url:https://github.com/monikapurpl3/breeze-core-native" \
    --info "maintainer:monikapurpl3 <monikapurpl3@users.noreply.github.com>" \
    --script "pre-install:/work/packaging/nfpm/scripts/preinstall.sh" \
    --script "post-install:/work/packaging/nfpm/scripts/postinstall.sh" \
    --script "pre-deinstall:/work/packaging/nfpm/scripts/preremove.sh" \
    --script "post-deinstall:/work/packaging/nfpm/scripts/postremove.sh" \
    --files "$R" \
    --output "/out/$owrt/breeze-core-$VERSION-r$RELEASE.apk"
  printf '  %-26s %s\n' "$owrt" "$(du -k "/out/$owrt/breeze-core-$VERSION-r$RELEASE.apk" | cut -f1) KB"
done
tar -cf - -C /out . >&3
APK
} | docker run --rm -i -v "$MOUNT:/work:ro" alpine:3.24 sh -eu -s | tar -xf - -C "$OUT"

echo
find "$OUT" -name '*.apk' | sort | sed 's/^/  /'
