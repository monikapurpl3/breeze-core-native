#!/bin/sh
# Stop and disable the service ONLY on real removal — never on upgrade, or an
# `apt upgrade` / `dnf upgrade` would kill a running service, because the OLD
# package's pre-removal scriptlet also fires during an upgrade.
#
# How each packager signals removal versus upgrade in the first argument:
#   rpm  %preun : $1 = 0 on removal, >= 1 on upgrade
#   dpkg prerm  : $1 = remove|purge on removal, upgrade|deconfigure on upgrade
#   pacman      : pre_remove runs only on removal, with the old VERSION as $1
#   apk         : pre-deinstall likewise: only on removal, with the version
#
# The last two are why `*.*` is here. A version always has a dot, and nothing
# rpm or dpkg passes does. Until 4.3.0 this matched only 0|remove|purge, so
# removing the package on Arch, Alpine or OpenWrt (apk) left the service
# enabled and running a deleted binary.
case "${1:-}" in
    0|remove|purge|*.*)
        if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
            systemctl stop breeze-core >/dev/null 2>&1 || true
            systemctl disable breeze-core >/dev/null 2>&1 || true
        elif command -v rc-service >/dev/null 2>&1; then
            rc-service breeze-core stop >/dev/null 2>&1 || true
            rc-update del breeze-core default >/dev/null 2>&1 || true
        elif [ -x /etc/init.d/breeze-core ]; then
            /etc/init.d/breeze-core stop >/dev/null 2>&1 || true
            /etc/init.d/breeze-core disable >/dev/null 2>&1 || true
        fi
        ;;
esac
exit 0
