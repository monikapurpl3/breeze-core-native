#!/usr/bin/env bash
# Install from the built repository tree, in clean containers, with signature
# checking ON — and check that it FAILS without the key.
#
#   ./packaging/repo/verify-repo.sh            # every client below
#   ./packaging/repo/verify-repo.sh debian
#
# The tree is served over HTTP by an nginx container on a private docker network,
# because that is the only way to exercise what a real client does: the URL
# layout, the index fetch, and the signature check. A file:// test would verify
# the signatures but not the paths, and the paths are what the landing page
# promises.
#
# **Both directions are checked.** A repository that installs is not necessarily
# a repository that verifies: every one of these clients will happily install
# from an unsigned or wrongly-signed source if asked the wrong way, so each case
# first proves the client refuses the repository without the key, then proves it
# accepts it with the key. Only the pair means anything.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
TREE="packaging/out/aspic"
VER="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
NET=aspic-verify
HOST=aspicrepo
BASE="http://$HOST"

MOUNT="$REPO"
case "$MOUNT" in /[a-z]/*) MOUNT="$(echo "$MOUNT" | sed -E 's#^/([a-z])/#\U\1:/#')" ;; esac
export MSYS_NO_PATHCONV=1

# --live tests what is actually published rather than what is about to be: same
# clients, same checks, against the public URL. Worth running once after a
# publish, because it is the only thing that exercises DNS, TLS, the vhost's
# headers and the tree all at once.
LIVE=0
if [ "${1:-}" = "--live" ]; then LIVE=1; shift; fi
if [ "$LIVE" = 1 ]; then
  BASE="${ASPIC_URL:-https://aspic.salataputarica.hr.eu.org}"
else
  [ -d "$TREE/deb" ] || { echo "no tree — run packaging/repo/build-repo.sh first"; exit 1; }
fi

pass=0; fail=0
report() {
  if [ "$1" = 0 ]; then printf '  \033[32mPASS\033[0m  %s\n' "$2"; pass=$((pass+1))
  else printf '  \033[31mFAIL\033[0m  %s\n' "$2"; fail=$((fail+1)); fi
}

cleanup() {
  docker rm -f "$HOST" >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
}
trap cleanup EXIT

if [ "$LIVE" = 1 ]; then
  # No private network: the containers need real DNS and the internet.
  NETARG=""
  echo "testing the PUBLISHED tree at $BASE"
else
  NETARG="--network $NET"
  docker network create "$NET" >/dev/null 2>&1 || true
  docker rm -f "$HOST" >/dev/null 2>&1 || true
  docker run -d --name "$HOST" --network "$NET" \
    -v "$MOUNT/$TREE:/usr/share/nginx/html:ro" nginx:alpine \
    sh -c "sed -i 's#location / {#location / { autoindex on;#' /etc/nginx/conf.d/default.conf && exec nginx -g 'daemon off;'" >/dev/null
  # Directory listings on, as site/aspic.conf has them: OpenBSD's pkg_add reads
  # its index out of one, and the Arch source case finds its tarball in another.
  echo "serving $TREE as $BASE"
fi

want=("$@")
selected() {
  [ ${#want[@]} -eq 0 ] && return 0
  for w in "${want[@]}"; do [ "$w" = "$1" ] && return 0; done
  return 1
}

run_case() {
  local name="$1" image="$2" script="$3"
  selected "$name" || return 0
  echo
  echo "=== $name ($image)"
  # BASE and VER as the script's first lines as well as -e: termux-docker's
  # entrypoint switches to the app user through `env -i`, which drops every
  # variable it does not list itself. Harmless everywhere else.
  if printf 'BASE=%s\nVER=%s\n%s' "$BASE" "$VER" "$script" \
     | timeout 900 docker run --rm -i ${NETARG} -e BASE="$BASE" -e VER="$VER" \
         "$image" sh -eu -s 2>&1 | sed 's/^/    /'; then
    report 0 "$name"
  else
    report 1 "$name"
  fi
}

# --- apt --------------------------------------------------------------------
run_case debian debian:12 '
  apt-get -qq update >/dev/null
  apt-get -qq install -y curl gnupg >/dev/null
  echo "deb $BASE/deb stable main" > /etc/apt/sources.list.d/aspic.list

  echo "-- without the key, apt must refuse the repository"
  if apt-get update -o Dir::Etc::sourcelist=/etc/apt/sources.list.d/aspic.list \
       -o Dir::Etc::sourceparts=/dev/null 2>&1 | grep -q "NO_PUBKEY\|not signed\|no longer signed\|Missing key"; then
    echo "   refused, as it should"
  else
    echo "   !! apt accepted an unverifiable repository"; exit 1
  fi

  echo "-- with the key, it installs"
  curl -fsSL "$BASE/aspic.asc" | gpg --dearmor -o /usr/share/keyrings/aspic.gpg
  echo "deb [signed-by=/usr/share/keyrings/aspic.gpg] $BASE/deb stable main" \
    > /etc/apt/sources.list.d/aspic.list
  apt-get -qq update >/dev/null
  apt-get -qq install -y breeze-core >/dev/null
  breeze-core --version | grep -q "breeze-core $VER"
  echo "   installed $(breeze-core --version | head -1) from the signed repo"
'

# --- Termux -----------------------------------------------------------------
# Termux's own apt, as the app user, in a real Android userland. x86_64 only:
# the three architectures share one index and one signature, and
# packaging/termux/verify-termux.sh runs the packages themselves on all three.
# termux-services comes from Termux's own repository, so this needs the
# internet as well as the repo container.
run_case termux termux/termux-docker:x86_64 '
  L=$PREFIX/etc/apt/sources.list.d/aspic.list
  mkdir -p $PREFIX/etc/apt/sources.list.d $PREFIX/etc/apt/keyrings
  echo "deb $BASE/termux stable main" > $L

  echo "-- without the key, apt must refuse the repository"
  if apt-get update 2>&1 | grep -q "NO_PUBKEY\|not signed\|no longer signed\|Missing key"; then
    echo "   refused, as it should"
  else
    echo "   !! apt accepted an unverifiable repository"; exit 1
  fi

  echo "-- with the key, it installs"
  curl -fsSL "$BASE/aspic.asc" -o $PREFIX/etc/apt/keyrings/aspic.asc
  echo "deb [signed-by=$PREFIX/etc/apt/keyrings/aspic.asc] $BASE/termux stable main" > $L
  apt-get -qq update >/dev/null
  apt-get -qq install -y breeze-core >/dev/null
  breeze-core --version | grep -q "breeze-core $VER"
  echo "   installed $(breeze-core --version | head -1) from the signed repo"
'

# --- dnf --------------------------------------------------------------------
# Run against BOTH rpm generations, because they do not agree about keys: rpm
# 4.14 (RHEL/Alma/Rocky 8) cannot import an ed25519 public key at all, so a
# repository signed with one verifies perfectly on the machine you test on and
# is unusable on a distribution that is still in support. The signing key is
# RSA-4096 for that reason, and this pair of cases is what keeps it that way.
RPM_CASE="$(cat <<'CASE'
  dnf -q -y install curl >/dev/null 2>&1 || true
  curl -fsSL "$BASE/rpm/aspic.repo" \
    | sed "s#https://aspic.salataputarica.hr.eu.org#$BASE#g" \
    > /etc/yum.repos.d/aspic.repo
  grep -q "repo_gpgcheck=1" /etc/yum.repos.d/aspic.repo || {
    echo "!! the shipped .repo does not enable repo_gpgcheck"; exit 1; }

  echo "-- signed by a key we do not trust, it must be refused"
  # NOT "with no key imported": `dnf -y` imports whatever gpgkey= points at,
  # because that is exactly what -y means, so an install with no key proves
  # nothing at all. The meaningful negative is a key that does not match —
  # generate one, point the repository at it, and verification must fail.
  dnf -q -y install gnupg2 >/dev/null 2>&1 || true
  export GNUPGHOME=$(mktemp -d)
  gpg --batch --quiet --passphrase "" \
      --quick-generate-key "Not Aspic <nobody@example.invalid>" default default never
  gpg --batch --armor --export > /tmp/wrong.asc
  if dnf -q -y --setopt=aspic.gpgkey=file:///tmp/wrong.asc install breeze-core >/tmp/out 2>&1; then
    echo "   !! dnf installed with the wrong key trusted"; tail -3 /tmp/out; exit 1
  else
    echo "   refused, as it should"
  fi
  dnf -q -y remove breeze-core >/dev/null 2>&1 || true

  echo "-- the key imports (rpm 4.14 refuses ed25519 outright)"
  rpm --import "$BASE/aspic.asc"

  echo "-- and the package signature verifies against it"
  # Asked of rpm rather than read out of a header tag: an RSA signature lands in
  # RSAHEADER and an ed25519 one in DSAHEADER, so querying the wrong tag reports
  # "(none)" for a package that is perfectly well signed. `rpm -K` just answers.
  # Fetched by dnf rather than by a constructed URL. The filename carries the
  # package RELEASE too (breeze-core-4.0.2-2.x86_64.rpm), and hardcoding -1
  # here meant the first rebuilt release 404'd this check while every other
  # check still passed -- a signature test that silently stops testing the
  # signature is worse than not having one.
  dnf -q -y download breeze-core --destdir /tmp/dl >/dev/null 2>&1 \
    || dnf -q -y install --downloadonly --downloaddir /tmp/dl breeze-core >/dev/null 2>&1
  ls /tmp/dl/breeze-core-*.rpm >/dev/null 2>&1 || {
    echo "   !! could not download the rpm to check its signature"; exit 1; }
  rpm -K /tmp/dl/breeze-core-*.rpm | tee /tmp/k
  grep -q "signatures OK" /tmp/k

  echo "-- with the key, it installs"
  dnf -q -y install breeze-core >/dev/null
  breeze-core --version | grep -q "breeze-core $VER"
  echo "   installed $(breeze-core --version | head -1)"
CASE
)"
run_case alma  almalinux:9 "$RPM_CASE"
run_case alma8 almalinux:8 "$RPM_CASE"

# --- pacman -----------------------------------------------------------------
run_case arch archlinux:base '
  pacman -Sy --noconfirm --quiet curl >/dev/null 2>&1 || true
  cat >> /etc/pacman.conf <<CONF

[aspic]
SigLevel = Required
Server = $BASE/arch/\$arch
CONF

  echo "-- without the key in the pacman keyring, the sync must fail"
  if pacman -Sy --noconfirm breeze-core >/tmp/out 2>&1; then
    echo "   !! pacman installed without a trusted key"; exit 1
  else
    echo "   refused, as it should"
  fi

  echo "-- with the key, it installs"
  pacman-key --init >/dev/null 2>&1
  curl -fsSL -o /tmp/aspic.asc "$BASE/aspic.asc"
  pacman-key --add /tmp/aspic.asc >/dev/null 2>&1
  fpr=$(gpg --show-keys --with-colons /tmp/aspic.asc | awk -F: "/^fpr/{print \$10; exit}")
  pacman-key --lsign-key "$fpr" >/dev/null 2>&1
  pacman -Sy --noconfirm breeze-core >/dev/null
  breeze-core --version | grep -q "breeze-core $VER"
  echo "   installed from a signed db with SigLevel = Required"
'

# --- apk --------------------------------------------------------------------
run_case alpine alpine:3.20 '
  apk add --no-cache curl >/dev/null
  # No architecture in the URL: apk appends it itself, and pointing at the
  # per-arch directory makes it fetch /alpine/x86_64/x86_64/APKINDEX.tar.gz.
  echo "$BASE/alpine" >> /etc/apk/repositories

  echo "-- without the key, apk must refuse the index"
  if apk update >/tmp/out 2>&1 && apk add breeze-core >>/tmp/out 2>&1; then
    echo "   !! apk installed from an untrusted index"; exit 1
  else
    echo "   refused, as it should"
  fi

  echo "-- with the key, it installs"
  curl -fsSL -o /etc/apk/keys/aspic-alpine.rsa.pub "$BASE/aspic-alpine.rsa.pub"
  apk update >/dev/null
  apk add breeze-core >/dev/null
  breeze-core --version | grep -q "breeze-core $VER"
  echo "   installed from an RSA-signed APKINDEX"
'

# --- xbps -------------------------------------------------------------------
# The "both directions" shape is different here, because xbps trusts on first
# use rather than from a key installed beforehand: there is no without-the-key
# case to construct. What matters instead is that xbps reports the repository as
# SIGNED and by us, that the fingerprint it shows matches the one the site
# publishes, and that every architecture is signed rather than only the one the
# signing container happened to be.
run_case void ghcr.io/void-linux/void-glibc-full:latest '
  # curl FIRST: the image ships no HTTP client at all -- no curl, no wget --
  # and the next line removes the repositories it would be installed from.
  xbps-install -Sy curl >/dev/null 2>&1
  # Void own repository removed, so this exercises ours and only ours, and does
  # not depend on their mirror being reachable.
  rm -f /usr/share/xbps.d/*.conf /etc/xbps.d/*.conf
  mkdir -p /etc/xbps.d
  echo "repository=$BASE/xbps" > /etc/xbps.d/20-aspic.conf

  echo "-- xbps must see it as RSA signed, and attribute it to us"
  xbps-install -S >/tmp/sync 2>&1 </dev/null || true
  grep -q "has been RSA signed by .Aspic Repository." /tmp/sync || {
    echo "   !! not reported as signed"; sed "s/^/      /" /tmp/sync | head; exit 1; }
  echo "   signed by Aspic Repository"

  echo "-- and the fingerprint must match the published one"
  shown=$(sed -n "s/^Fingerprint: //p" /tmp/sync | head -1 | tr -d " \r")
  published=$(curl -fsS "$BASE/aspic-xbps.fingerprint" | tr -d " \r\n")
  [ -n "$shown" ] && [ "$shown" = "$published" ] || {
    echo "   !! xbps shows [$shown], the site publishes [$published]"; exit 1; }
  echo "   $shown"

  echo "-- every architecture signed, not just this one"
  for a in x86_64 x86_64-musl aarch64 aarch64-musl armv7l armv7l-musl \
           riscv64 riscv64-musl ppc64le ppc64le-musl; do
    XBPS_ARCH=$a xbps-install -S 2>&1 </dev/null | grep -q "RSA signed by" \
      || { echo "   !! $a is UNSIGNED"; exit 1; }
  done
  echo "   all ten architecture/libc combinations"

  echo "-- and it installs, once the key is accepted"
  yes | xbps-install -S >/dev/null 2>&1 || true
  yes | xbps-install -y breeze-core >/tmp/inst 2>&1 || true
  [ -x /usr/bin/breeze-core ] || {
    echo "   !! not installed"; tail -6 /tmp/inst | sed "s/^/      /"; exit 1; }
  breeze-core --version | grep -q "breeze-core $VER"
  echo "   installed, and reports $VER"
'

# --- portage ----------------------------------------------------------------
# No signature to check: Gentoo trust is the Manifest, whose hashes portage
# enforces against the tarball it fetches from GitHub. What has no precedent in
# the other repositories -- and so is what this checks -- is that the overlay is
# reachable as a DUMB HTTP git remote from a purely static tree.
run_case gentoo alpine:3.20 '
  apk add --no-cache git >/dev/null

  echo "-- the overlay must clone over plain static HTTP"
  git clone -q "$BASE/portage/breeze.git" /tmp/aspic 2>/tmp/err || {
    echo "   !! clone failed"; sed "s/^/      /" /tmp/err; exit 1; }
  echo "   cloned"

  echo "-- carrying this version ebuild and a complete Manifest"
  # Globbed: a rebuild of the same upstream version carries a Gentoo revision
  # suffix (breeze-core-bin-4.0.2-r1.ebuild), and an exact name would fail then.
  ls /tmp/aspic/app-misc/breeze-core-bin/breeze-core-bin-"$VER"*.ebuild >/dev/null 2>&1 || {
    echo "   !! no ebuild for $VER"; ls /tmp/aspic/app-misc/breeze-core-bin; exit 1; }
  n=$(grep -c "^DIST " /tmp/aspic/app-misc/breeze-core-bin/Manifest || echo 0)
  [ "$n" = 6 ] || { echo "   !! Manifest has $n DIST lines, expected 6"; exit 1; }
  echo "   $(basename $(ls /tmp/aspic/app-misc/breeze-core-bin/breeze-core-bin-"$VER"*.ebuild | head -1)), 6 architectures in the Manifest"

  echo "-- the account ebuilds must be there too"
  test -f /tmp/aspic/acct-user/breeze/breeze-0.ebuild || { echo "   !! no acct-user"; exit 1; }
  test -f /tmp/aspic/acct-group/breeze/breeze-0.ebuild || { echo "   !! no acct-group"; exit 1; }
  grep -q "^acct-user_add_deps" /tmp/aspic/acct-user/breeze/breeze-0.ebuild || {
    echo "   !! acct-user is missing its add_deps call and would fail at merge"; exit 1; }
  echo "   acct-user and acct-group present, add_deps called"

  echo "-- and it must identify itself as aspic"
  grep -qx aspic /tmp/aspic/profiles/repo_name || { echo "   !! wrong repo_name"; exit 1; }
  grep -q "^masters = gentoo" /tmp/aspic/metadata/layout.conf || { echo "   !! no masters"; exit 1; }
  echo "   repo_name and layout.conf in place"

  echo "-- an incremental pull must work (what emerge --sync does)"
  git -C /tmp/aspic pull -q 2>/tmp/err2 || {
    echo "   !! pull failed"; sed "s/^/      /" /tmp/err2; exit 1; }
  echo "   pulled"
'

# --- source packages ---------------------------------------------------------
# The source, fetched through each package manager from the signed tree, and
# built with that distribution's own Rust. Tests are skipped in these builds
# (packaging/source/verify-source.sh runs them); what is checked here is the
# fetch, its signature, the build dependencies and the result. Each case
# compiles the whole program, so these take minutes.

run_case ubuntu-src ubuntu:26.04 '
  export DEBIAN_FRONTEND=noninteractive
  apt-get -qq update >/dev/null && apt-get -qq install -y curl gnupg dpkg-dev >/dev/null
  curl -fsSL "$BASE/aspic.asc" | gpg --dearmor -o /usr/share/keyrings/aspic.gpg
  printf "Types: deb deb-src\nURIs: %s/deb\nSuites: stable\nComponents: main\nSigned-By: /usr/share/keyrings/aspic.gpg\n" \
    "$BASE" > /etc/apt/sources.list.d/aspic.sources
  apt-get -qq update >/dev/null

  echo "-- apt source fetches it, and dpkg-source checks the .dsc signature"
  # dpkg-source verifies against the keys in trustedkeys.gpg, so trust the
  # repository key there first; without it, it only warns.
  gpg --batch --quiet --no-default-keyring --keyring trustedkeys.gpg --import /usr/share/keyrings/aspic.gpg
  cd /tmp && apt-get source breeze-core > src.log 2>&1 || { tail -20 src.log; exit 1; }
  grep -q "Good signature" src.log || { echo "   !! no good signature on the .dsc"; tail -10 src.log; exit 1; }
  echo "   $(grep -o "Good signature from \"[^\"]*\"" src.log | head -1)"

  echo "-- apt build-dep, then dpkg-buildpackage"
  apt-get -qq build-dep -y breeze-core >/dev/null
  cd breeze-core-$VER
  DEB_BUILD_OPTIONS=nocheck dpkg-buildpackage -b -us -uc > /tmp/build.log 2>&1 || { tail -30 /tmp/build.log; exit 1; }
  apt-get -qq install -y /tmp/breeze-core_${VER}-*_amd64.deb >/dev/null
  breeze-core --version | grep -q "breeze-core $VER"
  echo "   built and installed $(breeze-core --version | head -1)"
'

SRPM_CASE="$(cat <<'CASE'
  dnf -q -y install curl rpm-build dnf-plugins-core >/dev/null 2>&1 \
    || dnf -q -y install curl rpm-build "dnf-command(download)" "dnf-command(builddep)" >/dev/null
  curl -fsSL "$BASE/rpm/aspic.repo" \
    | sed "s#https://aspic.salataputarica.hr.eu.org#$BASE#g" > /etc/yum.repos.d/aspic.repo
  grep -q "^\[aspic-source\]" /etc/yum.repos.d/aspic.repo || { echo "!! aspic.repo has no source repository"; exit 1; }
  rpm --import "$BASE/aspic.asc"

  echo "-- dnf download fetches the SRPM from the switched-off source repository"
  # dnf 4 calls it --source, dnf 5 --srpm.
  cd /tmp
  dnf -q -y download --source breeze-core >/dev/null 2>&1 || dnf -q -y download --srpm breeze-core >/dev/null
  ls breeze-core-$VER-*.src.rpm >/dev/null
  rpm -K breeze-core-$VER-*.src.rpm | tee /tmp/k | sed "s/^/   /"
  grep -q "signatures OK" /tmp/k

  echo "-- dnf builddep, then rpmbuild --rebuild"
  dnf -q -y builddep breeze-core-$VER-*.src.rpm >/dev/null
  rpmbuild --rebuild --nocheck --define "_topdir /rb" breeze-core-$VER-*.src.rpm > /tmp/build.log 2>&1 \
    || { tail -30 /tmp/build.log; exit 1; }
  dnf -q -y install /rb/RPMS/*/breeze-core-$VER-*.rpm >/dev/null
  breeze-core --version | grep -q "breeze-core $VER"
  echo "   built and installed $(breeze-core --version | head -1)"
CASE
)"
run_case alma-src   almalinux:9 "$SRPM_CASE"
run_case fedora-src fedora:44   "$SRPM_CASE"

run_case arch-src archlinux:base '
  pacman -Syu --noconfirm --needed base-devel cargo >/dev/null 2>&1
  echo "-- the source tarball, found in the directory listing"
  f=$(curl -fsS "$BASE/arch/sources/" | grep -o "breeze-core-$VER-[0-9]*\.src\.tar\.gz" | sort -u | tail -1)
  [ -n "$f" ] || { echo "   !! no $VER source tarball listed at $BASE/arch/sources/"; exit 1; }
  cd /tmp && curl -fsSO "$BASE/arch/sources/$f" && curl -fsSO "$BASE/arch/sources/$f.sig"

  echo "-- its signature verifies against the repository key, and a tampered copy does not"
  curl -fsSL "$BASE/aspic.asc" | gpg --batch --quiet --import
  gpg --batch --verify "$f.sig" "$f" 2>&1 | grep -q "Good signature"
  cp "$f" tampered.tar.gz && printf x >> tampered.tar.gz
  if gpg --batch --verify "$f.sig" tampered.tar.gz >/dev/null 2>&1; then
    echo "   !! a tampered tarball verified"; exit 1
  fi
  echo "   $f: good signature; tampered copy refused"

  echo "-- makepkg, then pacman -U"
  useradd -m builder && mkdir /b && chown builder /b
  su builder -c "cd /b && tar -xf /tmp/$f && cd breeze-core && makepkg --nocheck --noconfirm" > /tmp/build.log 2>&1 \
    || { tail -30 /tmp/build.log; exit 1; }
  pacman -U --noconfirm /b/breeze-core/breeze-core-$VER-*-x86_64.pkg.tar.zst >/dev/null
  breeze-core --version | grep -q "breeze-core $VER"
  echo "   built and installed $(breeze-core --version | head -1)"
'

echo
echo "=== $pass passed, $fail failed ==="
[ "$fail" = 0 ] || exit 1
