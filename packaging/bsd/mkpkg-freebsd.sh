#!/bin/sh
# Build a native FreeBSD package with pkg-create(8). Run ON FreeBSD, from the
# source tree, after `cargo build --release`.
#
#   sh packaging/bsd/mkpkg-freebsd.sh [output-dir]
#
# No root needed: everything is staged in a temporary directory and pkg reads it
# from there, unlike the NetBSD script which has to place files inside the
# pkgsrc prefix before pkg_create can see them.
#
# The package declares **no dependencies**. The Python one needed
# lang/pythonNNN, matched to whatever the virtualenv had been built against,
# which is also why it could not be installed on a machine with a different
# Python.
set -eu

OUT="${1:-.}"
VER="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
BIN=target/release/breeze-core
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

[ -x "$BIN" ] || { echo "no $BIN - run: cargo build --release"; exit 1; }

# The package root. /usr/local is where FreeBSD packages live, and the rc.d
# script and example env go with it.
mkdir -p "$STAGE/usr/local/bin" \
         "$STAGE/usr/local/etc/rc.d" \
         "$STAGE/usr/local/share/examples/breeze-core" \
         "$STAGE/usr/local/share/doc/breeze-core"
install -m 0755 "$BIN" "$STAGE/usr/local/bin/breeze-core"
install -m 0755 packaging/bsd/rc.freebsd "$STAGE/usr/local/etc/rc.d/breeze_core"
install -m 0644 packaging/nfpm/breeze-core.env \
    "$STAGE/usr/local/share/examples/breeze-core/breeze-core.env"
install -m 0644 LICENSE "$STAGE/usr/local/share/doc/breeze-core/LICENSE"

# plist: every staged file, as an absolute path.
plist="$STAGE/.plist"
( cd "$STAGE" && find usr -type f -o -type l | sed 's#^#/#' ) | sort > "$plist"

# The manifest is UCL, not YAML, and `desc` is one quoted line. The scripts
# block is where the service account and the configuration directory come from:
# a package that names an account which does not exist fails half-installed, and
# a configuration directory removed on deinstall takes a paired V3 unit's
# credentials with it.
manifest="$STAGE/.manifest"
cat > "$manifest" <<'MANIFEST'
name = "breeze-core";
origin = "comms/breeze-core";
comment = "LAN-first REST API and web panel for Midea air conditioners";
maintainer = "monikapurpl3@users.noreply.github.com";
www = "https://github.com/monikapurpl3/breeze-core";
prefix = "/";
licenselogic = "single";
licenses = [ "AGPLv3+" ];
categories = [ "comms" ];
desc = "Self-hosted, LAN-first control for Midea air conditioners - REST API, web control panel, device-pairing authentication, server-side schedules and temperature curves, and a diagnostic CLI. One static executable with the panel compiled into it: no interpreter and no runtime dependencies. Provides the breeze_core rc.d service; configuration lives in /usr/local/etc/breeze-core.";
scripts {
  pre-install = <<EOS
if ! pw usershow breeze >/dev/null 2>&1; then
    pw groupadd breeze -q || true
    pw useradd breeze -g breeze -d /nonexistent -s /usr/sbin/nologin \
       -c "Breeze Core service" -q || true
fi
mkdir -p /usr/local/etc/breeze-core
EOS
  post-install = <<EOS
chown -R breeze:breeze /usr/local/etc/breeze-core 2>/dev/null || true
chmod 750 /usr/local/etc/breeze-core 2>/dev/null || true
if [ ! -f /usr/local/etc/breeze-core/breeze-core.env ]; then
    cp /usr/local/share/examples/breeze-core/breeze-core.env \
       /usr/local/etc/breeze-core/breeze-core.env 2>/dev/null || true
fi
cat <<'BANNER'

  Breeze Core installed.

    1. doas breeze-core pair --out /usr/local/etc/breeze-core/config.json
    2. edit /usr/local/etc/breeze-core/breeze-core.env  (BREEZE_HOST=...)
    3. sysrc breeze_core_enable=YES && service breeze_core start

BANNER
EOS
  post-deinstall = <<EOS
if [ -d /usr/local/etc/breeze-core ]; then
    echo "breeze-core: kept /usr/local/etc/breeze-core (config + device tokens)."
    echo "breeze-core: a paired V3 unit's credentials cannot be re-issued, so"
    echo "breeze-core: delete that directory by hand only if you are sure."
fi
EOS
}
MANIFEST
# The version is added separately rather than interpolated into the heredoc
# above: that heredoc is quoted so the shell scripts inside it survive intact,
# which also means $VER would not expand in it.
printf 'version = "%s";\n' "$VER" >> "$manifest"

mkdir -p "$OUT"
pkg create -o "$OUT" -r "$STAGE" -M "$manifest" -p "$plist"
ls -la "$OUT"/breeze-core-"$VER".pkg
echo "built: $OUT/breeze-core-$VER.pkg"
