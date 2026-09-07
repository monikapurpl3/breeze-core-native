#!/usr/bin/env bash
# Install from the FreeBSD repository on a real FreeBSD machine, with signature
# checking on - and check that it FAILS without the key.
#
#   ./packaging/bsd/verify-freebsd.sh [user@host] [base-url]
#
# Destructive on that machine: it removes whatever breeze-core is installed and
# leaves the aspic one in its place. It is a build VM.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
TARGET="${1:-monika@192.168.122.131}"
BASE="${2:-${ASPIC_URL:-https://aspic.salataputarica.hr.eu.org}}"
VER="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"

ssh "$TARGET" "set -eu
BASE='$BASE'
VER='$VER'

echo '-- starting from nothing'
doas service breeze_core stop >/dev/null 2>&1 || true
doas pkg delete -y breeze-core >/dev/null 2>&1 || true
doas rm -f /usr/local/etc/pkg/repos/aspic.conf /usr/local/etc/pkg/keys/aspic-freebsd.pub
doas rm -rf /usr/local/breeze-core
# 'Nothing' has to include pkg's OWN state, or the negative test below proves
# nothing at all. A previous run leaves a validly-signed catalogue in
# /var/db/pkg/repo-aspic.sqlite and the package itself in /var/cache/pkg -- and
# with those present, a pkg install after a REJECTED update still succeeds,
# resolving from the old catalogue and installing from the cache. That is not a
# hole (both were verified when they were fetched), but it means the check has
# to run against a machine that has never seen this repository.
# Both layouts: pkg 2.x keeps a catalogue DIRECTORY per repository under
# /var/db/pkg/repos/, older pkg a single repo-<name>.sqlite. Removing only the
# legacy path silently clears nothing, and then the stale-but-valid catalogue
# makes the next update answer 'aspic repository is up to date' without
# fetching -- so no signature is checked and the negative test reads as a pass
# for the wrong reason.
doas rm -rf /var/db/pkg/repos/aspic /var/db/pkg/repo-aspic.sqlite
doas pkg clean -ay >/dev/null 2>&1 || true

echo '-- a repository whose key we do not have must be refused'
doas mkdir -p /usr/local/etc/pkg/repos /usr/local/etc/pkg/keys
# One backslash per quote, not three. This heredoc is written by the LOCAL
# shell inside a double-quoted string and then read by the REMOTE shell as an
# unquoted here-document - and an unquoted here-document only treats a
# backslash as an escape before $, a backtick, another backslash or a newline.
# Before a quote it keeps the backslash, so the extra pair landed literal
# backslashes in the file, and pkg answered that https was an invalid scheme -
# a message that sends you looking at the URL rather than at the quoting.
doas sh -c \"cat > /usr/local/etc/pkg/repos/aspic.conf\" <<CONF
aspic: {
  url: \"\$BASE/freebsd\",
  signature_type: \"pubkey\",
  pubkey: \"/usr/local/etc/pkg/keys/aspic-freebsd.pub\",
  enabled: yes
}
CONF
# A key that is real but wrong: the repository is signed by aspic's FreeBSD key,
# so pointing pkg at any other public key must fail the signature check. (No
# key file at all makes pkg complain about the configuration rather than the
# signature, which proves less.)
doas sh -c 'openssl genrsa -out /tmp/wrong.rsa 2048 2>/dev/null; openssl rsa -in /tmp/wrong.rsa -pubout -out /usr/local/etc/pkg/keys/aspic-freebsd.pub 2>/dev/null'
# The test is what pkg SAYS and what it ends up installing, not what it exits
# with: pkg update rejects the signature, drops the repository, then prints
# 'aspic is up to date' about the repository it just dropped and exits 0. An
# exit-status test therefore reads that as acceptance.
doas pkg -r / update -f -r aspic >/tmp/out 2>&1 || true
if ! grep -qi 'invalid signature' /tmp/out; then
  echo '   !! pkg did not reject the signature'; tail -5 /tmp/out; exit 1
fi
grep -i 'invalid signature' /tmp/out | head -1 | sed 's/^/     /'
# And the outcome that actually matters: nothing gets installed from it.
if doas pkg install -y -r aspic breeze-core >/tmp/out2 2>&1; then
  echo '   !! it installed a package from a repository it could not verify'
  tail -3 /tmp/out2; exit 1
fi
echo '   refused, and nothing was installed'

echo '-- with the right key, it updates and installs'
doas fetch -q -o /usr/local/etc/pkg/keys/aspic-freebsd.pub \"\$BASE/freebsd/aspic-freebsd.pub\"
doas pkg update -r aspic >/dev/null
doas pkg install -y -r aspic breeze-core >/dev/null
pkg info breeze-core | head -2
test \"\$(pkg query %v breeze-core)\" = \"\$VER\"
echo \"   installed \$(breeze-core --version | head -1) from the signed repository\"

echo '-- no dependencies are declared'
deps=\$(pkg info -d breeze-core | tail -n +2 | tr -d '[:space:]')
[ -z \"\$deps\" ] || { echo \"   !! it depends on: \$deps\"; exit 1; }
echo '   none, as intended'

echo '-- the rc.d service starts, answers and stops'
doas sysrc breeze_core_enable=YES >/dev/null
doas mkdir -p /usr/local/etc/breeze-core
doas sh -c 'test -f /usr/local/etc/breeze-core/config.json || printf %s \"{\\\"api_key\\\":\\\"freebsd-verification\\\",\\\"units\\\":[]}\" > /usr/local/etc/breeze-core/config.json'
doas chown -R breeze:breeze /usr/local/etc/breeze-core
doas rm -f /var/run/breeze_core.pid
doas service breeze_core start >/dev/null
ok=0
i=0
while [ \$i -lt 12 ]; do
  sleep 1
  if fetch -qo - http://127.0.0.1:8420/api/health 2>/dev/null | grep -q status; then ok=1; break; fi
  i=\$((i+1))
done
# Identity, not just liveness: an old Python service left running on this box
# answered /api/health once and made a failed start look like a success.
ps -axo user,command | grep -q '^breeze */usr/local/bin/breeze-core serve' || {
  echo '   !! nothing of ours is running - something else answered'; exit 1; }
doas service breeze_core status
doas service breeze_core stop >/dev/null 2>&1 || true
[ \"\$ok\" = 1 ] || { echo '   !! it never answered'; doas tail -5 /var/log/breeze_core.log; exit 1; }
echo '   answered /api/health as the breeze user, then stopped'

echo 'ALL FREEBSD CHECKS PASSED'
"
