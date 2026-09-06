#!/usr/bin/env bash
# Test site/migrate.sh the way it will actually be used: on a machine that has
# the bolero repository configured and the Python breeze-core installed from it.
#
#   ./packaging/repo/verify-migrate.sh              # every distro below
#   ./packaging/repo/verify-migrate.sh debian
#
# Each case builds the real "before" state - bolero's key and repository, then
# `install breeze-core`, which lands 3.2.0 - drops a marker into config.json,
# and only then runs the migration against the live aspic. A migration script
# tested against a fresh machine tests nothing: the entire job is removing
# something that is already there.
#
# It checks the dry run first, and asserts that it changed nothing. That is the
# mode people will use, and a "plan" that quietly acts is worse than no plan.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
VER="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
ASPIC="${ASPIC_URL:-https://aspic.salataputarica.hr.eu.org}"

MOUNT="$REPO"
case "$MOUNT" in /[a-z]/*) MOUNT="$(echo "$MOUNT" | sed -E 's#^/([a-z])/#\U\1:/#')" ;; esac
export MSYS_NO_PATHCONV=1

pass=0; fail=0
report() {
  if [ "$1" = 0 ]; then printf '  \033[32mPASS\033[0m  %s\n' "$2"; pass=$((pass+1))
  else printf '  \033[31mFAIL\033[0m  %s\n' "$2"; fail=$((fail+1)); fi
}

want=("$@")
selected() {
  [ ${#want[@]} -eq 0 ] && return 0
  for w in "${want[@]}"; do [ "$w" = "$1" ] && return 0; done
  return 1
}

# The assertions every case makes after migrating. Kept in one place so a check
# cannot quietly exist for Debian only.
COMMON=$(cat <<'ASSERT'
echo "-- breeze-core is the native version"
test "$(breeze-core --version | head -1 | awk '{print $2}')" = "@VER@"

echo "-- and it is the same package name, upgraded in place"
test -x /usr/bin/breeze-core

echo "-- the configuration survived"
grep -q survives-the-migration /etc/breeze-core/config.json

echo "-- and so did the other stores"
test -f /etc/breeze-core/devices.json
grep -q kept-too /etc/breeze-core/devices.json

echo "-- a backup exists, root-only, with a rollback script in it"
b=$(ls -d /var/backups/breeze-core-migration-* 2>/dev/null | head -1)
test -n "$b"
test "$(stat -c %a "$b")" = 700
test -x "$b/ROLLBACK.sh"
test -f "$b/manifest.txt"
grep -q survives-the-migration "$b/etc/breeze-core/config.json"

echo "-- the backup names where it came from and where it went"
grep -q "from        breeze-core 3" "$b/manifest.txt"
grep -q "to          breeze-core @VER@" "$b/manifest.txt"

echo "-- and a tarball beside it"
ls /var/backups/breeze-core-migration-*.tar.gz >/dev/null

echo "ASSERTIONS PASSED"
ASSERT
)
assertions() { printf '%s' "$COMMON" | sed "s/@VER@/$VER/g"; }

run_case() {
  local name="$1" image="$2" script="$3"
  selected "$name" || return 0
  echo
  echo "=== $name ($image)"
  [ -n "${MIGRATE_DEBUG:-}" ] && printf '%s' "$script" > "/tmp/dbg-migrate-$name.sh"
  if printf '%s' "$script" \
     | timeout 1200 docker run --rm -i -v "$MOUNT/site:/site:ro" \
         -e ASPIC_URL="$ASPIC" -e VER="$VER" "$image" sh -eu -s 2>&1 \
     | sed 's/^/    /'; then
    report 0 "$name"
  else
    report 1 "$name"
  fi
}

# Common preamble for every case: put the machine into the state a real user is
# in before migrating.
before_state() { printf '%s' "
echo '-- installing the bolero repository and the Python 3.2.0 package'
$1
echo '-- leaving markers in the stores'
printf %s '{\"api_key\":\"survives-the-migration\",\"units\":[]}' > /etc/breeze-core/config.json
printf %s '{\"devices\":[\"kept-too\"]}' > /etc/breeze-core/devices.json
breeze-core version 2>/dev/null | head -1 || true
"; }

# --- apt --------------------------------------------------------------------
run_case debian debian:12 "
  apt-get -qq update >/dev/null && apt-get -qq install -y curl gnupg ca-certificates >/dev/null
  $(before_state "
    curl -fsSL https://bolero.salataputarica.hr.eu.org/breeze-core.asc | gpg --dearmor -o /usr/share/keyrings/breeze-core.gpg
    echo 'deb [signed-by=/usr/share/keyrings/breeze-core.gpg] https://bolero.salataputarica.hr.eu.org/deb stable main' > /etc/apt/sources.list.d/breeze-core.list
    apt-get -qq update >/dev/null
    apt-get -qq install -y breeze-core >/dev/null 2>&1
  ")

  echo '-- the dry run must change nothing'
  sh /site/migrate.sh > /tmp/plan.txt 2>&1
  grep -q 'this was a plan, not a migration' /tmp/plan.txt
  grep -q 'breeze-core 3' /tmp/plan.txt
  test -f /etc/apt/sources.list.d/breeze-core.list
  test ! -f /etc/apt/sources.list.d/aspic.list
  echo '   plan printed, nothing touched'

  echo '-- migrating'
  sh /site/migrate.sh --yes > /tmp/mig.txt 2>&1 || { tail -25 /tmp/mig.txt; exit 1; }

  echo '-- bolero is gone'
  test ! -f /etc/apt/sources.list.d/breeze-core.list
  test ! -f /usr/share/keyrings/breeze-core.gpg
  echo '-- aspic is in place, and apt trusts it'
  test -f /etc/apt/sources.list.d/aspic.list
  test -f /usr/share/keyrings/aspic.gpg
  apt-get -qq update >/dev/null
  $(assertions)
"

# --- dnf --------------------------------------------------------------------
run_case alma almalinux:9 "
  dnf -q -y install curl >/dev/null 2>&1 || true
  $(before_state "
    rpm --import https://bolero.salataputarica.hr.eu.org/breeze-core.asc
    curl -fsSL -o /etc/yum.repos.d/breeze-core.repo https://bolero.salataputarica.hr.eu.org/rpm/breeze-core.repo
    dnf -q -y install breeze-core >/dev/null 2>&1
  ")

  echo '-- the dry run must change nothing'
  sh /site/migrate.sh > /tmp/plan.txt 2>&1
  grep -q 'this was a plan, not a migration' /tmp/plan.txt
  test -f /etc/yum.repos.d/breeze-core.repo
  echo '   plan printed, nothing touched'

  echo '-- migrating'
  sh /site/migrate.sh --yes > /tmp/mig.txt 2>&1 || { tail -25 /tmp/mig.txt; exit 1; }

  echo '-- bolero is gone, key and all'
  test ! -f /etc/yum.repos.d/breeze-core.repo
  rpm -q gpg-pubkey --qf '%{SUMMARY}\n' | grep -qi 'breeze core repository' && { echo 'the bolero key is still trusted'; exit 1; } || true
  echo '-- aspic is in place'
  test -f /etc/yum.repos.d/aspic.repo
  rpm -q gpg-pubkey --qf '%{SUMMARY}\n' | grep -qi aspic
  $(assertions)
"

# --- pacman -----------------------------------------------------------------
run_case arch archlinux:base "
  pacman -Sy --noconfirm --quiet curl >/dev/null 2>&1 || true
  pacman-key --init >/dev/null 2>&1
  $(before_state "
    curl -fsSL -o /tmp/b.asc https://bolero.salataputarica.hr.eu.org/breeze-core.asc
    pacman-key --add /tmp/b.asc >/dev/null 2>&1
    pacman-key --lsign-key \"\$(gpg --show-keys --with-colons /tmp/b.asc | awk -F: '/^fpr/{print \$10; exit}')\" >/dev/null 2>&1
    { echo ''; echo '[breeze-core]'; echo 'SigLevel = Required'; echo 'Server = https://bolero.salataputarica.hr.eu.org/arch/\$arch'; } >> /etc/pacman.conf
    pacman -Sy --noconfirm breeze-core >/dev/null 2>&1
  ")

  echo '-- migrating'
  sh /site/migrate.sh --yes > /tmp/mig.txt 2>&1 || { tail -25 /tmp/mig.txt; exit 1; }

  echo '-- the [breeze-core] stanza is gone and [aspic] is there'
  grep -q '^\[breeze-core\]' /etc/pacman.conf && { echo 'stanza still present'; exit 1; } || true
  grep -q '^\[aspic\]' /etc/pacman.conf
  echo '-- and pacman.conf is otherwise intact'
  grep -q '^\[core\]' /etc/pacman.conf
  test -f /etc/pacman.conf.pre-aspic
  $(assertions)
"

# --- apk --------------------------------------------------------------------
run_case alpine alpine:3.20 "
  apk add --no-cache curl >/dev/null
  $(before_state "
    curl -fsSL -o /etc/apk/keys/breeze-core@bolero.rsa.pub https://bolero.salataputarica.hr.eu.org/alpine/breeze-core@bolero.rsa.pub
    echo 'https://bolero.salataputarica.hr.eu.org/alpine' >> /etc/apk/repositories
    apk update >/dev/null
    apk add breeze-core >/dev/null 2>&1
  ")

  echo '-- migrating'
  sh /site/migrate.sh --yes > /tmp/mig.txt 2>&1 || { tail -25 /tmp/mig.txt; exit 1; }

  echo '-- bolero is out of the repositories file and its key is gone'
  grep -q bolero /etc/apk/repositories && { echo 'bolero still listed'; exit 1; } || true
  test ! -f '/etc/apk/keys/breeze-core@bolero.rsa.pub'
  echo '-- aspic is in, with its key'
  grep -q 'aspic' /etc/apk/repositories
  test -f /etc/apk/keys/aspic-alpine.rsa.pub
  $(assertions)
"

echo
echo "=== $pass passed, $fail failed ==="
[ "$fail" = 0 ] || exit 1
