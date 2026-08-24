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
    -v "$MOUNT/$TREE:/usr/share/nginx/html:ro" nginx:alpine >/dev/null
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
  if printf '%s' "$script" \
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
  curl -fsSL -o /tmp/p.rpm "$BASE/rpm/x86_64/breeze-core-$VER-1.x86_64.rpm"
  rpm -K /tmp/p.rpm | tee /tmp/k
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

echo
echo "=== $pass passed, $fail failed ==="
[ "$fail" = 0 ] || exit 1
