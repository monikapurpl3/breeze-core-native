#!/bin/sh
# Service launcher for the rc script.
#
# The environment is set HERE, not in the rc script, and that is load-bearing:
# daemon(8) invoked with -u calls setusercontext(), which RESETS the
# environment. Anything exported from start_precmd never reaches the server, and
# because daemon runs with -f the reason goes nowhere an admin will look.
#
# One variable, not four: the server derives config.json, devices.json,
# programs.json and timers.json from AC_CONFIG_DIR unless each is named
# individually. The Python line needed PYTHONPATH here as well, and a venv
# interpreter to exec; this execs one static binary.
#
#   serve.sh <host> <port> [config-path]
set -eu

CONF="${3:-/usr/local/etc/breeze-core/config.json}"
AC_CONFIG_DIR="$(dirname "$CONF")"
export AC_CONFIG_DIR
# Named explicitly too, so a breeze_core_config pointing somewhere unusual is
# honoured rather than silently rounded to its directory's default.
AC_CONFIG="$CONF"
export AC_CONFIG

# The GUI's extra environment, rendered by configd from the model. Read line by
# line and exported as data - never sourced, because sourcing would run
# whatever was typed into the box as shell. The model already refuses bad
# lines; these checks are for a file edited by hand.
ENVF="$AC_CONFIG_DIR/service.env"
if [ -r "$ENVF" ]; then
    CR="$(printf '\r')"
    while IFS= read -r line || [ -n "$line" ]; do
        line="${line%"$CR"}"
        case "$line" in ''|'#'*) continue ;; esac
        case "$line" in *=*) ;; *) continue ;; esac
        name="${line%%=*}"
        value="${line#*=}"
        case "$name" in ''|[0-9]*|*[!A-Za-z0-9_]*) continue ;; esac
        case "$name" in
            AC_CONFIG|AC_CONFIG_DIR|AC_DEVICES|AC_PROGRAMS|AC_TIMERS|AC_BEHIND_PROXY|BREEZE_HOST|BREEZE_PORT) continue ;;
        esac
        export "$name=$value"
    done < "$ENVF"
fi

exec /usr/local/bin/breeze-core serve --host "${1:-127.0.0.1}" --port "${2:-8420}"
