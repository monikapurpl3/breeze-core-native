#!/bin/sh
# Post-install: refresh the service manager, and print the quickstart on a
# first install only.
#
# Nothing here relabels the binary. The Python package had to: its executable
# lived in /usr/lib (a directory of interpreter and dependencies), which is
# lib_t on SELinux and not a domain-transition entrypoint, so the service ran as
# init_t and its writes to /etc/breeze-core were denied silently. One static
# binary goes in /usr/bin, which is already bin_t.
set -e

# Ownership of the state directory, enforced here rather than trusted to the
# archive.
#
# nfpm's declared owner/group survives into the .deb, .rpm and .apk, but its
# archlinux packager drops it: /etc/breeze-core arrives root:root, and the
# service — which runs as breeze — then cannot write devices.json. Pairing fails
# with a 500 and nothing on the machine explains why. Doing it here covers every
# packager, including any that starts behaving the same way later.
if getent passwd breeze >/dev/null 2>&1; then
    chown breeze:breeze /etc/breeze-core 2>/dev/null || true
    chmod 750 /etc/breeze-core 2>/dev/null || true
    if [ -f /etc/breeze-core/breeze-core.env ]; then
        # Readable by the service, writable only by root: an admin edits it and
        # the daemon only ever reads it.
        chown root:breeze /etc/breeze-core/breeze-core.env 2>/dev/null || true
        chmod 640 /etc/breeze-core/breeze-core.env 2>/dev/null || true
    fi
fi

if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
    systemctl daemon-reload >/dev/null 2>&1 || true
    START='sudo systemctl enable --now breeze-core'
elif command -v rc-update >/dev/null 2>&1; then
    START='sudo rc-update add breeze-core default && sudo rc-service breeze-core start'
elif [ -x /etc/init.d/breeze-core ] && command -v procd >/dev/null 2>&1; then
    START='/etc/init.d/breeze-core enable && /etc/init.d/breeze-core start'
else
    START='start the breeze-core service with your init system'
fi

# Upgrade vs fresh install: rpm %post passes $1=1 (install) or >=2 (upgrade);
# dpkg postinst passes "configure" with the OLD version in $2 on upgrade (empty
# on a first install). On an upgrade, refresh a running service so the new
# binary takes effect, and skip the banner.
_upgrade=0
if [ "${1:-}" = "configure" ]; then
    [ -n "${2:-}" ] && _upgrade=1
elif [ "${1:-0}" -ge 2 ] 2>/dev/null; then
    _upgrade=1
fi

if [ "$_upgrade" -eq 1 ]; then
    if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
        systemctl try-restart breeze-core >/dev/null 2>&1 || true   # only if running
    elif command -v rc-service >/dev/null 2>&1; then
        rc-service breeze-core status >/dev/null 2>&1 && \
            rc-service breeze-core restart >/dev/null 2>&1 || true
    fi
    exit 0
fi

cat <<BANNER

  Breeze Core installed.

  1. Pair your AC units (writes /etc/breeze-core/config.json):
         sudo breeze-core pair
  2. For direct LAN use, set this machine's LAN IP in
         /etc/breeze-core/breeze-core.env   (BREEZE_HOST=...)
     or keep 127.0.0.1 if a reverse proxy fronts it.
  3. Start it:
         $START

  Then open http://<BREEZE_HOST>:8420 and pair a browser or the app.
  Docs: https://github.com/monikapurpl3/breeze-core

BANNER
exit 0
