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
#   ├── aspic.asc  aspic-alpine.rsa.pub  aspic-usign.pub  aspic-xbps.fingerprint
#   ├── deb/     dists/stable/… + pool/         (apt,  GPG InRelease)
#   ├── rpm/     <arch>/repodata/ + aspic.repo  (dnf/zypper, signed rpms)
#   ├── arch/    <arch>/aspic.db…               (pacman, signed db + packages)
#   ├── alpine/  <arch>/APKINDEX.tar.gz         (apk, RSA-signed index)
#   ├── openwrt/ <arch>/Packages + Packages.sig (opkg, usign)
#   ├── xbps/    <arch>-repodata + .xbps        (Void, RSA-signed, trust-on-first-use)
#   └── portage/breeze.git/                     (Gentoo overlay, dumb-HTTP git)
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
XBPS_KEY="aspic-xbps.pem"
# Persistent, and it MUST be: the Gentoo overlay is published as a git
# repository, and regenerating it from scratch each release would give every
# commit a new hash. A user who had added the overlay would then get
# "refusing to merge unrelated histories" from `emerge --sync` and would have
# to remove and re-add it every time. So the history lives here, next to the
# keys, and needs backing up for the same reason they do.
PORTAGE_GIT="packaging/repo/portage-git"

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

if [ ! -f "$KEYS/$XBPS_KEY" ]; then
  echo "=== generating the xbps RSA key (first run) ==="
  # A fourth key, and RSA again because that is all xbps-rindex signs with.
  #
  # xbps trusts on first use: the public half is embedded in the repodata
  # signature, and `xbps-install` shows the fingerprint and asks once before
  # storing it under /var/db/xbps/keys. So unlike the apt, apk and opkg keys
  # there is nothing for a user to fetch beforehand — but the fingerprint is
  # published next to the repository so the prompt can be checked against
  # something rather than accepted blind.
  docker run --rm -v "$MOUNT/$KEYS:/keys" alpine:3.20 sh -c "
    apk add --no-cache openssl >/dev/null
    openssl genrsa -out /keys/$XBPS_KEY 4096 2>/dev/null
    chmod 600 /keys/$XBPS_KEY
  "
fi

rm -rf "$OUT"; mkdir -p "$OUT"

# The pages and the repositories are one tree, because publishing replaces the
# whole thing: a page-only push that did not carry the repositories would delete
# them, and a repository-only push would delete the pages.
cp site/aspic.css site/favicon.svg "$OUT/"
mkdir -p "$OUT/breeze-core"
# @VER@ substituted rather than written out, because these pages carry download
# URLs with the version in the path -- the NetBSD and OpenBSD tarballs, the
# OPNsense plugin, the Windows installer. Hand-maintained, they went stale
# silently: the page kept advertising 4.0.0 files after 4.0.1 shipped, and a
# stale link on a download page is a 404 for a visitor and looks like the
# project is broken rather than the page being old.
sed "s/@VER@/$VER/g" site/index.html > "$OUT/index.html"
sed "s/@VER@/$VER/g" site/breeze-core/index.html > "$OUT/breeze-core/index.html"

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

# --- xbps (Void) ------------------------------------------------------------
# Two packages per architecture, glibc and musl, with identical bytes. Void
# treats the two libcs as separate architectures with separate repodata, and
# xbps reads only <its own arch>-repodata, so a single package would be
# invisible to half of Void even though a static binary runs on both.
#
# One directory holds every architecture's repodata, which is how Void's own
# repository is laid out. xbps-rindex indexes only packages matching XBPS_ARCH
# and says "ignoring …, unmatched arch" for the others, so it runs once per
# architecture rather than once over the directory.
echo "=== xbps repo (Void) ==="
if ls packaging/out/xbps/*.xbps >/dev/null 2>&1; then
  stage ghcr.io/void-linux/void-glibc-full:latest '
    KEY=/work/packaging/repo/keys/aspic-xbps.pem
    ARCHES="x86_64 x86_64-musl aarch64 aarch64-musl armv7l armv7l-musl
            riscv64 riscv64-musl ppc64le ppc64le-musl"
    mkdir -p /out/xbps
    cp /work/packaging/out/xbps/*.xbps /out/xbps/

    for a in $ARCHES; do
      XBPS_ARCH="$a" xbps-rindex -a /out/xbps/*.xbps 2>&1 \
        | grep -v "unmatched arch" || true
    done

    # The packages, all at once: --sign-pkg works off each package file and
    # does not care about XBPS_ARCH.
    xbps-rindex --sign-pkg --privkey "$KEY" /out/xbps/*.xbps >/dev/null 2>&1

    # The indexes, ONCE PER ARCHITECTURE. --sign signs only the repodata of
    # the current XBPS_ARCH, silently and with no hint that the other nine were
    # left alone: it prints "Initialized signed repository (1 package)" either
    # way. Running it once shipped nine unsigned architectures, which a Void
    # box on any of them would have accepted without a word about the key.
    for a in $ARCHES; do
      XBPS_ARCH="$a" xbps-rindex --sign --signedby "Aspic Repository" \
        --privkey "$KEY" /out/xbps >/dev/null
    done

    # The fingerprint, taken FROM XBPS rather than computed here.
    #
    # xbps shows this string when it asks a user whether to trust the key, and
    # it is not the md5, sha1 or sha256 of the DER or PEM public key -- all four
    # were checked against it and none matched. Publishing a digest of our own
    # invention would give a user something that disagrees with their prompt,
    # which is worse than publishing nothing: the natural reading of a mismatch
    # is that the repository has been tampered with.
    #
    # Void own default repository is removed first so this does not also pull
    # two megabytes of their index just to print one line.
    rm -f /usr/share/xbps.d/*.conf /etc/xbps.d/*.conf
    mkdir -p /tmp/conf && echo "repository=/out/xbps" > /tmp/conf/aspic.conf
    xbps-install -C /tmp/conf -S 2>&1 \
      | sed -n "s/^Fingerprint: //p" | head -1 > /out/aspic-xbps.fingerprint
    chmod 644 /out/aspic-xbps.fingerprint

    printf "  %s repodata, all signed\n" "$(ls -1 /out/xbps/*-repodata | wc -l | tr -d " ")"
    printf "  key fingerprint (as xbps shows it): %s\n" "$(cat /out/aspic-xbps.fingerprint)"
  '
  [ -s "$OUT/aspic-xbps.fingerprint" ] || {
    echo "  !! could not read the key fingerprint back out of xbps"; exit 1; }
else
  echo "  !! nothing in packaging/out/xbps"
  echo "     run packaging/xbps/build-xbps.sh, or that section will 404"
fi

# --- portage overlay (Gentoo) -----------------------------------------------
# Not a package but a git repository, because that is Gentoo's unit of
# distribution for third-party ebuilds. Served as ORDINARY STATIC FILES: a bare
# repository plus `git update-server-info` is a "dumb HTTP" remote, which git
# still clones and pulls from with no git backend on the host at all. That is
# what lets the overlay live in this static tree instead of needing a public
# git forge — verified by cloning one out of `python3 -m http.server`.
#
# The history is kept in $PORTAGE_GIT between releases. See the comment on that
# variable: a fresh repository each release would break `emerge --sync` for
# everyone who had already added it.
echo "=== portage overlay (Gentoo) ==="
if [ -f packaging/out/portage/app-misc/breeze-core-bin/Manifest ]; then
  # Done on the host rather than in a container: this is the one piece of
  # durable state the build writes, and a writable bind mount is the thing
  # Docker Desktop here is least reliable about.
  if [ ! -d "$PORTAGE_GIT/.git" ]; then
    echo "  creating the overlay history (first run) — BACK $PORTAGE_GIT UP"
    mkdir -p "$PORTAGE_GIT"
    git -C "$PORTAGE_GIT" init -q -b master
  fi
  # core.autocrlf off for this repository specifically. The workstation has it
  # on globally, and an ebuild is bash sourced by portage: a CRLF checkout
  # fails on its first line for every user of the overlay.
  git -C "$PORTAGE_GIT" config core.autocrlf false
  git -C "$PORTAGE_GIT" config user.name "Aspic Repository"
  git -C "$PORTAGE_GIT" config user.email "$GPG_EMAIL"

  # Mirror the generated overlay over the history, deletions included, so a
  # package removed upstream disappears here too.
  find "$PORTAGE_GIT" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
  cp -r packaging/out/portage/. "$PORTAGE_GIT/"
  git -C "$PORTAGE_GIT" add -A
  if git -C "$PORTAGE_GIT" diff --cached --quiet; then
    echo "  overlay unchanged since the last release, history untouched"
  else
    git -C "$PORTAGE_GIT" commit -q -m "breeze-core-bin $VER"
    echo "  committed breeze-core-bin $VER"
  fi

  mkdir -p "$OUT/portage"
  rm -rf "$OUT/portage/breeze.git"
  git clone -q --bare "$PORTAGE_GIT" "$OUT/portage/breeze.git"
  # Without this there is no info/refs or objects/info/packs, and a dumb HTTP
  # clone fails with "repository not found" — which reads as a missing repo
  # rather than a missing index.
  git -C "$OUT/portage/breeze.git" update-server-info
  # A bare clone carries the origin it came from, which is a path on this
  # workstation. Harmless but pointless in a published tree, and it names a
  # local directory to anyone who reads it.
  git -C "$OUT/portage/breeze.git" remote remove origin 2>/dev/null || true
  echo "  $(git -C "$PORTAGE_GIT" rev-list --count HEAD) commit(s), served over dumb HTTP"
else
  echo "  !! nothing in packaging/out/portage"
  echo "     run packaging/portage/build-overlay.sh, or that section will 404"
fi

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
