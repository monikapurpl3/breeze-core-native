#!/bin/sh
# Build an OpenBSD package with pkg_create(1), and sign it with signify(1).
# Run ON OpenBSD, from the source tree, after `cargo build --release`.
#
#   sh packaging/bsd/mkpkg-openbsd.sh [output-dir] [signify-secret-key]
#
# This is the first OpenBSD *package* for this project - the Python line only
# ever managed a source install here, because a virtualenv full of absolute
# paths is not something you can hand to pkg_add. One static binary is.
#
# Two OpenBSD specifics worth knowing before editing the packing list:
#
#   * The service account is declared IN the list, with @newgroup/@newuser,
#     rather than created by a script. pkg_add does it, pkg_delete undoes it,
#     and nothing has to be idempotent by hand.
#   * The configuration is @sample, not a plain file. pkg_add copies a sample
#     into place if it is absent and NEVER touches it afterwards, and pkg_delete
#     leaves an edited one alone - which is exactly the behaviour wanted for a
#     file holding an API key and a unit's V3 credentials.
set -eu

OUT="${1:-.}"
SECKEY="${2:-}"
VER="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
BIN=target/release/breeze-core
ARCH="$(uname -m)"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

[ -x "$BIN" ] || { echo "no $BIN - run: cargo build --release"; exit 1; }

# The fake root: plain plist entries are relative to the prefix (/usr/local),
# absolute ones are relative to this directory.
mkdir -p "$STAGE/usr/local/bin" \
         "$STAGE/usr/local/share/examples/breeze-core" \
         "$STAGE/usr/local/share/doc/breeze-core" \
         "$STAGE/etc/rc.d"
install -m 0755 "$BIN" "$STAGE/usr/local/bin/breeze-core"
install -m 0755 packaging/bsd/rc.openbsd "$STAGE/etc/rc.d/breeze_core"
install -m 0644 packaging/nfpm/breeze-core.env \
    "$STAGE/usr/local/share/examples/breeze-core/breeze-core.env"
install -m 0644 LICENSE "$STAGE/usr/local/share/doc/breeze-core/LICENSE"

# The pkgpath comment is not decoration: without it pkg_add prints "Use of
# uninitialized value in hash element at PkgAdd.pm line 304" on every install,
# because its security-quirks check keys a hash on $plist->fullpkgpath and a
# hand-built package has none. Every installed OpenBSD package carries one.
#
# 799: an unused id in the range OpenBSD leaves for locally added daemons. If a
# `breeze` account already exists (the Python source install made one), pkg_add
# leaves it as it is rather than renumbering it.
plist="$STAGE/.plist"
cat > "$plist" <<'PLIST'
@newgroup breeze:799
@newuser breeze:799:799:daemon:Breeze Core:/nonexistent:/sbin/nologin
@rcscript /etc/rc.d/breeze_core
bin/breeze-core
share/doc/breeze-core/LICENSE
share/examples/breeze-core/breeze-core.env
@owner breeze
@group breeze
@mode 750
@sample /etc/breeze-core/
@mode 640
@sample /etc/breeze-core/breeze-core.env
@owner
@group
@mode
PLIST

desc="$STAGE/.desc"
cat > "$desc" <<'DESC'
Self-hosted, LAN-first control for Midea air conditioners: a REST API, a web
control panel, device-pairing authentication, server-side schedules and
temperature curves, and a diagnostic CLI.

One static executable with the panel compiled into it - no interpreter, no
runtime dependencies. Configuration lives in /etc/breeze-core; enable the
service with `rcctl enable breeze_core`.
DESC

mkdir -p "$OUT"
pkg_create -A "$ARCH" -B "$STAGE" -p /usr/local \
    -D COMMENT="LAN-first REST API and web panel for Midea air conditioners" \
    -D MAINTAINER="monikapurpl3 <monikapurpl3@users.noreply.github.com>" \
    -D FULLPKGPATH=comms/breeze-core \
    -d "$desc" -f "$plist" \
    -o "$OUT" "breeze-core-$VER" 2>/dev/null \
  || pkg_create -A "$ARCH" -B "$STAGE" -p /usr/local \
    -D COMMENT="LAN-first REST API and web panel for Midea air conditioners" \
    -D MAINTAINER="monikapurpl3 <monikapurpl3@users.noreply.github.com>" \
    -D FULLPKGPATH=comms/breeze-core \
    -d "$desc" -f "$plist" \
    "$OUT/breeze-core-$VER.tgz"

pkg="$OUT/breeze-core-$VER.tgz"
[ -f "$pkg" ] || pkg="$(ls "$OUT"/breeze-core-"$VER"*.tgz | head -1)"
echo "built: $pkg"

# --- signing ---------------------------------------------------------------
# pkg_add verifies signify signatures and refuses an unsigned package unless
# asked twice, so an unsigned package here would mean telling every user to
# override the check - which is worse than not shipping one.
if [ -n "$SECKEY" ]; then
    signed="$OUT/signed"
    mkdir -p "$signed"
    pkg_sign -s signify2 -s "$SECKEY" -o "$signed" -S "$OUT" 2>&1 | tail -3
    mv "$signed"/*.tgz "$OUT/" 2>/dev/null || true
    rmdir "$signed" 2>/dev/null || true
    echo "signed: $pkg"
else
    echo "NOT signed: no signify key given (pass it as the second argument)"
fi

ls -la "$OUT"
