#!/bin/sh
# Runs from the `configure` configd action after settings are saved: regenerate
# breeze_core.conf from the model, then start or stop to match.
set -e

CONF=/usr/local/etc/breeze-core
RCCONF=/usr/local/etc/rc.conf.d/breeze_core
install -d -o breeze -g breeze -m 750 "$CONF"
# The directory rc.subr reads from, in case nothing else has created it yet.
install -d -m 755 /usr/local/etc/rc.conf.d

/usr/local/sbin/configctl template reload OPNsense/BreezeCore >/dev/null 2>&1 || true

# The rendered file, at the path rc.subr reads -- the same one the rc script
# picks up, so this decision and the service's own settings cannot disagree.
# shellcheck source=/dev/null
. "$RCCONF" 2>/dev/null || true

if [ "${breeze_core_enable}" = "YES" ]; then
    /usr/local/etc/rc.d/breeze_core onerestart >/dev/null 2>&1 || \
        /usr/local/etc/rc.d/breeze_core onestart
else
    /usr/local/etc/rc.d/breeze_core onestop >/dev/null 2>&1 || true
fi
exit 0
