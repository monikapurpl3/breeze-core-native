#!/usr/bin/env bash
# Verify the Gentoo overlay by emerging it in a real Gentoo container.
#
#   ./packaging/portage/verify-portage.sh
#
# Three separate things get checked, because each can be wrong on its own:
#
#   1. the Manifest hashes really are the hashes of OUR tarballs (computed
#      here, independently of portage, so a Manifest built from a downloaded
#      file would show up as a mismatch);
#   2. the ebuilds parse and their metadata generates, which is what catches a
#      bad inherit or a malformed SRC_URI;
#   3. `emerge` installs it and the result works — files in the right places,
#      the service account created, and the server answering HTTP.
#
# Plus the pkg_pretend guard, exercised directly with several CHOST values,
# since the whole point of it is to refuse machines this container is not.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"

OUT="packaging/out/portage"
STAGEDIST="packaging/out/portage-distdir"
VERSION="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
PKGDIR="$OUT/app-misc/breeze-core-bin"
TREE_VOLUME="bc-portage-tree"
STAGE3="gentoo/stage3:amd64-openrc"

[ -f "$PKGDIR/Manifest" ] || { echo "no overlay — run packaging/portage/build-overlay.sh first"; exit 1; }

fail=0
ok()  { printf '  ok    %s\n' "$1"; }
bad() { printf '  FAIL  %s\n' "$1"; fail=$((fail+1)); }

# --- 1. the Manifest, checked against the local tarballs --------------------
# Independently of portage. If portage had fetched a tarball from GitHub
# instead of using the staged one, the hashes would differ and this is the only
# check that would notice.
echo "=== 1. Manifest hashes vs. the local tarballs (hashed here, not by portage)"
py - "$PKGDIR/Manifest" "$STAGEDIST" <<'PY'
import hashlib, io, os, sys

manifest, distdir = sys.argv[1], sys.argv[2]
rows = []
for line in io.open(manifest, encoding="utf-8"):
    f = line.split()
    if not f or f[0] != "DIST":
        continue
    name, size = f[1], int(f[2])
    h = dict(zip(f[3::2], f[4::2]))
    rows.append((name, size, h))

bad = 0
for name, size, h in rows:
    p = os.path.join(distdir, name)
    if not os.path.exists(p):
        print("  FAIL  %s: not in the staged distdir" % name); bad += 1; continue
    data = open(p, "rb").read()
    got = {
        "BLAKE2B": hashlib.blake2b(data).hexdigest(),
        "SHA512": hashlib.sha512(data).hexdigest(),
    }
    problems = []
    if len(data) != size:
        problems.append("size %d != %d" % (len(data), size))
    for alg, want in h.items():
        if alg in got and got[alg] != want:
            problems.append("%s mismatch" % alg)
    if problems:
        print("  FAIL  %s: %s" % (name, ", ".join(problems))); bad += 1
    else:
        print("  ok    %s (%d bytes, BLAKE2B+SHA512 match)" % (name, size))

if not rows:
    print("  FAIL  the Manifest has no DIST lines at all"); bad += 1
elif len(rows) != 6:
    print("  FAIL  expected 6 DIST lines, found %d" % len(rows)); bad += 1
else:
    print("  ok    all 6 architectures have a DIST line")
sys.exit(1 if bad else 0)
PY
if [ $? -ne 0 ]; then fail=$((fail+1)); fi

# --- 2. the pkg_pretend guard, run directly ---------------------------------
# The function is extracted from the real ebuild and run with stubs, so this
# tests the shipped code rather than a copy of its logic. The guard is the only
# thing standing between an armv6 machine and a binary it cannot execute, and
# it is unreachable in a normal amd64 test.
echo
echo "=== 2. the pkg_pretend CHOST guard"
# Globbed, not named: the ebuild carries a Gentoo revision suffix
# (breeze-core-bin-4.0.2-r1.ebuild) whenever the same upstream version is
# rebuilt, and naming it exactly would fail the moment that happens.
EBUILD_FILE="$(ls -1 "$PKGDIR"/breeze-core-bin-"$VERSION"*.ebuild 2>/dev/null | head -1)"
[ -n "$EBUILD_FILE" ] || { echo "no ebuild for $VERSION in $PKGDIR"; exit 1; }
echo "    $(basename "$EBUILD_FILE")"
guard="$(sed -n '/^pkg_pretend()/,/^}/p' "$EBUILD_FILE")"
[ -n "$guard" ] || bad "could not extract pkg_pretend from the ebuild"

try_guard() {
  local desc="$1" arch="$2" chost="$3" expect="$4"   # expect: pass | die
  local out rc
  out="$(CHOST="$chost" bash -c "
    use() { [ \"\$1\" = '$arch' ]; }
    eerror() { echo \"    | \$*\"; }
    die() { echo \"DIED\"; exit 1; }
    $guard
    pkg_pretend
  " 2>&1)" && rc=0 || rc=1
  if [ "$expect" = die ] && [ "$rc" -ne 0 ]; then
    ok "$desc → refused"
  elif [ "$expect" = pass ] && [ "$rc" -eq 0 ]; then
    ok "$desc → allowed"
  else
    bad "$desc → expected $expect, got rc=$rc"
    printf '%s\n' "$out" | sed 's/^/      /'
  fi
}

try_guard "armv7 hard-float"      arm   armv7a-unknown-linux-musleabihf pass
try_guard "armv7 hf (gnueabihf)"  arm   armv7a-unknown-linux-gnueabihf  pass
try_guard "armv6 soft-float"      arm   armv6j-unknown-linux-gnueabi    die
try_guard "armv5"                 arm   armv5tel-unknown-linux-gnueabi  die
try_guard "ppc64 little-endian"   ppc64 powerpc64le-unknown-linux-gnu   pass
try_guard "ppc64 BIG-endian"      ppc64 powerpc64-unknown-linux-gnu     die
try_guard "amd64 (no guard)"      amd64 x86_64-pc-linux-gnu             pass

# --- 3. metadata + a real emerge -------------------------------------------
MOUNT="$REPO"
case "$MOUNT" in /[a-z]/*) MOUNT="$(echo "$MOUNT" | sed -E 's#^/([a-z])/#\U\1:/#')" ;; esac
export MSYS_NO_PATHCONV=1

echo
echo "=== 3. metadata generation and a real emerge"
docker run --rm -i \
  --volumes-from "$TREE_VOLUME" \
  -v "$MOUNT/$OUT:/overlay-ro:ro" \
  -v "$MOUNT/$STAGEDIST:/distdir:ro" \
  -e VERSION="$VERSION" \
  "$STAGE3" bash -eu <<'GENTOO'
fail=0
ok()   { printf '  ok    %s\n' "$1"; }
bad()  { printf '  FAIL  %s\n' "$1"; fail=$((fail+1)); }
check(){ if eval "$2" >/dev/null 2>&1; then ok "$1"; else bad "$1"; fi; }

cp -a /overlay-ro /overlay
mkdir -p /etc/portage/repos.conf /var/cache/distfiles
cat > /etc/portage/repos.conf/aspic.conf <<EOF
[aspic]
location = /overlay
EOF
cp /distdir/*.tar.zst /var/cache/distfiles/

cat >> /etc/portage/make.conf <<EOF
DISTDIR="/var/cache/distfiles"
FEATURES="-sandbox -usersandbox -ipc-sandbox -network-sandbox -pid-sandbox"
# The ebuild is ~amd64, and the profile accepts only stable by default.
ACCEPT_KEYWORDS="~amd64"
ACCEPT_LICENSE="* -@EULA"
EOF

echo
echo "--- 3a. metadata generation (parses every ebuild, resolves every inherit)"
# Output to a file first, then report. Piping into sed and testing the `if`
# would test SED's exit status, not the command's -- which is exactly how an
# earlier version of this script printed "emerge completed" over the top of a
# failed merge and then reported nine mystery FAILs underneath it.
if egencache --repo aspic --update --jobs 2 > /tmp/egencache.log 2>&1; then
  ok "egencache parsed the overlay"
else
  bad "egencache failed — an ebuild does not parse or an inherit is wrong"
fi
sed 's/^/    /' /tmp/egencache.log | tail -10

echo
echo "--- 3b. what portage thinks it is about to do"
emerge --pretend --verbose app-misc/breeze-core-bin 2>&1 | sed 's/^/    /' || true

echo
echo "--- 3c. emerge for real"
# --nodeps would skip acct-user/acct-group, which is half of what is being
# tested. No network: the v4.0.2 release does not exist on GitHub yet, so a
# fetch attempt would fail outright -- which is itself the proof that the
# staged distfile is what got used.
if emerge --quiet app-misc/breeze-core-bin > /tmp/emerge.log 2>&1; then
  ok "emerge completed"
else
  bad "emerge failed"
fi
# Only the tail on success; the whole thing when it broke, since the useful
# part of a portage failure is well above the last 25 lines.
if [ -s /tmp/emerge.log ]; then
  grep -q 'ERROR' /tmp/emerge.log && sed 's/^/    /' /tmp/emerge.log \
    || tail -25 /tmp/emerge.log | sed 's/^/    /'
fi

echo
echo "--- 3d. what landed"
check "binary in /usr/bin"           "[ -x /usr/bin/breeze-core ]"
check "OpenRC init script"           "[ -x /etc/init.d/breeze-core ]"
check "conf.d file"                  "[ -f /etc/conf.d/breeze-core ]"
check "systemd unit"                 "[ -f /usr/lib/systemd/system/breeze-core.service ] || [ -f /lib/systemd/system/breeze-core.service ]"
check "state directory"              "[ -d /etc/breeze-core ]"
check "README installed"             "ls /usr/share/doc/breeze-core-bin-*/README.md*"
# breeze-core.env must NOT be installed on Gentoo: conf.d is the config here,
# and two config files that both set BREEZE_HOST is a support question waiting
# to happen.
check "no breeze-core.env installed" "[ ! -f /etc/breeze-core/breeze-core.env ]"

echo
echo "--- 3e. the service account, via acct-user/acct-group"
check "breeze user exists"    "getent passwd breeze"
check "breeze group exists"   "getent group breeze"
check "home is the state dir" "[ \"\$(getent passwd breeze | cut -d: -f6)\" = /etc/breeze-core ]"
check "shell is nologin"      "getent passwd breeze | grep -q nologin"

echo
echo "--- 3f. permissions"
stat -c '    %U:%G %a  %n' /etc/breeze-core
check "/etc/breeze-core is breeze:breeze" "[ \"\$(stat -c '%U:%G' /etc/breeze-core)\" = 'breeze:breeze' ]"
check "/etc/breeze-core is 750"           "[ \"\$(stat -c '%a' /etc/breeze-core)\" = '750' ]"

echo
echo "--- 3g. it runs, and serves"
check "--version says $VERSION" "/usr/bin/breeze-core --version | grep -q \"\$VERSION\""
check "no ELF interpreter (static)" "! grep -qa 'ld-linux\|ld-musl' /usr/bin/breeze-core"
check "init script parses"          "sh -n /etc/init.d/breeze-core"

printf '%s' '{"api_key":"verify-portage","units":[]}' > /etc/breeze-core/config.json
chown breeze:breeze /etc/breeze-core/config.json
chmod 640 /etc/breeze-core/config.json

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
check "/api/health answers" "[ -n \"\$health\" ]"
code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 http://127.0.0.1:8420/api/units || true)"
printf '    GET /api/units without credentials -> %s\n' "$code"
check "/api/units is 401 without credentials" "[ \"\$code\" = 401 ]"
kill $srv 2>/dev/null || true
wait $srv 2>/dev/null || true
sed 's/^/    /' /tmp/serve.log | head -6

echo
echo "--- 3h. unmerge keeps the configuration"
emerge --quiet --unmerge app-misc/breeze-core-bin 2>&1 | tail -5 | sed 's/^/    /' || true
check "binary removed"   "[ ! -e /usr/bin/breeze-core ]"
check "config.json KEPT" "[ -f /etc/breeze-core/config.json ]"

echo
[ "$fail" -eq 0 ] || { echo "  ($fail check(s) failed inside the container)"; exit 1; }
GENTOO
container_rc=$?
[ "$container_rc" -eq 0 ] || fail=$((fail+1))

echo
if [ "$fail" -eq 0 ]; then
  echo "=== all checks passed"
else
  echo "=== $fail SECTION(S) FAILED"
  exit 1
fi
