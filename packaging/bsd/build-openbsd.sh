#!/usr/bin/env bash
# Build the OpenBSD binary and a signify-signed package - on a real OpenBSD
# machine.
#
#   ./packaging/bsd/build-openbsd.sh [user@host]
#
# The Python line never shipped an OpenBSD package at all: a virtualenv full of
# absolute paths is not something you can hand to pkg_add. One static binary is,
# and it builds natively here with TLS and everything else included.
#
# Output, fetched back here:
#   packaging/out/bin/openbsd-amd64/breeze-core
#   packaging/out/bsd/openbsd/breeze-core-<ver>.tgz   (signed)
#   packaging/out/dist/breeze-core-<ver>-openbsd-amd64.tar.zst
#
# **The signing key visits that machine.** signify signs locally, so the secret
# key is copied over as aspic-pkg.sec, used, and then removed with rm -P. The
# filename is not arbitrary: pkg_sign records it in the signature and pkg_add
# then looks for /etc/signify/aspic-pkg.pub, so renaming the key renames what
# every client must install.
#
# Builds happen in ~ on that box, which is /home: OpenBSD's default / is under a
# gigabyte and was 99% full, and a rust build does not fit there.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
TARGET="${1:-monika@192.168.122.128}"
VER="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
SEC="packaging/repo/keys/aspic-openbsd.sec"
WORK="bcn-build"

[ -f "$SEC" ] || { echo "no $SEC - generate it with: signify -G -n -p aspic-pkg.pub -s aspic-pkg.sec"; exit 1; }

COMMIT="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
git diff --quiet HEAD 2>/dev/null || COMMIT="$COMMIT-dirty"

echo "=== $TARGET: $(ssh "$TARGET" 'uname -srm')"

# The release and the architecture come from the machine that built it, because
# that is what the package is valid for: OpenBSD bumps its libc every release
# and pkg_add expands PKG_PATH's %c/%a into exactly this layout. Reading them
# here rather than hardcoding 7.9/amd64 means the next release lands beside this
# one instead of on top of it.
REL="$(ssh "$TARGET" 'uname -r')"
ARCH="$(ssh "$TARGET" 'uname -m')"
PKGDIR="packaging/out/bsd/openbsd/$REL/packages/$ARCH"

echo "=== sending the source"
tar --exclude='./target' --exclude='./packaging/out' --exclude='./.git' -czf /tmp/bcn-src.tgz .
ssh "$TARGET" "rm -rf ~/$WORK && mkdir -p ~/$WORK"
scp -q /tmp/bcn-src.tgz "$TARGET:~/$WORK/"
ssh "$TARGET" "cd ~/$WORK && tar -xzf bcn-src.tgz && rm bcn-src.tgz"

echo "=== building"
# ulimit -d: OpenBSD caps a process's data segment at 1.5 GB for the default
# login class, and rustc linking with fat LTO can want more than that.
ssh "$TARGET" "cd ~/$WORK && ulimit -d 4194304 2>/dev/null || true; CARGO_HOME=\$HOME/$WORK/.cargo BREEZE_COMMIT=$COMMIT cargo build --release 2>&1 | tail -3"
ssh "$TARGET" "~/$WORK/target/release/breeze-core --version"

echo "=== packaging and signing"
scp -q "$SEC" "$TARGET:~/$WORK/aspic-pkg.sec"
ssh "$TARGET" "
  set -e
  chmod 600 ~/$WORK/aspic-pkg.sec
  cd ~/$WORK
  sh packaging/bsd/mkpkg-openbsd.sh ~/$WORK/out ~/$WORK/aspic-pkg.sec 2>&1 | tail -6
  rm -P ~/$WORK/aspic-pkg.sec
  echo '  key destroyed on the target:'
  ls ~/$WORK/aspic-pkg.sec 2>/dev/null && echo '  !! STILL THERE' || echo '  gone'
"

echo "=== fetching the artifacts back"
mkdir -p packaging/out/bin/openbsd-amd64 "$PKGDIR" packaging/out/dist
scp -q "$TARGET:~/$WORK/out/*.tgz" "$PKGDIR/"
scp -q "$TARGET:~/$WORK/target/release/breeze-core" packaging/out/bin/openbsd-amd64/breeze-core
tar --zstd -cf "packaging/out/dist/breeze-core-$VER-openbsd-amd64.tar.zst" \
    -C packaging/out/bin/openbsd-amd64 breeze-core -C "$REPO" LICENSE README.md

# The public half travels with the package it verifies.
cp packaging/repo/keys/aspic-openbsd.pub packaging/out/bsd/openbsd/aspic-pkg.pub
chmod 644 packaging/out/bsd/openbsd/aspic-pkg.pub

echo
find packaging/out/bsd/openbsd -type f | sed "s|^|  |"
echo
echo "verify it on the machine with:  ./packaging/bsd/verify-openbsd.sh $TARGET"
