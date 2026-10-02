#!/usr/bin/env bash
# Verify the Termux packages by installing them in Termux itself.
#
#   ./packaging/termux/verify-termux.sh                # x86_64 aarch64 arm
#   ./packaging/termux/verify-termux.sh x86_64
#
# termux-docker is a real Android userland -- bionic, Termux's prefix, its
# apt and dpkg, running as an unprivileged app user -- so this installs the
# package the way a phone would, runs it under termux-services, pairs a client
# and reads the server's own account of where it is running. x86_64 runs
# natively here; aarch64 and arm go through QEMU, which is fine because nothing
# is compiled in the container, only run.
#
# What this canNOT check, stated so it is not mistaken for covered: Android
# itself. There is no Doze, no phantom-process killer, no Wi-Fi and no
# Termux:Boot in a container, and getprop has no property service to ask, so
# the Android version reads as plain "Android" here. Staying alive on a real
# phone is the wake lock's job, and only a phone can show it working.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
OUT="packaging/out/termux"
VERSION="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"

MOUNT="$REPO"
case "$MOUNT" in /[a-z]/*) MOUNT="$(echo "$MOUNT" | sed -E 's#^/([a-z])/#\U\1:/#')" ;; esac
export MSYS_NO_PATHCONV=1

want=("$@")
[ ${#want[@]} -gt 0 ] || want=(x86_64 aarch64 arm)

total_fail=0
for arch in "${want[@]}"; do
  case "$arch" in
    x86_64)  platform=linux/amd64;  sandbox=() ;;
    aarch64) platform=linux/arm64;  sandbox=() ;;
    # 32-bit bionic calls personality() as its very first act and aborts if
    # refused ("error getting old personality value: Operation not
    # permitted", then qemu's SIGABRT), and Docker's default seccomp profile
    # refuses the value it asks for. termux-docker's README offers privileged
    # mode or no seccomp; no seccomp is the smaller of the two, and only this
    # architecture needs it.
    arm)     platform=linux/arm/v7; sandbox=(--security-opt seccomp=unconfined) ;;
    *) echo "unknown architecture $arch (x86_64, aarch64, arm)"; exit 1 ;;
  esac
  deb="$(ls "$OUT"/breeze-core_"$VERSION"-*_"$arch".deb 2>/dev/null | tail -1 || true)"
  [ -n "$deb" ] || { echo "no $arch package for $VERSION -- run packaging/termux/build-packages.sh $arch"; exit 1; }

  echo
  echo "##### $arch ($platform): $(basename "$deb")"
  # The values go in as the script's first lines, not with -e: the image's
  # entrypoint switches to the app user through `env -i`, which keeps its own
  # short list of variables and drops every other one.
  if { printf 'DEB=%s\nVERSION=%s\nARCH=%s\n' "/pkgs/$(basename "$deb")" "$VERSION" "$arch"
       cat <<'TERMUX'
fail=0
ok()   { printf '  ok    %s\n' "$1"; }
bad()  { printf '  FAIL  %s\n' "$1"; fail=$((fail+1)); }
check(){ if eval "$2" >/dev/null 2>&1; then ok "$1"; else bad "$1"; fi; }
SV=$PREFIX/var/service/breeze-core
CONF=$PREFIX/etc/breeze-core
U=http://127.0.0.1:8420
up() {   # wait for the server to answer, up to 20 s
  i=0; while [ $i -lt 40 ]; do
    curl -fs -o /dev/null "$U/api/health" && return 0; sleep 0.5; i=$((i+1)); done
  return 1
}
gone() { # wait for it to stop answering
  i=0; while [ $i -lt 40 ]; do
    curl -fs -o /dev/null "$U/api/health" || return 0; sleep 0.5; i=$((i+1)); done
  return 1
}

echo "=== 1. declared metadata"
dpkg-deb -f "$DEB" Package Version Architecture Depends | sed 's/^/    /'
check "version is $VERSION"            "dpkg-deb -f '$DEB' Version | grep -q '^$VERSION-'"
check "architecture is Termux's $ARCH" "[ \"\$(dpkg-deb -f '$DEB' Architecture)\" = '$ARCH' ]"
check "depends on termux-services"     "dpkg-deb -f '$DEB' Depends | grep -qx termux-services"
check "dpkg accepts the architecture"  "[ \"\$(dpkg --print-architecture)\" = '$ARCH' ]"
# The ar member names are plain text, the data member's just after the control
# tarball (a couple of KB in). A gzip control member is what 32-bit apt
# rejected, and dpkg-deb -f reads it happily regardless, so only the name gives
# it away before apt does.
head -c 65536 "$DEB" | tr -c '[:alnum:]._' ' ' > $TMPDIR/members
check "control and data both xz, as Termux's own" \
  "grep -q 'control.tar.xz' $TMPDIR/members && grep -q 'data.tar.xz' $TMPDIR/members"

echo
echo "=== 2. install, as a phone would (apt resolves termux-services)"
apt-get -qq update >$TMPDIR/update.log 2>&1 || { sed 's/^/    /' $TMPDIR/update.log; exit 1; }
if apt-get -qq install -y "$DEB" >$TMPDIR/install.log 2>&1; then
  ok "installed with its dependency"
  sed -n '/Breeze Core installed/,/Docs:/p' $TMPDIR/install.log | sed 's/^/    /'
else
  bad "apt-get install"; sed 's/^/    /' $TMPDIR/install.log; exit 1
fi
check "breeze-core is on PATH and runs" "breeze-core --version | grep -q 'breeze-core $VERSION'"
check "the banner was printed"          "grep -q 'sv-enable breeze-core' $TMPDIR/install.log"
check "service run script executable"   "[ -x $SV/run ]"
check "log/run resolves to svlogger"    "[ -x $SV/log/run ]"
check "installed switched OFF (down)"   "[ -e $SV/down ]"
check "config dir is 0700"              "[ \"\$(stat -c %a $CONF)\" = 700 ]"
check "env file is 0600"                "[ \"\$(stat -c %a $CONF/breeze-core.env)\" = 600 ]"
check "env file is a conffile"          "grep -q breeze-core.env $PREFIX/var/lib/dpkg/info/breeze-core.conffiles"

echo
echo "=== 3. run under termux-services"
# Exactly what Termux does when a session opens, rather than a hand-started
# runsvdir: the profile script also exports LOGDIR, and svlogger, given none,
# tries to log under /sv and the log service never comes up.
. $PREFIX/etc/profile.d/start-services.sh
sleep 6
check "switched off, so not serving yet" "! curl -fs -o /dev/null $U/api/health"
# A config the way `breeze-core pair` would leave one with no units yet: the
# key is generated here and never printed.
K=$(od -An -tx1 -N16 /dev/urandom | tr -d ' \n')
printf '{"api_key": "%s", "units": []}\n' "$K" > $CONF/config.json
chmod 640 $CONF/config.json
sv-enable breeze-core >/dev/null 2>&1 || true
check "sv-enable removed down"           "[ ! -e $SV/down ]"
check "serving after sv-enable"          "up"
check "sv reports it running"            "sv status breeze-core | grep -q '^run:'"
check "the log reached svlogger"         "grep -q 'listening on 127.0.0.1:8420' $PREFIX/var/log/sv/breeze-core/current"

j() { sed -n "s/.*\"$1\": *\"\([^\"]*\)\".*/\1/p"; }
S=$(curl -s -X POST $U/api/auth/enroll/start -H "X-API-Key: $K" \
      -H 'Content-Type: application/json' -d '{"label":"verify-termux"}')
C=$(echo "$S" | j user_code); SID=$(echo "$S" | j session_id)
check "a pairing code was issued"        "[ -n '$C' ]"
check "approved from the LAN (loopback)" "curl -fs -o /dev/null -X POST $U/api/auth/enroll/approve -H 'X-API-Key: $K' -H 'Content-Type: application/json' -d '{\"code\":\"$C\"}'"
T=$(curl -s -X POST $U/api/auth/enroll/poll -H "X-API-Key: $K" \
      -H 'Content-Type: application/json' -d "{\"session_id\":\"$SID\"}" | j device_token)
check "the poll handed over a token"     "[ -n '$T' ]"
curl -s -H "X-API-Key: $K" -H "Authorization: Bearer $T" $U/api/system > $TMPDIR/system.json
check "devices.json written by the service" "[ -s $CONF/devices.json ]"
check "/api/system: platform android"    "grep -q '\"platform\":\"android\"' $TMPDIR/system.json"
check "/api/system: libc bionic"         "grep -q '\"libc\":\"bionic\"' $TMPDIR/system.json"
check "/api/system: init termux-services" "grep -q '\"name\":\"termux-services\"' $TMPDIR/system.json"
check "/api/system: config under PREFIX" "grep -q '\"config\":\"$CONF/config.json\"' $TMPDIR/system.json"

echo
echo "=== 4. reinstall, as an upgrade (enabled stays enabled)"
pid1=$(sv status breeze-core | sed -n 's/^run: [^(]*(pid \([0-9]*\)).*/\1/p')
apt-get -qq install -y --reinstall "$DEB" >$TMPDIR/re.log 2>&1 || { bad "reinstall"; sed 's/^/    /' $TMPDIR/re.log; }
check "no down file after the upgrade"  "[ ! -e $SV/down ]"
check "serving after the upgrade"       "up"
pid2=$(sv status breeze-core | sed -n 's/^run: [^(]*(pid \([0-9]*\)).*/\1/p')
check "restarted onto the new binary"   "[ -n '$pid2' ] && [ '$pid1' != '$pid2' ]"
check "no banner on an upgrade"         "! grep -q 'Breeze Core installed' $TMPDIR/re.log"

echo
echo "=== 5. remove: stopped, switched off, credentials kept"
apt-get -qq remove -y breeze-core >$TMPDIR/rm.log 2>&1 || { bad "remove"; sed 's/^/    /' $TMPDIR/rm.log; }
check "no longer serving"               "gone"
check "binary gone"                     "[ ! -e $PREFIX/bin/breeze-core ]"
check "left switched off (down)"        "[ -e $SV/down ]"
check "config.json kept"                "[ -s $CONF/config.json ]"
check "devices.json kept"               "[ -s $CONF/devices.json ]"
check "said where it kept them"         "grep -q 'kept $CONF' $TMPDIR/rm.log"

echo
echo "=== 6. purge: the service directory goes, the credentials stay"
apt-get -qq purge -y breeze-core >$TMPDIR/purge.log 2>&1 || { bad "purge"; sed 's/^/    /' $TMPDIR/purge.log; }
check "service directory removed"       "[ ! -e $SV ]"
check "config.json still kept"          "[ -s $CONF/config.json ]"

echo
if [ "$fail" -eq 0 ]; then echo "  all checks passed"; else echo "  $fail check(s) FAILED"; fi
[ "$fail" -eq 0 ]
TERMUX
     } | docker run --rm -i --platform "$platform" ${sandbox[@]+"${sandbox[@]}"} -v "$MOUNT/$OUT:/pkgs:ro" \
           "termux/termux-docker:$arch" sh -eu -s
  then :; else total_fail=$((total_fail+1)); fi
done

echo
if [ "$total_fail" -eq 0 ]; then echo "termux: every architecture passed"; else echo "termux: $total_fail architecture(s) FAILED"; exit 1; fi
