#!/usr/bin/env bash
# Make 32-bit MIPS OpenWrt runnable in Docker, for the verification cases.
#
#   ./packaging/mips/prepare-qemu.sh
#
# Two things, both idempotent:
#
#  1. Register qemu-mips and qemu-mipsel with binfmt_misc
#     (install-mips-binfmt.sh). Docker Desktop's own emulators stop at 64-bit
#     and ARM; without this a MIPS container dies with a bare "exec format
#     error". **The registration does not survive Docker Desktop's VM
#     restarting**, so run this again after a restart.
#
#  2. Import OpenWrt's malta little-endian root filesystem as a local image.
#     Docker Hub's openwrt/rootfs has big-endian mips_24kc images but no
#     little-endian MIPS at all, and malta-le's package architecture is
#     mipsel_24kc -- the same packages a MediaTek router installs. The
#     tarballs are checked against OpenWrt's published SHA-256s.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
export MSYS_NO_PATHCONV=1

"$HERE/install-mips-binfmt.sh"

# release | SHA-256 of openwrt-<release>-malta-le-default-rootfs.tar.gz
ROOTFS="
25.12.5|d28b07e52b21844ae29bdea48cec8e92ac70a8b0397a0bcc488ef7169f3f194d
24.10.8|a93c86a7a08744a7e32d1dcd0f22046ff2fed28416074fe671d73963fa00bf5b
"
while IFS='|' read -r rel sum; do
  [ -n "$rel" ] || continue
  img="bc-openwrt:malta-le-$rel"
  if docker image inspect "$img" >/dev/null 2>&1; then
    echo "  $img already imported"; continue
  fi
  f="openwrt-$rel-malta-le-default-rootfs.tar.gz"
  # Downloaded from inside the cache directory, by bare name: with
  # MSYS_NO_PATHCONV set, a Windows curl is handed any absolute Git Bash path
  # (/tmp/..., /c/...) untranslated, and fails to write it with curl error 23.
  cache="$HERE/../out/openwrt-rootfs"
  mkdir -p "$cache"
  [ -f "$cache/$f" ] || ( cd "$cache" && curl -fsSLO "https://downloads.openwrt.org/releases/$rel/targets/malta/le/$f" )
  ( cd "$cache" && echo "$sum  $f" | sha256sum -c - >/dev/null ) || {
    echo "!! $f does not match its published SHA-256"; rm -f "$cache/$f"; exit 1; }
  # Streamed rather than given a path, so Docker Desktop never has to resolve
  # a Windows path to it.
  docker import --platform linux/mipsle - "$img" < "$cache/$f" >/dev/null
  echo "  imported $img"
done <<< "$ROOTFS"

# Proof both endiannesses execute, rather than trusting the registration.
printf '  mipsel: '; docker run --rm --platform linux/mipsle bc-openwrt:malta-le-25.12.5 /bin/sh -c 'apk --print-arch'
printf '  mips:   '; docker run --rm openwrt/rootfs:mips_24kc-25.12.5 /bin/sh -c 'apk --print-arch' 2>/dev/null
