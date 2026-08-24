#!/bin/sh
# Create the unprivileged service account before files are laid down: the
# package declares breeze:breeze ownership on /etc/breeze-core, and a package
# that names an account which does not exist yet fails mid-install.
set -e

if ! getent group breeze >/dev/null 2>&1; then
    if command -v groupadd >/dev/null 2>&1; then
        groupadd --system breeze
    else
        addgroup -S breeze          # busybox (Alpine, OpenWrt)
    fi
fi

if ! getent passwd breeze >/dev/null 2>&1; then
    if command -v useradd >/dev/null 2>&1; then
        useradd --system --gid breeze --no-create-home \
            --home-dir /etc/breeze-core --shell /sbin/nologin breeze
    else
        adduser -S -D -H -G breeze -h /etc/breeze-core -s /sbin/nologin breeze
    fi
fi

exit 0
