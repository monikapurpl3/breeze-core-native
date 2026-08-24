#!/bin/sh
# Config, device tokens, programs and timers are deliberately KEPT — an upgrade
# or a reinstall picks them straight back up, and losing a paired V3 unit's
# credentials means Midea will not issue them again. Say how to purge instead.
if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
    systemctl daemon-reload >/dev/null 2>&1 || true
fi
if [ -d /etc/breeze-core ]; then
    echo "breeze-core: kept /etc/breeze-core (config + device tokens)." >&2
    echo "breeze-core: delete it manually for a full wipe: rm -rf /etc/breeze-core" >&2
fi
exit 0
