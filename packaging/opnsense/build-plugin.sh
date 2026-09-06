#!/usr/bin/env bash
# Build the os-breeze-core OPNsense plugin package.
#
#   packaging/opnsense/build-plugin.sh [freebsd-builder-host]
#
# Output: packaging/out/opnsense/os-breeze-core-<ver>.pkg
#
# Why this is a separate build from the ordinary FreeBSD package: OPNsense is
# **FreeBSD 14**, and the FreeBSD package here is built on 15. FreeBSD binaries
# run forward, not backward, so a 15-built binary on a 14 firewall is a gamble
# with no upside. So the binary for this package is compiled in a FreeBSD 14
# root on the builder - which is also where the Python line vendored its whole
# interpreter, for the harder reason that OPNsense ships neither rust nor pip
# nor any of its dependencies. Nothing is ever compiled on the firewall.
set -euo pipefail

HOST="${1:-192.168.122.131}"
USER_AT="${BSD_USER:-monika}@$HOST"
REPO="$(cd "$(dirname "$0")/../.." && pwd)"; cd "$REPO"
VER="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
SHA="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
git diff --quiet HEAD 2>/dev/null || SHA="$SHA-dirty"
ROOT="${FB14_ROOT:-/jail/fb14}"
FB_BASE="${FB_BASE:-14.3-RELEASE}"
OUT=packaging/out/opnsense
mkdir -p "$OUT"

echo "==> os-breeze-core $VER  (builder $USER_AT, $ROOT, FreeBSD $FB_BASE)" >&2

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
# An allowlist, not a tar of everything-minus-excludes: the remote side needs
# exactly the crates, the panel, the lockfile and the plugin's own files. The
# Python line learned this the hard way, having once shipped its whole working
# tree - signing keys included - to a build VM.
tar -czf "$TMP/src.tar.gz" \
    crates static Cargo.toml Cargo.lock LICENSE README.md \
    packaging/opnsense/files packaging/nfpm/breeze-core.env
scp -q -o BatchMode=yes "$TMP/src.tar.gz" "$USER_AT:/tmp/bc-opn-src.tar.gz"

# No -n on ssh: stdin IS this heredoc, and -n points stdin at /dev/null, so the
# remote side would run nothing at all and the only symptom would be a missing
# artefact at the end.
ssh -o BatchMode=yes "$USER_AT" "VER='$VER' SHA='$SHA' ROOT='$ROOT' FB_BASE='$FB_BASE' sh -s" <<'REMOTE'
set -eu

# ---------------------------------------------------------- 1. FreeBSD 14 root
if [ ! -d "$ROOT/usr" ]; then
    echo "  creating the FreeBSD 14 build root"
    doas mkdir -p "$ROOT"
    [ -f /tmp/base14.txz ] || \
        fetch -q -o /tmp/base14.txz "https://download.freebsd.org/releases/amd64/$FB_BASE/base.txz"
    doas tar -xpf /tmp/base14.txz -C "$ROOT"
    doas cp /etc/resolv.conf "$ROOT/etc/resolv.conf"
fi

# devfs is NOT optional. Without it cargo pipes source to `rustc -` on stdin,
# that read misbehaves with no /dev, and rustc parses an error message as
# source: "E0554: #![feature] may not be used on the stable release channel",
# which looks like a toolchain problem and is not one.
mount | grep -q "$ROOT/dev" || doas mount -t devfs devfs "$ROOT/dev"

if ! doas chroot "$ROOT" /bin/sh -c 'command -v cargo >/dev/null 2>&1'; then
    echo "  installing rust in the build root (once)"
    doas chroot "$ROOT" /bin/sh -c '
        export ASSUME_ALWAYS_YES=yes
        pkg bootstrap -f >/dev/null 2>&1 || true
        pkg update -q
        pkg install -y rust >/dev/null
    '
fi
ABI="$(doas chroot "$ROOT" pkg config ABI)"
echo "  build root: $ABI, $(doas chroot "$ROOT" cargo --version)"

# ------------------------------------------------------------------ 2. build
doas rm -rf "$ROOT/tmp/src" "$ROOT/tmp/stage" "$ROOT/tmp/pkgout"
doas mkdir -p "$ROOT/tmp/src"
doas tar -xzf /tmp/bc-opn-src.tar.gz -C "$ROOT/tmp/src"
doas chroot "$ROOT" /bin/sh -c "
    set -eu
    cd /tmp/src
    CARGO_HOME=/tmp/cargo BREEZE_COMMIT='$SHA' cargo build --release 2>&1 | tail -3
    ./target/release/breeze-core --version
"

# ------------------------------------------------------------------ 3. stage
S="$ROOT/tmp/stage"
doas mkdir -p "$S/usr/local/bin"
doas sh -c "cp -R $ROOT/tmp/src/packaging/opnsense/files/usr $S/"
doas cp "$ROOT/tmp/src/target/release/breeze-core" "$S/usr/local/bin/breeze-core"

# Explicit modes: the source tar is produced on Windows, which records no POSIX
# execute bit. A non-executable serve.sh fails invisibly, because daemon(8) runs
# with -f and the permission error goes to /dev/null.
doas chmod 755 "$S/usr/local/bin/breeze-core" \
     "$S/usr/local/lib/breeze-core/serve.sh" \
     "$S/usr/local/etc/rc.d/breeze_core" \
     "$S/usr/local/opnsense/scripts/OPNsense/BreezeCore/setup.sh"

# -------------------------------------------------------------- 4. manifest
# ABI comes from the chroot, so it is FreeBSD:14:amd64 by construction rather
# than a hardcoded string that can drift.
doas sh -c "cat > '$ROOT/tmp/manifest.ucl'" <<MANIFEST
name: "os-breeze-core"
version: "$VER"
origin: "opnsense/os-breeze-core"
comment: "Breeze Core - LAN-first control for Midea air conditioners"
desc: <<EOD
Breeze Core runs on this firewall and controls Midea air conditioners over the
LAN: a REST API plus a web panel, with no cloud dependency after pairing.

Adds Services > Breeze Core to the GUI for enable, bind address, port and
service control. One static executable with the panel compiled into it - no
interpreter and no dependencies, so nothing is ever compiled on the firewall
and nothing breaks when OPNsense changes its Python.

Pair air conditioners with 'breeze-core pair'; admit clients with
'breeze-core approve'. Approval is LAN-only by design.
EOD
maintainer: "monikapurpl3@users.noreply.github.com"
www: "https://github.com/monikapurpl3/breeze-core"
abi: "$ABI"
arch: "$ABI"
prefix: "/usr/local"
licenselogic: "single"
licenses: ["AGPLv3+"]
categories: ["www", "sysutils"]
scripts: {
  post-install: <<EOS
pw groupshow breeze >/dev/null 2>&1 || pw groupadd breeze -g 8420
pw usershow breeze >/dev/null 2>&1 || pw useradd breeze -u 8420 -g breeze \
    -d /nonexistent -s /usr/sbin/nologin -c "Breeze Core"
install -d -o breeze -g breeze -m 750 /usr/local/etc/breeze-core
# Where configd renders breeze_core - and the only place besides /etc that
# load_rc_config() will source it from.
install -d -m 755 /usr/local/etc/rc.conf.d
# OPNsense caches the MVC/volt tree, so a new plugin's menu and page do not
# appear until that cache is dropped.
rm -rf /tmp/opnsense_cache_* /var/cache/opnsense-mvc 2>/dev/null || true
echo "===> Breeze Core installed."
echo "     1) Services > Breeze Core: set the listen address, then enable."
echo "     2) breeze-core pair      - discover and pair the air conditioners."
echo "     3) breeze-core approve   - admit a phone or browser (LAN only)."
EOS
  post-deinstall: <<EOS
/usr/local/etc/rc.d/breeze_core onestop >/dev/null 2>&1 || true
echo "===> config kept at /usr/local/etc/breeze-core (remove by hand if unwanted)"
EOS
}
MANIFEST

# ----------------------------------------------------------------- 5. build
# The plist is not optional: given only -M and -r, pkg create packages the
# manifest and nothing else, exits 0, and produces a ~1 KB "package".
# Generating it from the staged tree means it cannot drift from what was staged.
doas chroot "$ROOT" /bin/sh -c '
    set -eu
    rm -rf /tmp/pkgout && mkdir -p /tmp/pkgout
    ( cd /tmp/stage && find . -type f -o -type l ) | sed "s|^\.||" | sort > /tmp/plist
    echo "  plist entries: $(wc -l < /tmp/plist | tr -d " ")"
    pkg create -M /tmp/manifest.ucl -r /tmp/stage -p /tmp/plist -o /tmp/pkgout
'
REMOTE

scp -q -o BatchMode=yes "$USER_AT:$ROOT/tmp/pkgout/os-breeze-core-*.pkg" "$OUT/" || {
    echo "no package came back" >&2; exit 1; }

PKG="$(ls -1 "$OUT"/os-breeze-core-*.pkg | tail -1)"
SIZE="$(wc -c < "$PKG")"
# A package this small means the plist was empty or missing - fail loudly rather
# than shipping it.
[ "$SIZE" -gt 900000 ] || { echo "package is only $SIZE bytes - plist problem" >&2; exit 1; }
echo "==> $(ls -1sh "$PKG")" >&2
