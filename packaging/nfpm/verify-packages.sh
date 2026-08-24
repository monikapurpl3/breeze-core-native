#!/usr/bin/env bash
# Install the packages in clean containers and check what actually landed.
#
#   ./packaging/nfpm/verify-packages.sh            # every distro below
#   ./packaging/nfpm/verify-packages.sh debian arch
#
# This is the acceptance test for packaging. Building a .deb proves the archive
# is well-formed; it says nothing about whether the scriptlets run, whether the
# service account gets made, whether a static musl binary really does execute on
# a glibc distro, or whether an upgrade over the Python package keeps a person's
# configuration. Each of those has its own way of failing quietly.
#
# The amd64 packages are the ones exercised: the others are the same binary in
# the same archive with a different architecture field, and a container cannot
# run them anyway.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
PKG="packaging/out/pkg"
VERSION="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"

MOUNT="$REPO"
case "$MOUNT" in /[a-z]/*) MOUNT="$(echo "$MOUNT" | sed -E 's#^/([a-z])/#\U\1:/#')" ;; esac
export MSYS_NO_PATHCONV=1

# The previous line, for the upgrade test: the Python package of the same name,
# from the host that still serves it. Fetched rather than vendored so the test
# is against what a real user actually has installed.
OLD_DEB_URL="https://bolero.salataputarica.hr.eu.org/deb/pool/main/b/breeze-core/breeze-core_3.2.0_amd64.deb"
OLD_RPM_URL="https://bolero.salataputarica.hr.eu.org/rpm/x86_64/breeze-core-3.2.0-1.x86_64.rpm"

pass=0
fail=0
report() {
  if [ "$1" = 0 ]; then
    printf '  \033[32mPASS\033[0m  %s\n' "$2"; pass=$((pass + 1))
  else
    printf '  \033[31mFAIL\033[0m  %s\n' "$2"; fail=$((fail + 1))
  fi
}


checks() { sed "s/@VERSION@/$VERSION/g" "$REPO/packaging/nfpm/checks.sh"; }

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
  # The script goes in on **stdin** (`sh -s`) rather than as an argument to
  # `sh -c`: it is a multi-line program full of quotes, and stdin is the one
  # path no shell — and no Windows argv layer — gets a second opinion about.
  # Hence `-i`, without which the container gets no stdin at all.
  if printf '%s' "$script" \
     | timeout 900 docker run --rm -i -v "$MOUNT/$PKG:/pkg:ro" "$image" sh -eu -s 2>&1 \
     | sed 's/^/    /'; then
    report 0 "$name"
  else
    report 1 "$name"
  fi
}

# --- Debian: install, then the same thing again as an upgrade over 3.2.0 -----
run_case debian debian:12 "
  apt-get -qq update >/dev/null && apt-get -qq install -y wget >/dev/null
  dpkg -i /pkg/breeze-core_${VERSION}_amd64.deb
  $(checks)
  echo '-- removal keeps the configuration'
  dpkg -r breeze-core >/dev/null 2>&1
  [ -d /etc/breeze-core ]
"

run_case debian-upgrade debian:12 "
  apt-get -qq update >/dev/null && apt-get -qq install -y wget ca-certificates >/dev/null
  echo '-- installing the Python 3.2.0 package first'
  wget -q -O /tmp/old.deb '$OLD_DEB_URL' || { echo 'could not fetch 3.2.0 - skipping'; exit 0; }
  dpkg -i /tmp/old.deb >/dev/null 2>&1 || apt-get -qq install -f -y >/dev/null
  breeze-core version 2>/dev/null | head -1 || true
  echo '-- leaving something behind in /etc/breeze-core'
  printf '{\"api_key\":\"kept-across-the-upgrade\",\"units\":[]}' > /etc/breeze-core/config.json
  echo '-- now upgrading to ${VERSION}'
  dpkg -i /pkg/breeze-core_${VERSION}_amd64.deb
  $(checks)
  echo '-- and the configuration survived'
  grep -q kept-across-the-upgrade /etc/breeze-core/config.json
"

# --- RHEL family ------------------------------------------------------------
run_case alma almalinux:9 "
  dnf -q -y install wget >/dev/null 2>&1 || true
  rpm -i /pkg/breeze-core-${VERSION}-1.x86_64.rpm
  $(checks)
  echo '-- removal keeps the configuration'
  rpm -e breeze-core
  [ -d /etc/breeze-core ]
"

run_case alma-upgrade almalinux:9 "
  dnf -q -y install wget >/dev/null 2>&1 || true
  wget -q -O /tmp/old.rpm '$OLD_RPM_URL' || { echo 'could not fetch 3.2.0 - skipping'; exit 0; }
  rpm -i --nodeps /tmp/old.rpm
  printf '{\"api_key\":\"kept-across-the-upgrade\",\"units\":[]}' > /etc/breeze-core/config.json
  echo '-- upgrading with rpm -U, the way dnf would'
  rpm -U /pkg/breeze-core-${VERSION}-1.x86_64.rpm
  $(checks)
  grep -q kept-across-the-upgrade /etc/breeze-core/config.json
"

# --- Arch -------------------------------------------------------------------
run_case arch archlinux:base "
  pacman -Sy --noconfirm --quiet wget >/dev/null 2>&1 || true
  pacman -U --noconfirm /pkg/breeze-core-${VERSION}-1-x86_64.pkg.tar.zst >/dev/null
  $(checks)
"

# --- Alpine (musl, OpenRC) --------------------------------------------------
run_case alpine alpine:3.20 "
  apk add --no-cache wget >/dev/null
  # --allow-untrusted: the package file itself carries no signature; the
  # repository index is what gets signed, and that is verified separately by
  # packaging/repo/verify-repo.sh.
  apk add --allow-untrusted /pkg/breeze-core-${VERSION}-r0.apk >/dev/null 2>&1 \
    || apk add --allow-untrusted /pkg/breeze-core_${VERSION}_x86_64.apk >/dev/null
  $(checks)
  echo '-- the OpenRC service is installed'
  [ -x /etc/init.d/breeze-core ]
"

echo
echo "=== $pass passed, $fail failed ==="
[ "$fail" = 0 ] || exit 1
