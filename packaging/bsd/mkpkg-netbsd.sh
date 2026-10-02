#!/bin/sh
# Build a native NetBSD binary package (.tgz) with pkg_create(1) from the base
# system. Run ON NetBSD, from the source tree, after `cargo build --release`.
#
#   doas sh packaging/bsd/mkpkg-netbsd.sh [output-dir]
#
# Much shorter than the Python line's version of this file, and the reason is
# the whole point of the rewrite: there is no interpreter to depend on, no
# virtualenv to stage and no absolute paths baked into a bundle. The package is
# one executable, an rc.d script and an example env file — and it declares **no
# @pkgdep at all**, where the Python package needed python312.
set -eu

# Needs root: it stages files into the pkgsrc prefix, which is root-owned.
# Without this check the failure is a bare "Permission denied" three lines
# later, which reads like a broken script rather than a missing doas.
if [ "$(id -u)" != 0 ]; then
    echo "mkpkg-netbsd.sh writes into the pkgsrc prefix — run it as root:" >&2
    echo "    doas sh packaging/bsd/mkpkg-netbsd.sh [output-dir]" >&2
    exit 1
fi

OUT="${1:-.}"
VER="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
PREFIX=/usr/pkg
BIN=target/release/breeze-core
META="$(mktemp -d)"
trap 'rm -rf "$META"' EXIT

[ -x "$BIN" ] || { echo "no $BIN — run: cargo build --release"; exit 1; }

# Stage into the prefix. pkg_create reads the packing list relative to @cwd, so
# everything packaged has to actually live there at build time.
install -m 0755 "$BIN" "$PREFIX/bin/breeze-core"
mkdir -p "$PREFIX/share/examples/rc.d" "$PREFIX/share/examples/breeze-core"
install -m 0755 packaging/bsd/rc.netbsd "$PREFIX/share/examples/rc.d/breeze_core"
install -m 0644 packaging/nfpm/breeze-core.env \
    "$PREFIX/share/examples/breeze-core/breeze-core.env"

printf 'LAN-first REST API and web panel for Midea air conditioners\n' > "$META/COMMENT"
cat > "$META/DESC" <<'EOF'
Breeze Core - self-hosted, LAN-first control for Midea air conditioners.
REST API, web control panel, device-pairing authentication, server-side
schedules and temperature curves, and a diagnostic CLI. One static executable
with the panel compiled into it: no interpreter and no runtime dependencies.

Provides the breeze_core rc.d service (copied to /etc/rc.d on install; enable
it with breeze_core=YES in /etc/rc.conf). Configuration lives in
/usr/pkg/etc/breeze-core.
EOF

# INSTALL script: the service account and its configuration directory have to
# exist before anything runs, and the directory must NOT be removed on
# deinstall — a paired V3 unit's credentials cannot be obtained again.
cat > "$META/INSTALL" <<'EOF'
#!/bin/sh
ETCDIR=/usr/pkg/etc/breeze-core
case "$2" in
PRE-INSTALL)
	id breeze >/dev/null 2>&1 || \
	  useradd -c "Breeze Core service" -d /nonexistent -s /sbin/nologin breeze 2>/dev/null || true
	mkdir -p "$ETCDIR"
	;;
POST-INSTALL)
	chown -R breeze "$ETCDIR" 2>/dev/null || true
	chmod 750 "$ETCDIR" 2>/dev/null || true
	[ -f "$ETCDIR/breeze-core.env" ] || \
	  cp /usr/pkg/share/examples/breeze-core/breeze-core.env "$ETCDIR/breeze-core.env" 2>/dev/null || true
	cat <<'BANNER'

  Breeze Core installed.

    1. doas breeze-core pair --out /usr/pkg/etc/breeze-core/config.json
    2. edit /usr/pkg/etc/breeze-core/breeze-core.env  (BREEZE_HOST=...)
    3. echo 'breeze_core=YES' >> /etc/rc.conf && service breeze_core start

BANNER
	;;
esac
exit 0
EOF

# Build info, so pkg_add sees a matching OS and ABI — without it, or with a
# mismatch, pkg_add refuses the package outright.
#
# PKGTOOLS_VERSION is deliberately absent: pkg_add then prints a harmless
# "lacks pkg_install version data" note, whereas a value that does not match the
# host's exact pkg_install release makes it reject the package.
cat > "$META/BUILD_INFO" <<EOF
OPSYS=$(uname -s)
OS_VERSION=$(uname -r)
MACHINE_ARCH=$(uname -p)
EOF

# The packing list. No @pkgdep line: the binary is static.
plist="$META/PLIST"
{
    echo "@cwd $PREFIX"
    echo "bin/breeze-core"
    echo "share/examples/rc.d/breeze_core"
    echo "share/examples/breeze-core/breeze-core.env"
    echo "@exec [ -d /etc/rc.d ] && cp -f %D/share/examples/rc.d/breeze_core /etc/rc.d/breeze_core && chmod 755 /etc/rc.d/breeze_core"
    echo "@unexec rm -f /etc/rc.d/breeze_core"
} > "$plist"

mkdir -p "$OUT"
# pkg_create writes exactly the name given — it appends no suffix — so name the
# file directly.
pkg_create \
    -c "$META/COMMENT" \
    -d "$META/DESC" \
    -i "$META/INSTALL" \
    -f "$plist" \
    -B "$META/BUILD_INFO" \
    "$OUT/breeze-core-$VER.tgz"

ls -la "$OUT/breeze-core-$VER.tgz"
echo "built: $OUT/breeze-core-$VER.tgz"
