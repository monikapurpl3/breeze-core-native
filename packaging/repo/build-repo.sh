#!/usr/bin/env bash
# Build the static, SIGNED aspic repository tree from packaging/out/pkg/*.
#
#   ./packaging/repo/build-repo.sh
#
# Everything runs in containers; the signing keys live in packaging/repo/keys/
# (git-ignored — BACK THEM UP) and are generated on first run. The output is a
# plain static tree, so the web host never holds a private key: a compromised
# host can serve stale or missing files but cannot forge a package any of these
# clients will accept.
#
#   packaging/out/aspic/
#   ├── index.html  aspic.css  favicon.svg      the repository's own page
#   ├── breeze-core/index.html                  one page per project
#   ├── aspic.asc  aspic-alpine.rsa.pub  aspic-usign.pub
#   ├── deb/     dists/stable/… + pool/         (apt,  GPG InRelease)
#   ├── rpm/     <arch>/repodata/ + aspic.repo  (dnf/zypper, signed rpms)
#   ├── arch/    <arch>/aspic.db…               (pacman, signed db + packages)
#   ├── alpine/  <arch>/APKINDEX.tar.gz         (apk, RSA-signed index)
#   └── openwrt/ <arch>/Packages + Packages.sig (opkg, usign)
#
# **One repository per package-manager family, at the root — not one per
# project.** Somebody who has added aspic gets everything published here, and a
# second project needs no second sources entry. Which is also why there is one
# host key rather than a key per project: trust is a thing you ask a person for
# once. Projects are distinguished by package name and by their own page.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
VER="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
PKG="packaging/out/pkg"
OUT="packaging/out/aspic"
KEYS="packaging/repo/keys"
GPG_NAME="Aspic Repository"
GPG_EMAIL="repo@aspic.salataputarica.hr.eu.org"
APK_KEY="aspic-alpine.rsa"

MOUNT="$REPO"
case "$MOUNT" in /[a-z]/*) MOUNT="$(echo "$MOUNT" | sed -E 's#^/([a-z])/#\U\1:/#')" ;; esac
export MSYS_NO_PATHCONV=1

# Run a stage and stream its output back as a tar.
#
# The stage writes into /out inside the container and nothing is bind-mounted
# writable, for two reasons. Docker Desktop on this machine does not always
# propagate a directory the host created moments earlier — the first run of this
# script died on `mkdir: cannot create directory '/out/deb'` for a parent that
# demonstrably existed — and rpmsign and repo-add both replace files by renaming
# them, which fails on a Windows bind mount anyway.
#
# The script arrives on stdin and the tar leaves on stdout, so fd 3 carries the
# archive while the stage's own chatter goes to stderr where a person can see it.
stage() {
  local img="$1" script="$2"
  mkdir -p "$OUT"
  printf '%s' "$script" \
    | docker run --rm -i -v "$MOUNT:/work:ro" -e VER="$VER" "$img" sh -euc '
        exec 3>&1 1>&2
        mkdir -p /out
        sh -eu -s
        tar -cf - -C /out . >&3
      ' \
    | tar -xf - -C "$OUT"
}

[ -e "$PKG/breeze-core_${VER}_amd64.deb" ] || {
  echo "no packages for $VER — run packaging/nfpm/build-packages.sh first"; exit 1; }

# Refuse to build a repository out of two versions at once.
#
# Every copy below globs on extension and architecture, never on version --
# `cp .../pkg/*.deb`, `*."$a".rpm`, and so on. build-packages.sh deliberately
# does NOT clear its output directory, because it takes an architecture list and
# clearing it would delete the architectures this run is not building. The two
# behaviours combine badly: after building 4.0.1 and later 4.0.2, out/pkg holds
# both and the repository silently ships both, with a package count that
# disagrees with the release page. Clients would still resolve the newer one, so
# nothing would look wrong until somebody counted.
#
# The version is matched delimited by - or _ so that 4.0.10 is not mistaken for
# a 4.0.1 package.
vre="[-_]$(printf '%s' "$VER" | sed 's/\./\\./g')[-_]"
stray="$(ls -1 "$PKG" 2>/dev/null | grep -Ev -- "$vre" || true)"
if [ -n "$stray" ]; then
  echo "packages from another version are sitting in $PKG:"
  printf '%s\n' "$stray" | sed 's/^/  /'
  echo
  echo "this builder globs by extension, not by version, so they would all be"
  echo "signed into the repository. Remove them and rebuild what you need:"
  echo "  rm -rf $PKG && ./packaging/nfpm/build-packages.sh"
  exit 1
fi

# --- keys (generated once; keep keys/ backed up and OUT of git) --------------
mkdir -p "$KEYS"

if [ ! -f "$KEYS/gpg-private.asc" ]; then
  echo "=== generating the aspic GPG key (first run) ==="
  # RSA-4096, not ed25519.
  #
  # This was ed25519 for exactly one afternoon. rpm 4.14 — RHEL 8, AlmaLinux 8,
  # Rocky 8, all still in support — cannot *import* an ed25519 public key at
  # all: `rpm --import` answers "key 1 import failed" and every signature then
  # reads as NOKEY. rpm 4.16 (RHEL 9) and later are fine, which is exactly what
  # makes it a trap: the machine you test on works.
  #
  # ed25519 buys a shorter signature and nothing else here, so RSA it is. apt
  # and pacman never cared either way.
  #
  # No passphrase: signing runs unattended in a container, and a passphrase a
  # script has to type is a passphrase stored next to the key. The protection is
  # that this file never leaves the workstation.
  docker run --rm -v "$MOUNT/$KEYS:/keys" debian:bookworm-slim bash -c "
    set -e
    apt-get -qq update >/dev/null && apt-get -qq install -y gnupg >/dev/null
    export GNUPGHOME=\$(mktemp -d)
    gpg --batch --quiet --gen-key <<EOF
%no-protection
Key-Type: RSA
Key-Length: 4096
Subkey-Type: RSA
Subkey-Length: 4096
Name-Real: $GPG_NAME
Name-Email: $GPG_EMAIL
Expire-Date: 0
%commit
EOF
    gpg --batch --armor --export-secret-keys > /keys/gpg-private.asc
    gpg --batch --armor --export > /keys/gpg-public.asc
    gpg --list-keys --with-colons | awk -F: '/^fpr/{print \$10; exit}' > /keys/gpg-fingerprint.txt
  "
fi
echo "GPG fingerprint: $(cat "$KEYS/gpg-fingerprint.txt")"

if [ ! -f "$KEYS/$APK_KEY" ]; then
  echo "=== generating the Alpine RSA key (first run) ==="
  # RSA because apk-tools requires it: there is no ed25519 option there.
  docker run --rm -v "$MOUNT/$KEYS:/keys" alpine:3.20 sh -c "
    apk add --no-cache openssl >/dev/null
    openssl genrsa -out /keys/$APK_KEY 4096 2>/dev/null
    openssl rsa -in /keys/$APK_KEY -pubout -out /keys/$APK_KEY.pub 2>/dev/null
  "
fi

rm -rf "$OUT"; mkdir -p "$OUT"

# The pages and the repositories are one tree, because publishing replaces the
# whole thing: a page-only push that did not carry the repositories would delete
# them, and a repository-only push would delete the pages.
cp site/index.html site/aspic.css site/favicon.svg "$OUT/"
mkdir -p "$OUT/breeze-core"
cp site/breeze-core/index.html "$OUT/breeze-core/index.html"

# The migration script, with a checksum generated here rather than pasted into a
# page. It is served from the root because the one-liner that fetches it is the
# shortest URL somebody will ever be asked to type into a root shell.
#
# Two gates before it is carried, because this file runs as root on somebody
# else's machine and both of these have already broken a migration halfway
# through - after the backup, before the install.
#
#   1. bash parses it.
#   2. Nothing inside the generated ROLLBACK here-document expands at write
#      time except a plain $VAR or a substitution with no ")" in it.
#
# The reason is OpenBSD. Its sh scans a here-document for substitutions with a
# matcher that stops at the first unbalanced ")", so "$(case $X in apt) ..."
# ends at "apt)" and the shell reports "syntax error: 'case' unmatched" - at
# RUNTIME, which is why `sh -n` calls the file clean and why this has to be
# checked here instead. A backtick in a comment in that document does the same
# thing, because the body is scanned before anything treats # as a comment.
# bash, dash, FreeBSD sh and NetBSD sh all accept both. Compute the value into
# a variable before the document and reference the variable.
bash -n site/migrate.sh || { echo "site/migrate.sh does not parse" >&2; exit 1; }
hd=$(awk '/<<ROLLBACK$/{h=1} h{print} /^ROLLBACK$/{h=0}' site/migrate.sh)
[ -n "$hd" ] || { echo "!! could not find the ROLLBACK here-document to check" >&2; exit 1; }
if printf '%s' "$hd" | grep -q -e '[$](case' -e "$(printf '`')"; then
  echo "!! the ROLLBACK here-document expands a case or a backtick inline." >&2
  echo "   It parses here and breaks on OpenBSD. Hoist it into a variable." >&2
  exit 1
fi
cp site/migrate.sh "$OUT/migrate.sh"
chmod 644 "$OUT/migrate.sh"
( cd "$OUT" && sha256sum migrate.sh > migrate.sh.sha256 )

cp "$KEYS/gpg-public.asc" "$OUT/aspic.asc"
cp "$KEYS/$APK_KEY.pub" "$OUT/aspic-alpine.rsa.pub"
# Public keys come out of a directory kept tight and MUST end up world-readable,
# or the web server 403s them and nobody can verify anything at all.
chmod 644 "$OUT/aspic.asc" "$OUT/aspic-alpine.rsa.pub"

# --- apt (deb) --------------------------------------------------------------
echo "=== apt repo ==="
stage debian:bookworm-slim '
  apt-get -qq update >/dev/null && apt-get -qq install -y apt-utils gnupg >/dev/null
  export GNUPGHOME=$(mktemp -d)
  gpg --batch --quiet --import /work/packaging/repo/keys/gpg-private.asc

  R=/out/deb
  mkdir -p "$R/pool/main/b/breeze-core"
  cp /work/packaging/out/pkg/*.deb "$R/pool/main/b/breeze-core/"
  cd "$R"
  ARCHES="amd64 arm64 armhf riscv64 ppc64el s390x"
  for a in $ARCHES; do
    mkdir -p "dists/stable/main/binary-$a"
    apt-ftparchive --arch "$a" packages pool > "dists/stable/main/binary-$a/Packages"
    gzip -9kf "dists/stable/main/binary-$a/Packages"
  done
  cd dists/stable
  apt-ftparchive \
    -o APT::FTPArchive::Release::Origin=Aspic \
    -o APT::FTPArchive::Release::Label=Aspic \
    -o APT::FTPArchive::Release::Suite=stable \
    -o APT::FTPArchive::Release::Codename=stable \
    -o APT::FTPArchive::Release::Architectures="$ARCHES" \
    -o APT::FTPArchive::Release::Components=main \
    release . > Release
  # Both forms: InRelease is what a modern apt fetches, Release.gpg is what an
  # older one falls back to, and shipping only the first quietly excludes it.
  gpg --batch --yes --clearsign -o InRelease Release
  gpg --batch --yes -abs -o Release.gpg Release
'

# --- rpm (dnf/zypper) -------------------------------------------------------
echo "=== rpm repo ==="
stage almalinux:9 '
  dnf -q install -y createrepo_c rpm-sign gnupg2 >/dev/null
  export GNUPGHOME=$(mktemp -d)
  gpg --batch --quiet --import /work/packaging/repo/keys/gpg-private.asc
  cat > ~/.rpmmacros <<EOF
%_signature gpg
%_gpg_name Aspic Repository
EOF
  for a in x86_64 aarch64 armv7hl riscv64 ppc64le s390x; do
    mkdir -p "/out/rpm/$a"
    cp /work/packaging/out/pkg/*."$a".rpm "/out/rpm/$a/" 2>/dev/null || continue
    rpmsign --addsign "/out/rpm/$a/"*.rpm >/dev/null
    createrepo_c --general-compress-type gz "/out/rpm/$a" >/dev/null
    # repo_gpgcheck=1 verifies this file, which is what makes the metadata
    # trustworthy rather than only the packages it lists.
    gpg --batch --yes -abs -o "/out/rpm/$a/repodata/repomd.xml.asc" \
        "/out/rpm/$a/repodata/repomd.xml"
  done
'
cp packaging/repo/aspic.repo "$OUT/rpm/aspic.repo"

# --- pacman (arch) ----------------------------------------------------------
echo "=== pacman repo ==="
stage archlinux:base '
  export GNUPGHOME=$(mktemp -d)
  gpg --batch --quiet --import /work/packaging/repo/keys/gpg-private.asc
  KEYID=$(gpg --list-keys --with-colons | awk -F: "/^fpr/{print \$10; exit}")
  for a in x86_64 aarch64 armv7h; do
    mkdir -p "/out/arch/$a"
    cp /work/packaging/out/pkg/*-"$a".pkg.tar.zst "/out/arch/$a/" 2>/dev/null || continue
    ( cd "/out/arch/$a"
      # SigLevel = Required wants a signature per package AND a signed database.
      for p in *.pkg.tar.zst; do gpg --batch --yes --detach-sign "$p"; done
      repo-add --sign --key "$KEYID" aspic.db.tar.gz *.pkg.tar.zst >/dev/null
      # repo-add leaves .db and .files as symlinks to the .tar.gz. A symlink in a
      # published tree is a 404 waiting to happen, so materialise them.
      for l in aspic.db aspic.db.sig aspic.files aspic.files.sig; do
        [ -L "$l" ] && cp --remove-destination "$(readlink -f "$l")" "$l"
      done
      true )
  done
'

# --- apk (alpine) -----------------------------------------------------------
echo "=== apk repo ==="
stage alpine:3.20 '
  apk add --no-cache abuild apk-tools >/dev/null
  for a in x86_64 aarch64 armv7 riscv64 ppc64le s390x; do
    src=/work/packaging/out/pkg/breeze-core_${VER}_${a}.apk
    [ -f "$src" ] || continue
    mkdir -p "/out/alpine/$a"
    # apk fetches a package as <name>-<V field>.apk, so the filename has to
    # match the index rather than nfpm output naming.
    cp "$src" "/out/alpine/$a/breeze-core-${VER}.apk"
    ( cd "/out/alpine/$a"
      apk index --allow-untrusted --rewrite-arch "$a" -o APKINDEX.tar.gz *.apk 2>/dev/null
      abuild-sign -k /work/packaging/repo/keys/aspic-alpine.rsa APKINDEX.tar.gz )
  done
'

# --- opkg feed (OpenWrt) ----------------------------------------------------
# opkg verifies feeds with usign, OpenWrt's own ed25519 signer: GPG is useless
# to it. usign is tiny, so it gets built from source here.
echo "=== opkg feed (OpenWrt) ==="
printf '%s' '
apk add --no-cache build-base cmake git >/dev/null
git clone -q https://github.com/openwrt/usign /tmp/usign
cmake -S /tmp/usign -B /tmp/usign/b >/dev/null && make -s -C /tmp/usign/b
US=/tmp/usign/b/usign

if [ ! -f /keys/usign.sec ]; then
  echo "  generating the usign key (first run)"
  "$US" -G -s /keys/usign.sec -p /keys/usign.pub -c "Aspic repository"
fi

for f in /work/packaging/out/pkg/breeze-core_*_*.ipk; do
  [ -e "$f" ] || { echo "no .ipk files"; exit 1; }
  # breeze-core_<ver>_<arch>.ipk, and OpenWrt architecture labels contain
  # underscores (aarch64_generic), so take fields 3 onward, not the last one.
  arch=$(basename "$f" .ipk | cut -d_ -f3-)
  mkdir -p "/out/openwrt/$arch"; cp "$f" "/out/openwrt/$arch/"
done
for d in /out/openwrt/*/; do (
  cd "$d"
  : > Packages
  for p in *.ipk; do
    tar -xzOf "$p" ./control.tar.gz | tar -xzO ./control >> Packages
    printf "Filename: %s\nSize: %s\nSHA256sum: %s\n\n" \
      "$p" "$(wc -c < "$p")" "$(sha256sum "$p" | cut -d" " -f1)" >> Packages
  done
  gzip -9kf Packages
  "$US" -S -m Packages -s /keys/usign.sec -x Packages.sig
); done
cp /keys/usign.pub /out/aspic-usign.pub
chmod 644 /out/aspic-usign.pub
"$US" -F -p /keys/usign.pub > /out/aspic-usign.fingerprint
echo "  feed signed (usign fingerprint $(cat /out/aspic-usign.fingerprint))"
' | docker run --rm -i -v "$MOUNT:/work:ro" -v "$MOUNT/$KEYS:/keys" \
      -e VER="$VER" alpine:3.20 sh -euc '
        exec 3>&1 1>&2
        mkdir -p /out
        sh -eu -s
        tar -cf - -C /out . >&3
      ' | tar -xf - -C "$OUT"

# --- Windows installer ------------------------------------------------------
# A download, not a repository: Windows has no package manager in the sense the
# index page is about, so the installer is linked from the project's own page
# instead. Built by packaging/windows/build-installer.ps1, which needs Windows.
echo "=== Windows installer ==="
if ls packaging/out/windows/*.exe >/dev/null 2>&1; then
  mkdir -p "$OUT/windows"
  cp packaging/out/windows/*.exe "$OUT/windows/"
  # A checksum file per installer, generated here so it cannot drift from the
  # binary the way a hash pasted into a page does.
  ( cd "$OUT/windows" && for f in *.exe; do sha256sum "$f" > "$f.sha256"; done )
  ls -1 "$OUT/windows" | sed 's/^/  /'
else
  # Loud, because the project page has a Windows section and a missing
  # installer makes it a section that 404s.
  echo "  !! nothing in packaging/out/windows"
  echo "     build it on Windows: .\\packaging\\windows\\build-installer.ps1"
fi

# --- FreeBSD (pkg) ----------------------------------------------------------
# Built AND signed on a real FreeBSD machine by packaging/bsd/build-freebsd.sh:
# `pkg repo` signs locally, so unlike every other repository here this one is
# not assembled on this workstation. The key visits that machine and is shredded
# afterwards; see that script.
echo "=== FreeBSD repo ==="
if [ -d packaging/out/bsd/freebsd ]; then
  cp -R packaging/out/bsd/freebsd "$OUT/freebsd"
  chmod 644 "$OUT/freebsd/"*
  # data.pkg and packagesite.pkg are the catalogue, not packages, so they are
  # excluded from the count rather than inflating it to three.
  echo "  carried $(ls "$OUT/freebsd"/*.pkg 2>/dev/null | grep -v -e '/data\.pkg$' -e '/packagesite\.pkg$' | wc -l | tr -d ' ') package(s) and their signed catalogue"
else
  echo "  !! nothing staged in packaging/out/bsd/freebsd"
  echo "     run packaging/bsd/build-freebsd.sh, or the FreeBSD section will 404"
fi

# --- NetBSD (pkgin) ---------------------------------------------------------
# Built on a real NetBSD machine by packaging/bsd/build-netbsd.sh and carried
# here, because there is no cross-build for it: Zig bundles no NetBSD libc.
#
# Unsigned, unlike every other feed here — pkgin has no signature verification
# to offer, and pkg_add's GPG support needs a signed-package format this does
# not use. The page says so rather than implying parity.
echo "=== NetBSD feed ==="
if [ -d packaging/out/bsd/netbsd ]; then
  cp -R packaging/out/bsd/netbsd "$OUT/netbsd"
  echo "  carried $(find packaging/out/bsd/netbsd -name '*.tgz' | wc -l | tr -d ' ') package(s)"
else
  # Loud, because the page has a NetBSD section and a missing feed makes it a
  # section that 404s.
  echo "  !! nothing staged in packaging/out/bsd/netbsd"
  echo "     run packaging/bsd/build-netbsd.sh, or the NetBSD section will 404"
fi

# --- OpenBSD (pkg_add) ------------------------------------------------------
# Built and signify-signed on a real OpenBSD machine by build-openbsd.sh, which
# stages it as <release>/packages/<arch>/ - the layout pkg_add builds when it
# expands %c and %a out of PKG_PATH. Release-specific on purpose: OpenBSD moves
# its libc every six months and does not pretend a 7.9 package runs on 7.8.
#
# How pkg_add finds it matters here in a way it does not for any other client:
# there is no index file at all. pkg_add GETs the directory and scrapes
# <A HREF="....tgz"> out of the HTML, so the `autoindex on` in site/aspic.conf
# is not a convenience for humans browsing the tree - it is the OpenBSD index.
# Turn it off and pkg_add reports the package as nonexistent.
echo "=== OpenBSD packages ==="
if [ -d packaging/out/bsd/openbsd ]; then
  cp -R packaging/out/bsd/openbsd "$OUT/openbsd"
  find "$OUT/openbsd" -type f -exec chmod 644 {} +
  echo "  carried $(find "$OUT/openbsd" -name '*.tgz' | wc -l | tr -d ' ') signed package(s)"
  # The public key is the whole trust anchor: pkg_add reads the key's NAME out
  # of the signature and opens /etc/signify/<name>.pub, so this file has to
  # arrive under the name the signature asks for or every install refuses.
  if [ -f "$OUT/openbsd/aspic-pkg.pub" ]; then
    echo "  aspic-pkg.pub: $(tr -d '\n' < "$OUT/openbsd/aspic-pkg.pub" | tail -c 24)"
  else
    echo "  !! aspic-pkg.pub is missing - pkg_add will refuse every package"
  fi
else
  echo "  !! nothing staged in packaging/out/bsd/openbsd"
  echo "     run packaging/bsd/build-openbsd.sh, or the OpenBSD section will 404"
fi

# --- OPNsense plugin --------------------------------------------------------
# A single .pkg installed by hand (`pkg add <url>`), not a repository: an
# OPNsense box already has pkg pointed at its own mirrors with its own ABI and
# its own trust, and adding a third-party repository to a firewall is a bigger
# ask than fetching one file. Built for FreeBSD:14, which is what OPNsense is.
echo "=== OPNsense plugin ==="
if ls packaging/out/opnsense/os-breeze-core-*.pkg >/dev/null 2>&1; then
  mkdir -p "$OUT/opnsense"
  cp packaging/out/opnsense/os-breeze-core-*.pkg "$OUT/opnsense/"
  chmod 644 "$OUT/opnsense"/*.pkg
  ( cd "$OUT/opnsense" && for f in *.pkg; do sha256sum "$f" > "$f.sha256"; done )
  ls -1 "$OUT/opnsense" | sed 's/^/  /'
else
  echo "  !! nothing in packaging/out/opnsense"
  echo "     run packaging/opnsense/build-plugin.sh, or that section will 404"
fi

echo
echo "== $OUT =="
find "$OUT" -type f | sed "s|$OUT/|  |" | sort
echo
echo "signed for $VER. Verify it with:   ./packaging/repo/verify-repo.sh"
echo "Publish it with:                   ./site/publish.sh --tree $OUT"
