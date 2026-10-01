#!/usr/bin/env bash
# Register 32-bit MIPS (big- and little-endian) user emulation for Docker.
#
#   packaging/mips/install-mips-binfmt.sh   (prepare-qemu.sh runs it)
#
# ── Why this is needed ──────────────────────────────────────────────────────
# The usual `docker run --privileged tonistiigi/binfmt --install all` answers
# "unsupported architecture: mipsle" — that image simply ships no mips32
# emulator. It is easy to read that as "this host cannot emulate mips32", which
# is wrong: Debian qemu-user-static provides qemu-mips-static and
# qemu-mipsel-static, and registering those gives full mips32 support.
#
# This matters because 32-bit MIPS is where the actual OpenWrt routers are —
# ath79 (mips_24kc, big-endian) and ramips (mipsel_24kc, little-endian) — and
# without a handler nothing can be run or verified for them at all.
#
# The F ("fix binary") flag is the load-bearing detail: the kernel opens the
# interpreter at registration time and keeps that file, so the registration
# survives this throwaway container exiting. Without F, the path would dangle the
# moment the container is gone and every exec would fail with ENOENT.
set -euo pipefail

echo "==> registering qemu-mips / qemu-mipsel handlers via binfmt_misc" >&2
MSYS_NO_PATHCONV=1 docker run --rm --privileged debian:bookworm-slim bash -c '
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq --no-install-recommends qemu-user-static binfmt-support >/dev/null 2>&1

mountpoint -q /proc/sys/fs/binfmt_misc || mount binfmt_misc -t binfmt_misc /proc/sys/fs/binfmt_misc

for a in mips mipsel; do
  # Idempotent: re-registering an existing name returns EEXIST and aborts the
  # script under set -e, so clear it first if present.
  [ -e "/proc/sys/fs/binfmt_misc/qemu-$a" ] && \
    echo -1 > "/proc/sys/fs/binfmt_misc/qemu-$a" 2>/dev/null || true
done

# Magic/mask taken from the specs Debian ships in /usr/share/binfmts, rather
# than hand-written: e_machine 8 (MIPS) with the endianness byte distinguishing
# the two, which is exactly the pair that is easy to get subtly wrong by hand.
for a in mips mipsel; do
  spec="/usr/share/binfmts/qemu-$a"
  [ -f "$spec" ] || { echo "no spec for qemu-$a"; exit 1; }
  magic="$(sed -n "s/^magic //p" "$spec")"
  mask="$(sed -n "s/^mask //p" "$spec")"
  printf ":qemu-%s:M::%s:%s:/usr/bin/qemu-%s-static:OCF\n" "$a" "$magic" "$mask" "$a" \
    > /proc/sys/fs/binfmt_misc/register
  echo "registered qemu-$a"
done

echo "--- active MIPS handlers ---"
for f in /proc/sys/fs/binfmt_misc/qemu-mips /proc/sys/fs/binfmt_misc/qemu-mipsel; do
  [ -e "$f" ] && echo "  $(basename "$f"): $(head -1 "$f")"
done
'
