#!/usr/bin/env bash
# Verify the .xbps packages by installing them in a real Void container.
#
#   ./packaging/xbps/verify-xbps.sh
#
# Installs for real — INSTALL script, account creation, permissions, the lot —
# then starts the server and talks to it. The glibc image is used on purpose:
# the binary is static musl, so if it runs here the "no runtime dependencies"
# claim is demonstrated rather than asserted.
#
# What this canNOT check, stated so it is not mistaken for covered: runit is
# not PID 1 in a container, so `sv up` has nothing to talk to. The run scripts
# are checked for syntax and for being executable, and the server is started
# the way the run script starts it, but "comes back after a reboot" is only
# ever provable on a real Void machine.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"

OUT="packaging/out/xbps"
VERSION="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
IMAGE="ghcr.io/void-linux/void-glibc-full:latest"

[ -d "$OUT" ] || { echo "no packages — run packaging/xbps/build-xbps.sh first"; exit 1; }
ls "$OUT"/breeze-core-*.x86_64.xbps >/dev/null 2>&1 || {
  echo "no x86_64 package in $OUT — run packaging/xbps/build-xbps.sh amd64"; exit 1; }

MOUNT="$REPO"
case "$MOUNT" in /[a-z]/*) MOUNT="$(echo "$MOUNT" | sed -E 's#^/([a-z])/#\U\1:/#')" ;; esac
export MSYS_NO_PATHCONV=1

docker run --rm -i \
  -v "$MOUNT/$OUT:/pkgs-ro:ro" \
  -e VERSION="$VERSION" \
  "$IMAGE" sh -eu <<'VOID'
fail=0
ok()   { printf '  ok    %s\n' "$1"; }
bad()  { printf '  FAIL  %s\n' "$1"; fail=$((fail+1)); }
check(){ if eval "$2" >/dev/null 2>&1; then ok "$1"; else bad "$1"; fi; }

# curl is a TEST dependency only — it is not a dependency of the package, and
# the package genuinely has none. The image ships no HTTP client at all.
# (It also ships no bash, which is why nothing below calls ldd: ldd is a
# #!/bin/bash script, and dash reports a missing interpreter as
# "ldd: not found", which reads like a missing tool rather than a missing shell.)
echo "=== 0. test tooling"
xbps-install -Sy curl >/tmp/curl.log 2>&1 || { sed 's/^/    /' /tmp/curl.log; exit 1; }
ok "curl $(curl --version | head -1 | cut -d' ' -f2) installed (test-only)"

# The mount is read-only and xbps-rindex writes repodata next to the packages.
mkdir -p /pkgs && cp /pkgs-ro/*.xbps /pkgs/

echo
echo "=== 1. repository index"
# Once per architecture. xbps-rindex indexes only packages matching XBPS_ARCH
# and says "ignoring …, unmatched arch" for the rest, so a single run over a
# directory holding both libc flavours silently indexes only the host's. Both
# repodata files land in the same directory, which is exactly how Void's own
# repository is laid out.
for a in x86_64 x86_64-musl; do
  XBPS_ARCH="$a" xbps-rindex -a /pkgs/*.xbps 2>&1 | sed "s/^/    [$a] /"
done
check "x86_64-repodata written"      "[ -s /pkgs/x86_64-repodata ]"
check "x86_64-musl-repodata written" "[ -s /pkgs/x86_64-musl-repodata ]"

echo
echo "=== 2. declared metadata"
xbps-query --repository=/pkgs -S breeze-core > /tmp/meta
sed 's/^/    /' /tmp/meta
check "version is $VERSION"     "grep -q '^pkgver: breeze-core-$VERSION' /tmp/meta"
check "architecture is x86_64"  "grep -qx 'architecture: x86_64' /tmp/meta"
# The headline claim of the whole rewrite. If a dependency ever appears here,
# something started linking dynamically and the package quietly stopped being
# installable on a minimal system.
check "NO run-time dependencies" "! grep -q '^run_depends' /tmp/meta"
# conf_files puts each path on its own tab-indented line UNDER the key, so this
# has to look at the following line rather than the key's own.
check "env file marked as config" "grep -A1 '^conf_files:' /tmp/meta | grep -q '/etc/breeze-core/breeze-core.env'"

echo
echo "=== 3. INSTALL/REMOVE are scripts, not installed files"
xbps-query --repository=/pkgs -f breeze-core > /tmp/files
check "no /INSTALL in the file list" "! grep -qx '/INSTALL' /tmp/files"
check "no /REMOVE in the file list"  "! grep -qx '/REMOVE' /tmp/files"

echo
echo "=== 4. install for real"
xbps-install --repository=/pkgs -y breeze-core 2>&1 | sed 's/^/    /'
check "binary installed"            "[ -x /usr/bin/breeze-core ]"
check "breeze account created"      "getent passwd breeze"
check "breeze group created"        "getent group breeze"
check "nologin shell exists"        "[ -x \"\$(getent passwd breeze | cut -d: -f7)\" ]"
check "runit run script"            "[ -x /etc/sv/breeze-core/run ]"
check "runit log run script"        "[ -x /etc/sv/breeze-core/log/run ]"
check "supervise is a symlink"      "[ -L /etc/sv/breeze-core/supervise ]"
check "supervise points into /run"  "[ \"\$(readlink /etc/sv/breeze-core/supervise)\" = /run/runit/supervise/breeze-core ]"
check "service NOT auto-enabled"    "[ ! -e /var/service/breeze-core ]"
check "licence installed"           "[ -f /usr/share/licenses/breeze-core/LICENSE ]"

echo
echo "=== 5. permissions (the ones that break pairing when wrong)"
stat -c '    %U:%G %a  %n' /etc/breeze-core /etc/breeze-core/breeze-core.env /var/log/breeze-core
check "/etc/breeze-core is breeze:breeze" "[ \"\$(stat -c '%U:%G' /etc/breeze-core)\" = 'breeze:breeze' ]"
check "/etc/breeze-core is 750"           "[ \"\$(stat -c '%a' /etc/breeze-core)\" = '750' ]"
check "env file is root:breeze"           "[ \"\$(stat -c '%U:%G' /etc/breeze-core/breeze-core.env)\" = 'root:breeze' ]"
check "env file is 640"                   "[ \"\$(stat -c '%a' /etc/breeze-core/breeze-core.env)\" = '640' ]"
check "log dir owned by breeze"           "[ \"\$(stat -c '%U' /var/log/breeze-core)\" = 'breeze' ]"

echo
echo "=== 6. the static claim, on a glibc host"
# A dynamically linked ELF carries its interpreter path as a literal string
# (/lib64/ld-linux-x86-64.so.2, or /lib/ld-musl-*.so.1). Grepping for it needs
# no tools at all, which matters in an image with no ldd, no file and no
# readelf. Calibrated against a known-dynamic binary in the same image so that
# a passing result cannot just mean "grep found nothing anywhere".
if grep -qa 'ld-linux\|ld-musl' /usr/bin/xbps-install; then
  ok "control: xbps-install is detected as dynamic"
else
  bad "control: the loader-string test found nothing in a dynamic binary — test is broken, ignore the next line"
fi
check "breeze-core has no ELF interpreter" "! grep -qa 'ld-linux\|ld-musl' /usr/bin/breeze-core"
check "--version says $VERSION"            "/usr/bin/breeze-core --version | grep -q '$VERSION'"

echo
echo "=== 7. run scripts are valid shell"
check "run parses"     "sh -n /etc/sv/breeze-core/run"
check "log/run parses" "sh -n /etc/sv/breeze-core/log/run"

echo
echo "=== 8. it actually serves"
# A minimal config, because the server refuses to start without an api_key --
# deliberately, since a server with no key can only produce confusing 401s.
printf '%s' '{"api_key":"verify-xbps","units":[]}' > /etc/breeze-core/config.json
chown breeze:breeze /etc/breeze-core/config.json
chmod 640 /etc/breeze-core/config.json

# Started as the breeze user, the way the run script does, to prove the
# permissions above are sufficient for the service and not merely for root.
su breeze -s /bin/sh -c 'AC_CONFIG_DIR=/etc/breeze-core /usr/bin/breeze-core serve --host 127.0.0.1 --port 8420' \
  > /tmp/serve.log 2>&1 &
srv=$!
i=0
while [ $i -lt 60 ]; do
  curl -fsS --max-time 1 http://127.0.0.1:8420/api/health >/dev/null 2>&1 && break
  i=$((i+1)); sleep 0.25
done

health="$(curl -fsS --max-time 3 http://127.0.0.1:8420/api/health 2>/dev/null || true)"
printf '    health: %s\n' "${health:-<no response>}"
check "/api/health answers"    "[ -n \"\$health\" ]"
check "health body says ok"    "printf '%s' \"\$health\" | grep -q 'ok\|healthy\|status'"
# An unauthenticated control path must still be refused -- worth one line here
# because a packaging change that broke it would look like a working install.
code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 http://127.0.0.1:8420/api/units || true)"
printf '    GET /api/units without credentials -> %s\n' "$code"
check "/api/units is 401 without credentials" "[ \"\$code\" = 401 ]"

# Writing devices.json is the operation that fails when /etc/breeze-core is
# owned by root -- the exact bug nfpm's archlinux packager caused. Proving the
# service user can write there is the point of this step.
check "service user can write the store dir" "su breeze -s /bin/sh -c 'touch /etc/breeze-core/.wtest && rm -f /etc/breeze-core/.wtest'"

kill $srv 2>/dev/null || true
wait $srv 2>/dev/null || true
echo "    --- server log ---"
sed 's/^/    /' /tmp/serve.log | head -8

echo
echo "=== 9. the glibc/musl split actually separates them"
# The mechanism is repodata, not a check inside the package: a host reads
# <its-arch>-repodata and never learns the other flavour exists. Asserted in
# both directions, because "not found" on its own could just as easily mean the
# index was empty.
g="$(xbps-query --repository=/pkgs -s breeze-core 2>/dev/null || true)"
m="$(XBPS_ARCH=x86_64-musl xbps-query --repository=/pkgs -s breeze-core 2>/dev/null || true)"
check "a glibc host sees exactly one candidate" "[ \"\$(printf '%s\n' \"\$g\" | grep -c breeze-core)\" = 1 ]"
check "a musl host sees exactly one candidate"  "[ \"\$(printf '%s\n' \"\$m\" | grep -c breeze-core)\" = 1 ]"
check "the glibc host's candidate is x86_64" \
  "[ \"\$(xbps-query --repository=/pkgs -S breeze-core | grep '^architecture:')\" = 'architecture: x86_64' ]"
check "the musl host's candidate is x86_64-musl" \
  "[ \"\$(XBPS_ARCH=x86_64-musl xbps-query --repository=/pkgs -S breeze-core | grep '^architecture:')\" = 'architecture: x86_64-musl' ]"

echo
echo "=== 10. removal keeps the configuration"
xbps-remove -y breeze-core 2>&1 | sed 's/^/    /'
check "binary removed"        "[ ! -e /usr/bin/breeze-core ]"
check "config directory KEPT" "[ -d /etc/breeze-core ]"
check "config.json KEPT"      "[ -f /etc/breeze-core/config.json ]"

echo
if [ "$fail" -eq 0 ]; then
  echo "=== all checks passed"
else
  echo "=== $fail CHECK(S) FAILED"
  exit 1
fi
VOID
