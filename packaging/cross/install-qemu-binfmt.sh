#!/usr/bin/env bash
# Register QEMU user-mode handlers in Docker's VM for architectures
# tonistiigi/binfmt does not cover, so a foreign static binary can be run in any
# container on this machine.
#
#   packaging/cross/install-qemu-binfmt.sh ppc64 loongarch64
#
# The names are QEMU's (ppc64 = big-endian POWER, loongarch64, mips, ...).
# Like packaging/mips/install-mips-binfmt.sh, which this generalises, it does
# not survive Docker Desktop restarting its VM: re-run it before a verify, or a
# container fails with a bare "exec format error" that looks like a broken image.
#
# The F flag makes the kernel open the interpreter when it is registered, so it
# works inside every container afterwards without the container having QEMU.
# Magic and mask come from the spec files Debian ships, never typed by hand: the
# endianness byte is exactly what is easy to get subtly wrong.
set -euo pipefail
[ $# -gt 0 ] || { echo "usage: $0 QEMU-ARCH..." >&2; exit 2; }

echo "==> registering qemu handlers: $*" >&2
MSYS_NO_PATHCONV=1 docker run --rm --privileged debian:13 bash -c '
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq --no-install-recommends qemu-user-static binfmt-support >/dev/null 2>&1
mountpoint -q /proc/sys/fs/binfmt_misc || mount binfmt_misc -t binfmt_misc /proc/sys/fs/binfmt_misc
for a in "$@"; do
  # Debian 13 ships systemd binfmt.d files, already in the kernel register
  # format (:name:type:offset:magic:mask:interpreter:flags; a colon inside the
  # magic is escaped as \x3a, so splitting on ":" is safe).
  spec="/usr/lib/binfmt.d/qemu-$a.conf"
  [ -f "$spec" ] || { echo "no binfmt spec for qemu-$a"; exit 1; }
  # Idempotent: an existing name makes the register write fail.
  [ -e "/proc/sys/fs/binfmt_misc/qemu-$a" ] && echo -1 > "/proc/sys/fs/binfmt_misc/qemu-$a"
  magic="$(grep -m1 "^:" "$spec" | cut -d: -f5)"
  mask="$(grep -m1 "^:" "$spec" | cut -d: -f6)"
  printf ":qemu-%s:M::%s:%s:/usr/bin/qemu-%s-static:OCF\n" "$a" "$magic" "$mask" "$a" \
    > /proc/sys/fs/binfmt_misc/register
  echo "  qemu-$a: $(head -1 "/proc/sys/fs/binfmt_misc/qemu-$a")"
done
' _ "$@"
