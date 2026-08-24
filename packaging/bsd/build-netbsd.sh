#!/usr/bin/env bash
# Build the NetBSD binary, package and pkgin feed — on a real NetBSD machine.
#
#   ./packaging/bsd/build-netbsd.sh [user@host]
#
# There is no cross-build for this: Zig bundles no NetBSD libc, so the only
# honest way to produce a NetBSD binary is to compile on NetBSD. The machine
# needs `cargo` (pkgin install rust) and doas.
#
# Output, fetched back here:
#   packaging/out/bin/netbsd-amd64/breeze-core
#   packaging/out/bsd/netbsd/All/{breeze-core-<ver>.tgz, pkg_summary.gz}
#   packaging/out/dist/breeze-core-<ver>-netbsd-amd64.tar.zst
#
# The whole toolchain is NetBSD's own: cargo from pkgsrc, pkg_create(1) from the
# base system, pkg_info -X for the pkgin summary. Nothing here is emulated and
# nothing is cross-compiled, which is also why it is worth automating — doing it
# by hand is a dozen ssh round trips.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
TARGET="${1:-monika@192.168.122.130}"
VER="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
WORK="bcn-build"

COMMIT="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
git diff --quiet HEAD 2>/dev/null || COMMIT="$COMMIT-dirty"

echo "=== $TARGET: $(ssh "$TARGET" 'uname -srm')"

echo "=== sending the source"
# No target/, no packaging/out, no .git — a few hundred KB.
tar --exclude='./target' --exclude='./packaging/out' --exclude='./.git' -czf /tmp/bcn-src.tgz .
ssh "$TARGET" "rm -rf ~/$WORK && mkdir -p ~/$WORK"
scp -q /tmp/bcn-src.tgz "$TARGET:~/$WORK/"
ssh "$TARGET" "cd ~/$WORK && tar -xzf bcn-src.tgz && rm bcn-src.tgz"

echo "=== building"
# CARGO_HOME inside the work directory on purpose: ~/.cargo on that machine is
# root-owned (pkgsrc built rust as root), so a normal user's build dies on
# "Permission denied" while downloading the first crate.
ssh "$TARGET" "cd ~/$WORK && CARGO_HOME=\$HOME/$WORK/.cargo BREEZE_COMMIT=$COMMIT cargo build --release 2>&1 | tail -3"
ssh "$TARGET" "~/$WORK/target/release/breeze-core --version"

echo "=== packaging"
# PATH: pkg_create and pkg_info live in /usr/pkg/sbin and /usr/sbin, neither of
# which is in a NetBSD login PATH.
ssh "$TARGET" "cd ~/$WORK && PATH=\$PATH:/usr/pkg/sbin:/usr/sbin doas sh packaging/bsd/mkpkg-netbsd.sh ~/$WORK/out 2>&1 | tail -2"
# mkpkg runs as root, so the output belongs to root until told otherwise.
ssh "$TARGET" "doas chown -R \$(id -un) ~/$WORK/out"

echo "=== pkgin catalogue"
ssh "$TARGET" "set -e
  cd ~/$WORK/out
  mkdir -p All && cp breeze-core-$VER.tgz All/
  cd All && PATH=\$PATH:/usr/pkg/sbin:/usr/sbin pkg_info -X breeze-core-$VER.tgz > pkg_summary
  gzip -9kf pkg_summary
  ls -la"

echo "=== fetching the artifacts back"
mkdir -p packaging/out/bin/netbsd-amd64 packaging/out/bsd/netbsd/All packaging/out/dist
scp -q "$TARGET:~/$WORK/out/All/*" packaging/out/bsd/netbsd/All/
scp -q "$TARGET:~/$WORK/target/release/breeze-core" packaging/out/bin/netbsd-amd64/breeze-core
tar --zstd -cf "packaging/out/dist/breeze-core-$VER-netbsd-amd64.tar.zst" \
    -C packaging/out/bin/netbsd-amd64 breeze-core -C "$REPO" LICENSE README.md

echo
ls -l packaging/out/bsd/netbsd/All/ packaging/out/bin/netbsd-amd64/
echo
echo "verify it on the machine with:  ./packaging/bsd/verify-netbsd.sh $TARGET"
