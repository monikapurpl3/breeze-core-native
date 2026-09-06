#!/usr/bin/env bash
# Install the OpenBSD package on a real OpenBSD machine, and check that pkg_add
# REFUSES it when the signing key is not trusted.
#
#   ./packaging/bsd/verify-openbsd.sh [user@host]
#
# Destructive on that machine. It is a build VM.
#
# Note the pkgspecs: OpenBSD's `pkg_info -e` takes a spec, not a stem, so it
# wants "breeze-core-*" and answers "Invalid spec: breeze-core" to anything
# less. Same for the other queries here.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
TARGET="${1:-monika@192.168.122.128}"
VER="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"

# A glob, not a fixed path: build-openbsd.sh stages into <rel>/packages/<arch>/
# so that the published tree is the layout pkg_add PKG_PATH expects.
PKG_LOCAL="$(find packaging/out/bsd/openbsd -name "breeze-core-$VER.tgz" | head -1)"
[ -n "$PKG_LOCAL" ] || { echo "no breeze-core-$VER.tgz staged - run packaging/bsd/build-openbsd.sh"; exit 1; }
scp -q "$PKG_LOCAL" "$TARGET:/tmp/"
scp -q packaging/repo/keys/aspic-openbsd.pub "$TARGET:/tmp/aspic-pkg.pub"

# The remote script is a quoted heredoc, so nothing here is expanded twice.
# $VER is passed in through the environment instead.
ssh "$TARGET" "VER='$VER' sh -s" <<'REMOTE'
set -eu
PKG=/tmp/breeze-core-$VER.tgz

echo '-- starting from nothing'
doas rcctl stop breeze_core >/dev/null 2>&1 || true
doas pkg_delete breeze-core >/dev/null 2>&1 || true
doas rm -f /etc/signify/aspic-pkg.pub
# Leftovers from the Python line's SOURCE install: pkg_add refuses to overwrite
# files that no package owns, so without this the test fails for a reason that
# has nothing to do with what it is testing.
doas rm -rf /usr/local/breeze-core /usr/local/bin/breeze-core /etc/rc.d/breeze_core

echo '-- without the key in /etc/signify, pkg_add must refuse it'
if doas pkg_add "$PKG" >/tmp/out 2>&1; then
  echo '   !! pkg_add installed a package signed by a key it does not trust'
  tail -3 /tmp/out
  exit 1
fi
echo '   refused, and this is what it said:'
sed 's/^/     /' /tmp/out | tail -3

echo '-- with the key installed, it goes in'
doas cp /tmp/aspic-pkg.pub /etc/signify/aspic-pkg.pub
doas pkg_add "$PKG" 2>&1 | tail -3
pkg_info -e 'breeze-core-*' >/dev/null

echo '-- what landed'
pkg_info -L breeze-core | tail -6
test -x /usr/local/bin/breeze-core
test -x /etc/rc.d/breeze_core
/usr/local/bin/breeze-core --version | head -1

echo '-- the service account came from the packing list'
id breeze >/dev/null && echo "   $(id breeze)"

echo '-- the config directory is a sample, owned by the service'
doas ls -ld /etc/breeze-core
doas test -f /etc/breeze-core/breeze-core.env && echo '   breeze-core.env seeded'

echo '-- no dependencies'
deps=$(pkg_info -f breeze-core | grep -c '^@depend' || true)
[ "$deps" = 0 ] || { echo "   !! it has $deps dependencies"; exit 1; }
echo '   none, as intended'

echo '-- the service starts, answers and stops'
doas sh -c 'test -f /etc/breeze-core/config.json || printf %s "{\"api_key\":\"openbsd-verification\",\"units\":[]}" > /etc/breeze-core/config.json'
doas chown -R breeze:breeze /etc/breeze-core
doas rcctl enable breeze_core
doas rcctl start breeze_core >/dev/null 2>&1 || true
ok=0
i=0
while [ $i -lt 12 ]; do
  sleep 1
  if ftp -o - http://127.0.0.1:8420/api/health 2>/dev/null | grep -q status; then ok=1; break; fi
  i=$((i+1))
done
# Identity, not just liveness: on FreeBSD an old Python service answered
# /api/health and made a failed start look like a success.
ps -axo user,args | grep '[b]reeze-core serve' | head -1
doas rcctl check breeze_core
doas rcctl stop breeze_core >/dev/null 2>&1 || true
[ "$ok" = 1 ] || { echo '   !! it never answered'; exit 1; }
echo '   answered /api/health, then stopped'

echo '-- deinstall keeps the configuration'
doas pkg_delete breeze-core >/dev/null 2>&1
test ! -x /usr/local/bin/breeze-core
doas test -f /etc/breeze-core/config.json
echo '   binary gone, /etc/breeze-core kept'

echo 'ALL OPENBSD CHECKS PASSED'
REMOTE
