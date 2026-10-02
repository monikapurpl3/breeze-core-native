#!/bin/sh
# Runs from the `configure` configd action after settings are saved: regenerate
# breeze_core.conf from the model, then start or stop to match.
set -e

CONF=/usr/local/etc/breeze-core
# Where OPNsense's boot looks as well as rc.subr -- see the template's comment.
RCCONF=/etc/rc.conf.d/breeze_core
install -d -o breeze -g breeze -m 750 "$CONF"
install -d -m 755 /etc/rc.conf.d
# Where 4.1.1 and earlier rendered it. rc.subr reads this one AFTER the one in
# /etc, so a copy left behind would quietly override every later Save.
rm -f /usr/local/etc/rc.conf.d/breeze_core

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
