#!/usr/bin/env bash
# Build the SOURCE packages: an SRPM for rpmbuild, a Debian source package for
# dpkg-buildpackage, and a makepkg source tarball for Arch.
#
#   ./packaging/source/build-source.sh
#   BC_RELEASE=2 ./packaging/source/build-source.sh
#
# Output, in packaging/out/source/:
#
#   breeze-core-<ver>.tar.xz           git archive of the commit
#   breeze-core-<ver>-vendor.tar.xz    every crate it needs, from `cargo vendor`
#   rpm/breeze-core-<ver>-<rel>.src.rpm
#   deb/breeze-core_<ver>-<rel>.dsc + .debian.tar.xz + the two .orig tarballs
#   arch/breeze-core-<ver>-<rel>.src.tar.gz    (makepkg --allsource)
#
# All three build the same package as the binary repositories -- the same name,
# files, service account and scriptlets (packaging/nfpm/scripts/, pasted into
# each recipe) -- so a source-built install and a repository one replace each
# other. The binary is the difference: built by the distribution's own Rust and
# linked against its glibc, where the repository's is static musl from Zig.
#
# **The crates are vendored.** Distribution builders run offline (mock, sbuild,
# a clean Arch chroot), and a source package that fetched crates.io during the
# build would not be the source it claims to be. The vendor tarball is 12 MB of
# xz, most of it Windows crates a Linux build never compiles; trimming them
# means rewriting cargo's checksums by hand, which is not worth 7 MB.
#
# Signing happens in packaging/repo/build-repo.sh, with the repository key, like
# every binary package.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
VER="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
REL="${BC_RELEASE:-1}"
OUT="packaging/out/source"
SRC="packaging/source"
SCRIPTS="packaging/nfpm/scripts"

# A committed tree, because the tarball IS the commit: `git archive HEAD` holds
# nothing uncommitted, so building from a dirty tree would publish source that
# does not match what the packaging around it was tested with. BC_ALLOW_DIRTY=1
# is for working on this script, and says so in the commit it embeds.
COMMIT="$(git rev-parse --short HEAD)"
if ! git diff --quiet HEAD -- || [ -n "$(git ls-files --others --exclude-standard -- crates static Cargo.toml Cargo.lock packaging/nfpm)" ]; then
  if [ "${BC_ALLOW_DIRTY:-0}" = 1 ]; then
    echo "!! uncommitted changes: the source tarball is HEAD ($COMMIT) without them"
    COMMIT="$COMMIT-dirty"
  else
    echo "refusing: the tree has uncommitted changes, and the source tarball is the commit"
    echo "  commit first, or BC_ALLOW_DIRTY=1 while working on this script"
    exit 1
  fi
fi
# Every timestamp in the tarballs is the commit's, so the same commit gives the
# same bytes on any machine and on any day.
EPOCH="$(git log -1 --format=%ct HEAD)"

MOUNT="$REPO"
case "$MOUNT" in /[a-z]/*) MOUNT="$(echo "$MOUNT" | sed -E 's#^/([a-z])/#\U\1:/#')" ;; esac
export MSYS_NO_PATHCONV=1

rm -rf "$OUT"
mkdir -p "$OUT/rpm" "$OUT/deb" "$OUT/arch" "$OUT/recipes/debian/source"
echo "breeze-core $VER-$REL source packages, commit $COMMIT"

# --- 1. the two tarballs -----------------------------------------------------
# core.autocrlf=false: with it on, which is how this repository is checked out
# on Windows, git archive writes CRLF into every text file it exports.
echo "=== tarballs (source + vendored crates)"
git -c core.autocrlf=false archive --format=tar --prefix="breeze-core-$VER/" HEAD \
  | docker run --rm -i -e VER="$VER" -e EPOCH="$EPOCH" rust:1.92-slim-bookworm sh -euc '
      exec 3>&1 1>&2
      apt-get -qq update >/dev/null && apt-get -qq install -y xz-utils >/dev/null
      mkdir -p /w /out && cd /w
      cat > src.tar
      # -T1: multithreaded xz splits the stream differently on every machine.
      xz -9 -T1 -c src.tar > "/out/breeze-core-$VER.tar.xz"
      tar -xf src.tar && cd "breeze-core-$VER"
      cargo vendor --locked --quiet vendor >/dev/null
      tar --sort=name --mtime="@$EPOCH" --owner=0 --group=0 --numeric-owner \
          --format=gnu -cf - vendor | xz -9 -T1 > "/out/breeze-core-$VER-vendor.tar.xz"
      tar -cf - -C /out . >&3
    ' | tar -xf - -C "$OUT"
SHA_SRC="$(sha256sum "$OUT/breeze-core-$VER.tar.xz" | cut -d' ' -f1)"
SHA_VEN="$(sha256sum "$OUT/breeze-core-$VER-vendor.tar.xz" | cut -d' ' -f1)"

# --- 2. the recipes ----------------------------------------------------------
subst() {
  sed -e "s/@VER@/$VER/g" -e "s/@REL@/$REL/g" -e "s/@COMMIT@/$COMMIT/g" \
      -e "s/@RPMDATE@/$(date -u -d "@$EPOCH" '+%a %b %d %Y')/g" \
      -e "s/@DEBDATE@/$(date -u -R -d "@$EPOCH")/g" \
      -e "s/@SHA256_SOURCE@/$SHA_SRC/g" -e "s/@SHA256_VENDOR@/$SHA_VEN/g" "$1"
}
# paste FILE PLACEHOLDER SCRIPT: replace the placeholder line with the script.
paste_script() {
  awk -v ph="$2" -v f="$3" '$0 == ph { while ((getline l < f) > 0) print l; close(f); next } { print }' "$1"
}

# The spec: rpm expands % everywhere, comments included, so the pasted scripts
# have theirs doubled.
spec="$OUT/recipes/breeze-core.spec"
subst "$SRC/breeze-core.spec.in" > "$spec"
for pair in PREINSTALL:preinstall POSTINSTALL:postinstall PREREMOVE:preremove POSTREMOVE:postremove; do
  sed 's/%/%%/g' "$SCRIPTS/${pair#*:}.sh" > "$OUT/recipes/script"
  paste_script "$spec" "@${pair%%:*}@" "$OUT/recipes/script" > "$spec.new" && mv "$spec.new" "$spec"
done

# debian/: the maintainer scripts are the nfpm scripts as they are.
deb="$OUT/recipes/debian"
for f in control copyright source/format; do cp "$SRC/debian/$f" "$deb/$f"; done
subst "$SRC/debian/rules" > "$deb/rules" && chmod 755 "$deb/rules"
subst "$SRC/debian/changelog.in" > "$deb/changelog"
cp "$SCRIPTS/preinstall.sh"  "$deb/breeze-core.preinst"
cp "$SCRIPTS/postinstall.sh" "$deb/breeze-core.postinst"
cp "$SCRIPTS/preremove.sh"   "$deb/breeze-core.prerm"
cp "$SCRIPTS/postremove.sh"  "$deb/breeze-core.postrm"

# The PKGBUILD, and its install file in the form nfpm writes for the binary
# package: each script as the body of the function pacman calls.
subst "$SRC/PKGBUILD.in" > "$OUT/recipes/PKGBUILD"
{
  for pair in pre_install:preinstall post_install:postinstall pre_remove:preremove post_remove:postremove; do
    echo "function ${pair%%:*}() {"
    cat "$SCRIPTS/${pair#*:}.sh"
    echo "}"
    echo
  done
} > "$OUT/recipes/breeze-core.install"
rm -f "$OUT/recipes/script"

# Recipes and tarballs, as one stream for each builder below.
bundle() { tar -cf - -C "$OUT" "breeze-core-$VER.tar.xz" "breeze-core-$VER-vendor.tar.xz" recipes; }

# --- 3. SRPM -----------------------------------------------------------------
# dist is empty: this one SRPM serves Fedora, RHEL and openSUSE alike, and each
# rebuild adds its own dist tag to the binary package it makes.
echo "=== SRPM"
bundle | docker run --rm -i -e SOURCE_DATE_EPOCH="$EPOCH" almalinux:9 sh -euc '
    exec 3>&1 1>&2
    dnf -q -y install rpm-build >/dev/null
    mkdir -p /rb/SOURCES /rb/SPECS /in /out
    tar -xf - -C /in
    cp /in/*.tar.xz /rb/SOURCES/ && cp /in/recipes/breeze-core.spec /rb/SPECS/
    rpmbuild -bs --define "_topdir /rb" --define "dist %{nil}" /rb/SPECS/breeze-core.spec
    cp /rb/SRPMS/*.src.rpm /out/
    tar -cf - -C /out . >&3
  ' | tar -xf - -C "$OUT/rpm"

# --- 4. Debian source package -------------------------------------------------
# Format 3.0 (quilt) with the vendored crates as a second, "component" orig
# tarball, which dpkg-source unpacks into vendor/. Both tarballs are byte for
# byte the ones above, renamed the way dpkg-source expects.
echo "=== Debian source package"
bundle | docker run --rm -i -e VER="$VER" -e REL="$REL" debian:bookworm-slim sh -euc '
    exec 3>&1 1>&2
    apt-get -qq update >/dev/null && apt-get -qq install -y dpkg-dev xz-utils >/dev/null
    mkdir -p /w /out && cd /w
    tar -xf -
    cp "breeze-core-$VER.tar.xz"        "breeze-core_$VER.orig.tar.xz"
    cp "breeze-core-$VER-vendor.tar.xz" "breeze-core_$VER.orig-vendor.tar.xz"
    tar -xf "breeze-core_$VER.orig.tar.xz"
    tar -xf "breeze-core_$VER.orig-vendor.tar.xz" -C "breeze-core-$VER"
    cp -r recipes/debian "breeze-core-$VER/debian"
    dpkg-source -b "breeze-core-$VER"
    cp "breeze-core_$VER-$REL.dsc" "breeze-core_$VER-$REL.debian.tar.xz" \
       "breeze-core_$VER.orig.tar.xz" "breeze-core_$VER.orig-vendor.tar.xz" /out/
    tar -cf - -C /out . >&3
  ' | tar -xf - -C "$OUT/deb"

# --- 5. Arch source tarball ---------------------------------------------------
# --allsource: the PKGBUILD, its install file and both tarballs in one archive,
# so it builds with nothing else downloaded. makepkg refuses to run as root.
echo "=== Arch source tarball"
bundle | docker run --rm -i -e VER="$VER" -e REL="$REL" archlinux:base sh -euc '
    exec 3>&1 1>&2
    pacman -Sy --noconfirm --needed fakeroot >/dev/null 2>&1
    useradd -m builder
    mkdir -p /w /out
    tar -xf - -C /w
    mv /w/recipes/PKGBUILD /w/recipes/breeze-core.install /w/
    chown -R builder /w
    su builder -c "cd /w && makepkg --allsource" >/dev/null
    cp "/w/breeze-core-$VER-$REL.src.tar.gz" /out/
    tar -cf - -C /out . >&3
  ' | tar -xf - -C "$OUT/arch"

rm -rf "$OUT/recipes"
echo
( cd "$OUT" && find . -type f | sort | while read -r f; do
    printf '  %8s  %s\n' "$(du -k "$f" | cut -f1)K" "${f#./}"; done )
