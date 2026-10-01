#!/bin/sh
# Create the unprivileged service account before files are laid down: the
# package declares breeze:breeze ownership on /etc/breeze-core, and a package
# that names an account which does not exist yet fails mid-install.
#
# Three toolkits, because no two families agree:
#
#   groupadd/useradd   shadow-utils (Debian, Fedora, Arch, Void, ...)
#   addgroup/adduser   busybox, as Alpine builds it
#   /lib/functions.sh  OpenWrt, whose busybox has NONE of the above and no
#                      getent either -- it is how OpenWrt's own packages add
#                      their users. Until 4.3.0 this script exited 127 there,
#                      so the OpenWrt package never had its account and procd,
#                      told to run the service as breeze, could not start it.
set -e

# Whether the account exists, with or without getent.
has() {   # has group|passwd NAME
    if command -v getent >/dev/null 2>&1; then
        getent "$1" "$2" >/dev/null 2>&1
    else
        grep -qs "^$2:" "/etc/$1"
    fi
}

if ! has group breeze; then
    if command -v groupadd >/dev/null 2>&1; then
        groupadd --system breeze
    elif command -v addgroup >/dev/null 2>&1; then
        addgroup -S breeze          # busybox (Alpine)
    elif [ -f /lib/functions.sh ]; then
        . /lib/functions.sh         # OpenWrt
        # Its helpers take /var/lock/group and /var/lock/passwd, which the
        # boot scripts create: a chroot or a container that never booted has
        # no /var/lock, and the lock -- and with it the account -- fails.
        mkdir -p /var/lock 2>/dev/null || true
        group_add_next breeze >/dev/null
    fi
fi

if ! has passwd breeze; then
    if command -v useradd >/dev/null 2>&1; then
        useradd --system --gid breeze --no-create-home \
            --home-dir /etc/breeze-core --shell /sbin/nologin breeze
    elif command -v adduser >/dev/null 2>&1; then
        adduser -S -D -H -G breeze -h /etc/breeze-core -s /sbin/nologin breeze
    elif [ -f /lib/functions.sh ]; then
        . /lib/functions.sh         # OpenWrt: next free uid from 32768
        user_add breeze "" "$(group_add_next breeze)" "Breeze Core" /etc/breeze-core /bin/false
    fi
fi

exit 0
