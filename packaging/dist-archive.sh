#!/usr/bin/env bash
# One release archive: the executable, the licence and the README, as zstd.
#
#   packaging/dist-archive.sh OUT.tar.zst DIR-HOLDING-breeze-core
#
# Every build script that makes a `breeze-core-<ver>-<os>-<arch>.tar.zst` calls
# this, so all of them come out the same: breeze-core 0755, LICENSE and
# README.md 0644, everything owned by root.
#
# The modes are set here rather than read from the files, because on the
# workstation the files are on NTFS, where Git Bash's tar records no execute
# bit. So every archive before 4.3.0 held a breeze-core that would not run
# until it was chmod'ed. GNU tar's --mode applies to a whole invocation, hence
# one tar call for the binary and an append for the rest. And --force-local,
# because a path with a drive letter is otherwise taken for host:file.
set -euo pipefail
[ $# -eq 2 ] || { echo "usage: $0 OUT.tar.zst DIR-HOLDING-breeze-core" >&2; exit 2; }
out="$1"; bindir="$2"
repo="$(cd "$(dirname "$0")/.." && pwd)"
[ -f "$bindir/breeze-core" ] || { echo "no $bindir/breeze-core" >&2; exit 1; }

own=(--force-local --owner=0 --group=0 --numeric-owner)
tmp="${out%.zst}.partial"
rm -f "$tmp"
tar -cf "$tmp" "${own[@]}" --mode=0755 -C "$bindir" breeze-core
tar -rf "$tmp" "${own[@]}" --mode=0644 -C "$repo" LICENSE README.md
zstd -q -f --rm "$tmp" -o "$out"

# The point of this file, checked rather than assumed. Listed into a variable
# first: `tar | grep -q` fails under pipefail when grep stops reading early.
listing="$(tar --force-local --numeric-owner --zstd -tvf "$out")"
grep -Eq '^-rwxr-xr-x +0/0 .* breeze-core$' <<< "$listing" \
  || { echo "$out: breeze-core is not 0755 root:root inside the archive" >&2; exit 1; }
