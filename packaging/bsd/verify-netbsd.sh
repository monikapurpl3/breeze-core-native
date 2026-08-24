#!/usr/bin/env bash
# Install the NetBSD package on a real NetBSD machine and check what happened —
# including an upgrade straight over the published Python 3.2.0.
#
#   ./packaging/bsd/verify-netbsd.sh [user@host]
#
# This is destructive on that machine: it removes whatever breeze-core is
# installed, installs 3.2.0 from bolero, upgrades to the local build, and leaves
# the local build in place. It is a build VM; that is what it is for.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
TARGET="${1:-monika@192.168.122.130}"
VER="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
WORK="bcn-build"
OLD_URL="https://bolero.salataputarica.hr.eu.org/netbsd/All/breeze-core-3.2.0.tgz"

ssh "$TARGET" "set -eu
export PATH=\$PATH:/usr/pkg/sbin:/usr/sbin
PKG=\$HOME/$WORK/out/breeze-core-$VER.tgz
[ -f \"\$PKG\" ] || { echo 'no package — run packaging/bsd/build-netbsd.sh first'; exit 1; }

echo '-- starting from nothing'
doas service breeze_core stop >/dev/null 2>&1 || true
doas pkg_delete -f breeze-core >/dev/null 2>&1 || true
doas rm -rf /usr/pkg/breeze-core

echo '-- installing the published Python 3.2.0'
doas pkg_add '$OLD_URL' 2>&1 | tail -2
pkg_info | grep -i breeze

echo '-- marking the config so we can see whether it survives'
doas sh -c 'printf %s \"{\\\"api_key\\\":\\\"survives-the-bsd-upgrade\\\",\\\"units\\\":[]}\" > /usr/pkg/etc/breeze-core/config.json'

echo '-- upgrading in place, the way a BSD user would'
doas pkg_add -U \"\$PKG\" 2>&1 | tail -3
pkg_info | grep -i breeze

echo '-- the binary is ours'
/usr/pkg/bin/breeze-core --version

echo '-- no dependencies are declared'
deps=\$(pkg_info -n breeze-core | tail -n +2 | tr -d '[:space:]')
[ -z \"\$deps\" ] || { echo \"   !! it depends on: \$deps\"; exit 1; }
echo '   none, as intended'

echo '-- the configuration survived'
doas grep -q survives-the-bsd-upgrade /usr/pkg/etc/breeze-core/config.json
echo '   yes'

echo '-- the old Python tree is gone'
[ ! -d /usr/pkg/breeze-core ] || { echo '   !! /usr/pkg/breeze-core is still there'; exit 1; }
echo '   removed by the upgrade'

echo '-- the rc.d service starts, answers and stops'
grep -q '^breeze_core=YES' /etc/rc.conf 2>/dev/null || doas sh -c 'echo breeze_core=YES >> /etc/rc.conf'
doas rm -f /var/run/breeze_core.pid
doas service breeze_core start >/dev/null
ok=0
i=0
while [ \$i -lt 10 ]; do
  sleep 1
  if ftp -o - http://127.0.0.1:8420/api/health 2>/dev/null | grep -q status; then ok=1; break; fi
  i=\$((i+1))
done
ps -axo user,comm | grep -q '^breeze .*breeze-core' && echo '   running as the breeze user'
doas service breeze_core stop >/dev/null 2>&1 || true
[ \"\$ok\" = 1 ] || { echo '   !! it never answered'; doas tail -5 /var/log/breeze_core.log; exit 1; }
echo '   answered /api/health, then stopped'

echo 'ALL NETBSD CHECKS PASSED'
"
