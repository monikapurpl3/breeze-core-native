#!/usr/bin/env bash
# Build the os-breeze-core OPNsense plugin package.
#
#   packaging/opnsense/build-plugin.sh
#
# Input:  packaging/out/bin/freebsd-amd64/breeze-core (build-binaries.sh)
# Output: packaging/out/opnsense/os-breeze-core-<ver>.pkg
#
# Why this is a separate package from the ordinary FreeBSD one: it carries the
# GUI (Services > Breeze Core), configd actions and the account handling
# OPNsense needs, and OPNsense spans two FreeBSD majors - 26.1 is FreeBSD 14.3,
# 26.7 is FreeBSD 15.1. The binary is the same one the FreeBSD package wraps:
# it is cross-built against FreeBSD 14's libc, the oldest base it has to run
# on, and FreeBSD binaries run forward. Until 4.3.2 that meant compiling it in
# a FreeBSD 14 root on a FreeBSD VM; now the binary is cross-built and the
# package is made with FreeBSD's own pkg in the bc-bsdpkg container
# (packaging/bsd/Dockerfile). Nothing is ever compiled on the firewall.
#
# The package's ABI is therefore the pattern FreeBSD:1[45]:amd64. pkg matches a
# package's ABI with fnmatch against the host's, so that installs on 14 and 15
# and refuses 13 (where a 14 binary cannot run) and a future 16 (where nobody
# has tried it). Stamped with a single release's ABI, as 4.1.1 was, it was
# refused by every OPNsense 26.7:
#   "wrong architecture: FreeBSD:14:amd64 instead of FreeBSD:15:amd64".
# Widen the pattern only after running the binary on the new major.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"; cd "$REPO"
VER="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
BIN=packaging/out/bin/freebsd-amd64/breeze-core
OUT=packaging/out/opnsense
PKG_ABI='FreeBSD:1[45]:amd64'
export MSYS_NO_PATHCONV=1

[ -f "$BIN" ] || { echo "no $BIN - run: ./packaging/build-binaries.sh freebsd-amd64" >&2; exit 1; }
newer="$(find crates static Cargo.toml Cargo.lock -newer "$BIN" -type f -print -quit 2>/dev/null || true)"
[ -z "$newer" ] || { echo "$BIN is older than the source ($newer)" >&2; exit 1; }
echo "==> os-breeze-core $VER" >&2

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT

# ------------------------------------------------------------- 1. manifest
cat > "$TMP/manifest.ucl" <<MANIFEST
name: "os-breeze-core"
version: "$VER"
origin: "opnsense/os-breeze-core"
comment: "Breeze Core - LAN-first control for Midea air conditioners"
desc: <<EOD
Breeze Core runs on this firewall and controls Midea air conditioners over the
LAN: a REST API plus a web panel, with no cloud dependency after pairing.

Adds Services > Breeze Core to the GUI: enable, bind address, port, extra
environment and service control; the paired units and the API key, edited in
place in Breeze Core's own config.json with the V3 credentials never sent to
the browser; and enrolled clients, with approving a pairing code and revoking.
One static executable with the panel compiled into it - no
interpreter and no dependencies, so nothing is ever compiled on the firewall
and nothing breaks when OPNsense changes its Python.

Pair air conditioners with 'breeze-core pair'; admit clients with
'breeze-core approve'. Approval is LAN-only by design.
EOD
maintainer: "monikapurpl3@users.noreply.github.com"
www: "https://github.com/monikapurpl3/breeze-core-native"
abi: "$PKG_ABI"
arch: "$PKG_ABI"
prefix: "/usr/local"
licenselogic: "single"
licenses: ["AGPLv3+"]
categories: ["www", "sysutils"]
scripts: {
  post-install: <<EOS
# The service account, with ids BELOW 2000 on purpose. OPNsense's
# local_sync_accounts() deletes every user and group from 2000 to 65000 that
# is not in config.xml, at each boot and each account sync. Up to 4.1.1 this
# was 8420, so the account vanished at the first reboot and the service could
# not start: daemon: unknown user: breeze. 1842 is unassigned in FreeBSD's
# ports UIDs and GIDs, and OPNsense's own users start at 2000. The plain
# FreeBSD package keeps 8420; nothing there deletes accounts.
if pw groupshow breeze >/dev/null 2>&1; then pw groupmod breeze -g 1842; else pw groupadd breeze -g 1842; fi
if pw usershow breeze >/dev/null 2>&1; then pw usermod breeze -u 1842 -g breeze; else pw useradd breeze -u 1842 -g breeze \
    -d /nonexistent -s /usr/sbin/nologin -c "Breeze Core"; fi
install -d -o breeze -g breeze -m 750 /usr/local/etc/breeze-core
# Hand over what the old ids owned - config.json, devices.json, the log - and
# nothing else: a deleted account leaves its files owned by a bare 8420, which
# the service cannot read once it runs as 1842. Only those, because configd
# renders service.env as root with the service group, and that stays so.
for p in /usr/local/etc/breeze-core /var/log/breeze_core.log /var/run/breeze_core; do
  if [ -e "\$p" ]; then find "\$p" -user 8420 -exec chown breeze {} +; find "\$p" -group 8420 -exec chgrp breeze {} +; fi
done
# Where configd renders breeze_core: /etc/rc.conf.d, the one directory both
# rc.subr and OPNsense's boot (rc.freebsd) read. Up to 4.1.1 it went to
# /usr/local/etc/rc.conf.d, which the boot never reads, so the service did not
# come back after a reboot. That old copy goes, because rc.subr reads it after
# /etc's and it would override every later Save.
install -d -m 755 /etc/rc.conf.d
rm -f /usr/local/etc/rc.conf.d/breeze_core
# What opnsense/plugins' Mk/plugins.mk appends to every plugin's +POST_INSTALL,
# in its order (Templates/actions.d, models, configure, templates). configd
# reads actions.d only when it starts: without the restart, every Services >
# Breeze Core button answers "Action not allowed or missing" until the
# firewall reboots - which 4.1.1 did, found on a real OPNsense 26.7. The
# configure step is also what drops the MVC/volt cache, so the new menu and
# page appear at once.
if [ -f /usr/local/etc/rc.d/configd ]; then /usr/local/etc/rc.d/configd restart; fi
if [ -f /usr/local/opnsense/mvc/script/run_migrations.php ]; then /usr/local/opnsense/mvc/script/run_migrations.php OPNsense/BreezeCore; fi
if [ -f /usr/local/etc/rc.configure_plugins ]; then echo "Reloading plugin configuration"; /usr/local/etc/rc.configure_plugins POST_INSTALL; fi
if [ -f /usr/local/sbin/configctl ]; then echo -n "Reloading template OPNsense/BreezeCore: "; /usr/local/sbin/configctl template reload OPNsense/BreezeCore; fi
# Last, and ours rather than upstream's: an upgrade runs the old package's
# post-deinstall, which stops the service, and nothing started it again - so
# upgrading the plugin left Breeze Core down until someone pressed Start. The
# configure action is what Save uses: it starts the service only if enabled.
if [ -f /usr/local/sbin/configctl ]; then echo -n "Starting Breeze Core if enabled: "; /usr/local/sbin/configctl breezecore configure; fi
echo "===> Breeze Core installed."
echo "     1) Services > Breeze Core: set the listen address, then enable."
echo "     2) breeze-core pair      - discover and pair the air conditioners."
echo "     3) breeze-core approve   - admit a phone or browser (LAN only)."
EOS
  post-deinstall: <<EOS
/usr/local/etc/rc.d/breeze_core onestop >/dev/null 2>&1 || true
# Upstream's configure step again, so the menu entry goes with the plugin.
if [ -f /usr/local/etc/rc.configure_plugins ]; then echo "Reloading plugin configuration"; /usr/local/etc/rc.configure_plugins POST_DEINSTALL; fi
echo "===> config kept at /usr/local/etc/breeze-core (remove by hand if unwanted)"
EOS
}
MANIFEST

# ---------------------------------------------------- 2. stage and package
# Streamed in and out, never bind-mounted (see packaging/nfpm/build-packages.sh).
docker build -q -t bc-bsdpkg packaging/bsd >/dev/null
mkdir -p "$OUT"
rm -f "$OUT"/os-breeze-core-*.pkg
tar -cf - packaging/opnsense/files "$BIN" -C "$TMP" manifest.ucl \
  | docker run --rm -i bc-bsdpkg sh -euc '
      exec 3>&1 1>&2
      mkdir -p /w && cd /w && tar -xf -
      S=/tmp/stage
      mkdir -p "$S/usr/local/bin"
      cp -R packaging/opnsense/files/usr "$S/"
      cp packaging/out/bin/freebsd-amd64/breeze-core "$S/usr/local/bin/breeze-core"
      # Explicit modes: the files come from a Windows checkout, which records no
      # POSIX execute bit. A non-executable serve.sh fails invisibly, because
      # daemon(8) runs with -f and the permission error goes to /dev/null.
      chmod 755 "$S/usr/local/bin/breeze-core" \
                "$S/usr/local/lib/breeze-core/serve.sh" \
                "$S/usr/local/etc/rc.d/breeze_core" \
                "$S/usr/local/opnsense/scripts/OPNsense/BreezeCore/setup.sh"
      # The plist is not optional: given only -M and -r, pkg create packages
      # the manifest and nothing else, exits 0, and produces a ~1 KB
      # "package". Generated from the staged tree, it cannot drift from it.
      ( cd "$S" && find . -type f -o -type l ) | sed "s|^\.||" | sort > /tmp/plist
      echo "  plist entries: $(wc -l < /tmp/plist | tr -d " ")"
      mkdir -p /tmp/pkgout
      pkg create -M manifest.ucl -r "$S" -p /tmp/plist -o /tmp/pkgout
      tar -cf - -C /tmp/pkgout . >&3
    ' | tar -xf - -C "$OUT"

PKG="$(ls -1 "$OUT"/os-breeze-core-*.pkg | tail -1)"
SIZE="$(wc -c < "$PKG")"
# A package this small means the plist was empty or missing - fail loudly rather
# than shipping it.
[ "$SIZE" -gt 900000 ] || { echo "package is only $SIZE bytes - plist problem" >&2; exit 1; }
echo "==> $(ls -1sh "$PKG")" >&2
