#!/bin/sh
#
#  ============================================================================
#   READ THIS BEFORE YOU RUN IT
#  ============================================================================
#
#   This script is about to run as root, from the internet, on a machine you
#   care about. That is exactly the shape of the thing you should not do
#   without looking first, and no amount of reassurance in a comment written by
#   the same people who wrote the script changes that.
#
#   So: read it. It is about 400 lines and most of them are comments. You are
#   looking for the parts that delete things, and there are five of them
#   (search for "run rm" and "run pkg"). The checksum published next to this
#   file proves the download was not corrupted; it does NOT prove the file is
#   trustworthy, because it comes from the same server. Only reading does that.
#
#   What it changes:
#
#     * removes the bolero package repository and its signing key
#     * adds the aspic package repository and its signing key
#     * upgrades the breeze-core package from 3.x (Python) to 4.x (native)
#     * stops the service before, starts it after -- and only if it was running
#
#   What it does not touch: your configuration. /etc/breeze-core is copied to a
#   timestamped backup before anything happens, and the upgrade keeps it in
#   place. A paired V3 unit's token and key cannot be re-issued by Midea, so
#   that directory is the one thing here that is genuinely irreplaceable.
#
#   Run it with no arguments first. That prints the plan and changes nothing.
#
#  ============================================================================
#
#   curl -fsSL https://aspic.salataputarica.hr.eu.org/migrate.sh -o migrate.sh
#   less migrate.sh          # <- the important step
#   sudo sh migrate.sh       # plan only
#   sudo sh migrate.sh --yes # do it
#
set -eu

ASPIC="${ASPIC_URL:-https://aspic.salataputarica.hr.eu.org}"
BOLERO="${BOLERO_URL:-https://bolero.salataputarica.hr.eu.org}"
WANT_VERSION="${BREEZE_WANT_VERSION:-4.0.0}"
TS="$(date -u +%Y%m%d-%H%M%S)"
BACKUP_ROOT="${BREEZE_BACKUP_DIR:-/var/backups}"
BACKUP=""

DO_IT=0
FORCE=0

# ---------------------------------------------------------------- output
# Plain ASCII and no colour codes unless we are on a terminal: this output ends
# up pasted into bug reports.
if [ -t 1 ]; then
    B="$(printf '\033[1m')"; R="$(printf '\033[31m')"; G="$(printf '\033[32m')"
    Y="$(printf '\033[33m')"; N="$(printf '\033[0m')"
else
    B=''; R=''; G=''; Y=''; N=''
fi
say()  { printf '%s\n' "$*"; }
head_() { printf '\n%s== %s%s\n' "$B" "$*" "$N"; }
ok()   { printf '  %sok%s   %s\n' "$G" "$N" "$*"; }
warn() { printf '  %swarn%s %s\n' "$Y" "$N" "$*"; }
die()  { printf '\n  %serror%s %s\n\n' "$R" "$N" "$*" >&2; exit 1; }
plan() { printf '  %s\n' "$*"; }

# Every command that changes the machine goes through this, so that a dry run is
# the same code path with one branch flipped -- and so the log says exactly what
# was done, in a form you can undo by hand.
run() {
    if [ "$DO_IT" = 1 ]; then
        printf '  + %s\n' "$*"
        # shellcheck disable=SC2294
        eval "$@" || die "that command failed. Your backup is at ${BACKUP:-<not yet made>}"
    else
        printf '  would: %s\n' "$*"
    fi
}

usage() {
    cat <<EOF
usage: sh migrate.sh [--yes] [--force] [--help]

  (no flags)  print the plan and change nothing
  --yes       actually do it
  --force     proceed even if this looks already migrated

Environment: ASPIC_URL, BOLERO_URL, BREEZE_BACKUP_DIR, BREEZE_WANT_VERSION.
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --yes|-y) DO_IT=1 ;;
        --force)  FORCE=1 ;;
        --dry-run) DO_IT=0 ;;
        --help|-h) usage; exit 0 ;;
        *) die "unknown argument '$1' (try --help)" ;;
    esac
    shift
done

# ---------------------------------------------------------------- downloader
# curl on most Linuxes, fetch on FreeBSD, ftp on NetBSD and OpenBSD. Picked
# once, here, so the rest of the script does not care.
DL=""
for c in curl wget fetch ftp; do
    if command -v "$c" >/dev/null 2>&1; then DL="$c"; break; fi
done
[ -n "$DL" ] || die "none of curl, wget, fetch or ftp is installed"
fetch_to() {  # fetch_to <url> <path>
    case "$DL" in
        curl)  curl -fsSL "$1" -o "$2" ;;
        wget)  wget -qO "$2" "$1" ;;
        fetch) fetch -qo "$2" "$1" ;;
        ftp)   ftp -o "$2" "$1" ;;
    esac
}

# ---------------------------------------------------------------- detection
OS="$(uname -s)"
MACHINE="$(uname -m)"
case "$MACHINE" in
    x86_64|amd64)  ARCH=amd64 ;;
    aarch64|arm64) ARCH=arm64 ;;
    armv7l|armv7)  ARCH=armhf ;;
    riscv64)       ARCH=riscv64 ;;
    ppc64le)       ARCH=ppc64le ;;
    s390x)         ARCH=s390x ;;
    *)             ARCH="$MACHINE" ;;
esac

DISTRO=""
FAMILY=""
if [ "$OS" = Linux ]; then
    if [ -r /etc/openwrt_release ]; then
        DISTRO=openwrt; FAMILY=opkg
    elif [ -r /etc/os-release ]; then
        # shellcheck disable=SC1091
        . /etc/os-release
        DISTRO="${ID:-linux}"
        case "${ID:-}${ID_LIKE:+ $ID_LIKE}" in
            *debian*|*ubuntu*)                 FAMILY=apt ;;
            *fedora*|*rhel*|*centos*)          FAMILY=dnf ;;
            *suse*|*sles*)                     FAMILY=zypper ;;
            *arch*)                            FAMILY=pacman ;;
            *alpine*)                          FAMILY=apk ;;
        esac
    fi
    # ID_LIKE is not always set (Alma sets it, Arch does not), so fall back to
    # asking which tool exists. Order matters: a machine with both dnf and apt
    # is a Debian box with dnf installed for some reason, not the reverse.
    if [ -z "$FAMILY" ]; then
        for t in apt-get dnf zypper pacman apk opkg; do
            if command -v "$t" >/dev/null 2>&1; then
                case "$t" in
                    apt-get) FAMILY=apt ;; dnf) FAMILY=dnf ;;
                    zypper) FAMILY=zypper ;; pacman) FAMILY=pacman ;;
                    apk) FAMILY=apk ;; opkg) FAMILY=opkg ;;
                esac
                break
            fi
        done
    fi
else
    DISTRO="$OS"
    case "$OS" in
        FreeBSD) FAMILY=pkg ;;
        NetBSD)  FAMILY=pkgin ;;
        OpenBSD) FAMILY=pkg_add ;;
    esac
fi
[ -n "$FAMILY" ] || die "could not work out which package manager this system uses (OS=$OS)"

# The BSDs keep their package tools off a login PATH: pkg_info, pkg_add and
# pkg_create live in /usr/pkg/sbin and /usr/sbin on NetBSD, /usr/local/sbin on
# FreeBSD. Without this, `pkg_info -e breeze-core` is simply not found, and the
# script concludes that nothing is installed and offers to install rather than
# upgrade -- which it did, on the first NetBSD run.
case "$OS" in
    NetBSD|OpenBSD|FreeBSD)
        PATH="$PATH:/usr/pkg/sbin:/usr/pkg/bin:/usr/sbin:/usr/local/sbin:/usr/local/bin"
        export PATH
        ;;
esac

# Where the stores live. AC_CONFIG_DIR wins, then the packaged default, then the
# pkgsrc prefix the BSD packages use.
CONFDIR="${AC_CONFIG_DIR:-}"
if [ -z "$CONFDIR" ]; then
    for d in /etc/breeze-core /usr/pkg/etc/breeze-core /usr/local/etc/breeze-core /etc/meow-ac; do
        if [ -d "$d" ]; then CONFDIR="$d"; break; fi
    done
fi
[ -n "$CONFDIR" ] || CONFDIR=/etc/breeze-core

# What is installed, and how.
INSTALLED=""
INSTALL_KIND=""
case "$FAMILY" in
    apt)    INSTALLED="$(dpkg-query -W -f='${Version}' breeze-core 2>/dev/null || true)" ;;
    dnf|zypper) INSTALLED="$(rpm -q --qf '%{VERSION}' breeze-core 2>/dev/null || true)" ;;
    pacman) INSTALLED="$(pacman -Q breeze-core 2>/dev/null | awk '{print $2}' || true)" ;;
    # `apk list -I` prints "breeze-core-3.2.0 x86_64 {...} (...) [installed]",
    # which is the only apk output with the version in a predictable place.
    apk)    INSTALLED="$(apk list -I breeze-core 2>/dev/null | head -1 | sed 's/^breeze-core-//;s/ .*//' || true)" ;;
    opkg)   INSTALLED="$(opkg list-installed breeze-core 2>/dev/null | awk '{print $3}' || true)" ;;
    pkg)    INSTALLED="$(pkg query %v breeze-core 2>/dev/null || true)" ;;
    pkgin|pkg_add) INSTALLED="$(pkg_info -e breeze-core 2>/dev/null | sed 's/breeze-core-//' || true)" ;;
esac
[ -n "$INSTALLED" ] && INSTALL_KIND=package

# A source install is a different migration, and pretending otherwise would
# leave a service unit pointing at a virtualenv that is no longer there.
SOURCE_HINT=""
for d in /opt/meow-ac /opt/breeze-core /usr/local/breeze-core; do
    [ -d "$d" ] && SOURCE_HINT="$d"
done
[ -d /usr/lib/breeze-core/venv ] && SOURCE_HINT=/usr/lib/breeze-core/venv

# The service, and whether it is running now (so it can be put back that way).
SVC_KIND=""
SVC_RUNNING=0
if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
    SVC_KIND=systemd
    systemctl is-active --quiet breeze-core 2>/dev/null && SVC_RUNNING=1
elif command -v rc-service >/dev/null 2>&1; then
    SVC_KIND=openrc
    rc-service breeze-core status >/dev/null 2>&1 && SVC_RUNNING=1
elif [ -x /etc/init.d/breeze-core ] && command -v procd >/dev/null 2>&1; then
    SVC_KIND=procd
    /etc/init.d/breeze-core running >/dev/null 2>&1 && SVC_RUNNING=1
elif [ -x /etc/rc.d/breeze_core ]; then
    SVC_KIND=bsdrc
    /etc/rc.d/breeze_core status >/dev/null 2>&1 && SVC_RUNNING=1
fi

svc() {  # svc start|stop
    case "$SVC_KIND" in
        systemd) run "systemctl $1 breeze-core" ;;
        openrc)  run "rc-service breeze-core $1" ;;
        procd)   run "/etc/init.d/breeze-core $1" ;;
        bsdrc)   run "service breeze_core $1" ;;
        *)       warn "no service manager found - $1 it yourself" ;;
    esac
}

# ---------------------------------------------------------------- the plan
head_ "what this machine is"
plan "os          $OS ($DISTRO), $MACHINE -> $ARCH"
plan "packages    $FAMILY"
plan "config      $CONFDIR"
plan "installed   ${INSTALLED:-nothing} ${INSTALL_KIND:+($INSTALL_KIND)}"
# Not ${SVC_RUNNING:+...}: that expands on any non-empty value, and this one is
# the string "0" when it is stopped.
if [ "$SVC_RUNNING" = 1 ]; then
    plan "service     ${SVC_KIND:-none}, running"
else
    plan "service     ${SVC_KIND:-none}, stopped"
fi

# Platforms aspic has nothing for yet. Said plainly rather than half-migrating.
case "$FAMILY" in
    pkg)
        die "aspic has no FreeBSD packages yet, so there is nothing to migrate to.
        Your bolero setup keeps working. Watch $ASPIC/breeze-core/ or use the
        binary tarball from the release." ;;
    pkg_add)
        die "aspic has no OpenBSD packages yet, so there is nothing to migrate to.
        Your existing install keeps working. Watch $ASPIC/breeze-core/." ;;
esac

if [ -n "$SOURCE_HINT" ] && [ -z "$INSTALLED" ]; then
    die "this looks like a source install ($SOURCE_HINT), not a packaged one.
        Migrating it means replacing a service unit that points at a virtualenv,
        which this script will not do behind your back. Install the package
        first (see $ASPIC), then remove the old tree by hand."
fi

if [ -z "$INSTALLED" ]; then
    warn "breeze-core is not installed here, so there is nothing to upgrade."
    warn "The repository swap below still applies if you want it."
fi

case "$INSTALLED" in
    4.*) if [ "$FORCE" = 0 ]; then
             head_ "nothing to do"
             ok "breeze-core $INSTALLED is already the native line"
             say ""
             say "  Pass --force if you want the repository swap done anyway."
             exit 0
         fi ;;
esac

head_ "what it will change"
plan "backup      $BACKUP_ROOT/breeze-core-migration-$TS/  (config + repo files + a rollback script)"
plan "remove      the bolero repository and its signing key"
plan "add         the aspic repository and its signing key"
plan "upgrade     breeze-core ${INSTALLED:-<none>} -> $WANT_VERSION"
say ""
plan "The package name does not change, so this is an upgrade in place rather"
plan "than a remove-and-install: your configuration, device tokens, programs and"
plan "timers stay exactly where they are, and the service stays enabled."

if [ "$DO_IT" = 0 ]; then
    head_ "this was a plan, not a migration"
    say ""
    say "  Nothing has been changed. To do it:"
    say ""
    if [ "$(id -u)" = 0 ]; then
        say "      sh migrate.sh --yes"
    else
        say "      sudo sh migrate.sh --yes"
    fi
    say ""
    say "  Read the script first. It is the only real check available to you."
    say ""
    exit 0
fi

# ---------------------------------------------------------------- guards
[ "$(id -u)" = 0 ] || die "this needs root (try: sudo sh migrate.sh --yes)"

# ---------------------------------------------------------------- backup
head_ "backing up"
BACKUP="$BACKUP_ROOT/breeze-core-migration-$TS"
# 0700, because config.json holds the API key and every unit's V3 token and key.
( umask 077; mkdir -p "$BACKUP/etc" "$BACKUP/repo" )
run "chmod 700 '$BACKUP'"

if [ -d "$CONFDIR" ]; then
    run "cp -R '$CONFDIR' '$BACKUP/etc/'"
    ok "copied $CONFDIR"
else
    warn "$CONFDIR does not exist - nothing to preserve"
fi

# The repository definition and key for this family, whatever it is called here.
BOLERO_FILES=""
case "$FAMILY" in
    apt)    BOLERO_FILES="/etc/apt/sources.list.d/breeze-core.list /usr/share/keyrings/breeze-core.gpg" ;;
    dnf|zypper) BOLERO_FILES="/etc/yum.repos.d/breeze-core.repo" ;;
    pacman) BOLERO_FILES="/etc/pacman.conf" ;;
    apk)    BOLERO_FILES="/etc/apk/repositories /etc/apk/keys/breeze-core@bolero.rsa.pub" ;;
    opkg)   BOLERO_FILES="/etc/opkg/customfeeds.conf" ;;
    pkgin)  BOLERO_FILES="/usr/pkg/etc/pkgin/repositories.conf" ;;
esac
for f in $BOLERO_FILES; do
    if [ -e "$f" ]; then run "cp -p '$f' '$BACKUP/repo/'"; ok "copied $f"; fi
done

# A manifest, so the backup explains itself a year from now.
if [ "$DO_IT" = 1 ]; then
    {
        echo "breeze-core migration backup"
        echo "date        $(date -u +%Y-%m-%dT%H:%M:%SZ)"
        echo "host        $(hostname 2>/dev/null || echo unknown)"
        echo "os          $OS $DISTRO $MACHINE ($ARCH)"
        echo "packages    $FAMILY"
        echo "from        breeze-core ${INSTALLED:-<none>}"
        echo "to          breeze-core $WANT_VERSION (aspic)"
        echo "config      $CONFDIR"
        echo "service     ${SVC_KIND:-none} (running before: $SVC_RUNNING)"
        echo ""
        echo "files kept here:"
        ( cd "$BACKUP" && find . -type f | sed 's/^/  /' )
    } > "$BACKUP/manifest.txt"
    chmod 600 "$BACKUP/manifest.txt"
fi

# And a rollback script, generated with this machine's actual values. Written
# rather than promised: "restore from your backup" is not instructions.
if [ "$DO_IT" = 1 ]; then
    cat > "$BACKUP/ROLLBACK.sh" <<ROLLBACK
#!/bin/sh
# Undo the breeze-core migration of $TS on $(hostname 2>/dev/null || echo this host).
# Read it before running it, same as the script that made it.
set -eu
[ "\$(id -u)" = 0 ] || { echo "needs root"; exit 1; }

echo "== stopping the service"
$(case "$SVC_KIND" in
    systemd) echo "systemctl stop breeze-core || true" ;;
    openrc)  echo "rc-service breeze-core stop || true" ;;
    procd)   echo "/etc/init.d/breeze-core stop || true" ;;
    bsdrc)   echo "service breeze_core stop || true" ;;
    *)       echo "# no service manager was detected" ;;
esac)

echo "== restoring the repository files and key"
for f in "$BACKUP"/repo/*; do
    [ -e "\$f" ] || continue
    case "\$(basename "\$f")" in
        breeze-core.list) cp -p "\$f" /etc/apt/sources.list.d/ ;;
        breeze-core.gpg)  cp -p "\$f" /usr/share/keyrings/ ;;
        breeze-core.repo) cp -p "\$f" /etc/yum.repos.d/ ;;
        pacman.conf)      cp -p "\$f" /etc/ ;;
        repositories)     cp -p "\$f" /etc/apk/ ;;
        breeze-core@bolero.rsa.pub) cp -p "\$f" /etc/apk/keys/ ;;
        customfeeds.conf) cp -p "\$f" /etc/opkg/ ;;
        repositories.conf) cp -p "\$f" /usr/pkg/etc/pkgin/ ;;
    esac
done

echo "== restoring the configuration"
cp -R "$BACKUP/etc/$(basename "$CONFDIR")" "$(dirname "$CONFDIR")/" 2>/dev/null || true

echo "== reinstalling breeze-core ${INSTALLED:-3.2.0} from bolero"
$(case "$FAMILY" in
    apt)    echo "apt-get update && apt-get install -y --allow-downgrades breeze-core=${INSTALLED:-3.2.0}" ;;
    dnf)    echo "dnf -y downgrade breeze-core || dnf -y install breeze-core-${INSTALLED:-3.2.0}" ;;
    zypper) echo "zypper --non-interactive install --oldpackage breeze-core-${INSTALLED:-3.2.0}" ;;
    pacman) echo "pacman -Sy --noconfirm breeze-core" ;;
    apk)    echo "apk update && apk add 'breeze-core=${INSTALLED:-3.2.0}-r0'" ;;
    opkg)   echo "opkg update && opkg install breeze-core" ;;
    pkgin)  echo "pkgin -y update && pkg_add -U $BOLERO/netbsd/All/breeze-core-${INSTALLED:-3.2.0}.tgz" ;;
esac)

echo "== done. Start it when you are happy:"
echo "   ${SVC_KIND:-your init system} start breeze-core"
ROLLBACK
    chmod 700 "$BACKUP/ROLLBACK.sh"
    ok "wrote $BACKUP/ROLLBACK.sh"
fi

if [ "$DO_IT" = 1 ]; then
    ( cd "$BACKUP_ROOT" && tar -czf "breeze-core-migration-$TS.tar.gz" "breeze-core-migration-$TS" 2>/dev/null ) || true
    [ -f "$BACKUP_ROOT/breeze-core-migration-$TS.tar.gz" ] && \
        chmod 600 "$BACKUP_ROOT/breeze-core-migration-$TS.tar.gz" && \
        ok "and $BACKUP_ROOT/breeze-core-migration-$TS.tar.gz"
fi

# ---------------------------------------------------------------- stop
if [ "$SVC_RUNNING" = 1 ]; then
    head_ "stopping the service"
    svc stop
fi

# ---------------------------------------------------------------- bolero out
head_ "removing the bolero repository"
case "$FAMILY" in
    apt)
        run "rm -f /etc/apt/sources.list.d/breeze-core.list"
        run "rm -f /usr/share/keyrings/breeze-core.gpg"
        ;;
    dnf|zypper)
        run "rm -f /etc/yum.repos.d/breeze-core.repo"
        [ "$FAMILY" = zypper ] && run "zypper removerepo breeze-core || true"
        # The imported key is an rpm package called gpg-pubkey-<id>. Matched on
        # its summary so nothing else on the system is touched.
        for k in $(rpm -q gpg-pubkey --qf '%{NAME}-%{VERSION}-%{RELEASE} %{SUMMARY}\n' 2>/dev/null | grep -i 'breeze' | awk '{print $1}'); do
            run "rpm -e '$k'"
        done
        ;;
    pacman)
        # Surgical: drop the [breeze-core] stanza and nothing else. pacman.conf
        # is the file that decides where this machine gets its software, so it
        # is edited through a temp file and only replaced if awk succeeded.
        if grep -q '^\[breeze-core\]' /etc/pacman.conf 2>/dev/null; then
            run "cp -p /etc/pacman.conf /etc/pacman.conf.pre-aspic"
            run "awk '/^\\[breeze-core\\]/{skip=1;next} /^\\[/{skip=0} !skip' /etc/pacman.conf > /tmp/pacman.conf.new"
            run "mv /tmp/pacman.conf.new /etc/pacman.conf"
        fi
        # The fingerprint by way of gpg against pacman's own keyring: parsing
        # `pacman-key --list-keys` output means parsing a human-readable block
        # where the uid and the fingerprint are on different lines.
        for f in $(gpg --homedir /etc/pacman.d/gnupg --list-keys --with-colons 2>/dev/null \
                   | awk -F: '/^pub/{fpr=""} /^fpr/{if(fpr=="")fpr=$10} /^uid/{if($10 ~ /Breeze Core/ && fpr!="")print fpr}' \
                   | sort -u || true); do
            run "pacman-key --delete '$f' || true"
        done
        ;;
    # The three line-oriented configs. Each is guarded on the file existing and
    # on it actually mentioning bolero: `grep -v` over a file with no match
    # writes an identical file, but `grep -v` over a file that is not there
    # fails, and under `set -e` that would end the migration on a machine that
    # simply never had bolero configured.
    apk)
        if grep -q bolero /etc/apk/repositories 2>/dev/null; then
            run "grep -v 'bolero' /etc/apk/repositories > /tmp/apkrepos && mv /tmp/apkrepos /etc/apk/repositories"
        fi
        run "rm -f '/etc/apk/keys/breeze-core@bolero.rsa.pub'"
        ;;
    opkg)
        if grep -q bolero /etc/opkg/customfeeds.conf 2>/dev/null; then
            run "grep -v 'bolero' /etc/opkg/customfeeds.conf > /tmp/feeds && mv /tmp/feeds /etc/opkg/customfeeds.conf"
        fi
        # opkg keys are named after their own fingerprint, so the only way to
        # find bolero's is to ask usign what each one is.
        if [ -d /etc/opkg/keys ] && command -v usign >/dev/null 2>&1; then
            run "rm -f /etc/opkg/keys/9a7eb487cbe301eb"
        fi
        ;;
    pkgin)
        if grep -q bolero /usr/pkg/etc/pkgin/repositories.conf 2>/dev/null; then
            run "grep -v 'bolero' /usr/pkg/etc/pkgin/repositories.conf > /tmp/pkgin.conf && mv /tmp/pkgin.conf /usr/pkg/etc/pkgin/repositories.conf"
        fi
        ;;
esac

# ---------------------------------------------------------------- aspic in
head_ "adding the aspic repository"
case "$FAMILY" in
    apt)
        run "fetch_to '$ASPIC/aspic.asc' /tmp/aspic.asc"
        run "gpg --dearmor < /tmp/aspic.asc > /usr/share/keyrings/aspic.gpg"
        run "rm -f /tmp/aspic.asc"
        run "printf '%s\\n' 'deb [signed-by=/usr/share/keyrings/aspic.gpg] $ASPIC/deb stable main' > /etc/apt/sources.list.d/aspic.list"
        run "apt-get -qq update"
        ;;
    dnf|zypper)
        run "rpm --import '$ASPIC/aspic.asc'"
        run "fetch_to '$ASPIC/rpm/aspic.repo' /etc/yum.repos.d/aspic.repo"
        # -y is load-bearing: without it dnf hits an interactive "Is this ok [y/N]"
        # for the key import, reads EOF from a piped script, declines -- and then
        # reports the repository as "Bad GPG signature", which sends you looking
        # for a signing problem that does not exist.
        if [ "$FAMILY" = dnf ]; then
            run "dnf -q -y makecache"
        else
            run "zypper --gpg-auto-import-keys refresh"
        fi
        ;;
    pacman)
        run "fetch_to '$ASPIC/aspic.asc' /tmp/aspic.asc"
        run "pacman-key --add /tmp/aspic.asc"
        run "pacman-key --lsign-key \"\$(gpg --show-keys --with-colons /tmp/aspic.asc | awk -F: '/^fpr/{print \$10; exit}')\""
        run "rm -f /tmp/aspic.asc"
        # Single-quoted echo, not printf.
        #
        # pacman.conf needs a literal $arch for pacman to substitute, and the
        # two shells disagree about how to produce one: dash's printf drops the
        # backslash in "\$", bash's keeps it. Arch is the one distribution here
        # whose /bin/sh IS bash, so a printf that tested clean everywhere else
        # wrote "Server = .../arch/\$arch" on the only machine that would ever
        # read it -- and pacman answered with a 404 for breeze-core.db.
        run "{ echo ''; echo '[aspic]'; echo 'SigLevel = Required'; echo 'Server = $ASPIC/arch/\$arch'; } >> /etc/pacman.conf"
        run "pacman -Sy --noconfirm"
        ;;
    apk)
        run "fetch_to '$ASPIC/aspic-alpine.rsa.pub' /etc/apk/keys/aspic-alpine.rsa.pub"
        run "printf '%s\\n' '$ASPIC/alpine' >> /etc/apk/repositories"
        run "apk update"
        ;;
    opkg)
        run "fetch_to '$ASPIC/aspic-usign.pub' /tmp/aspic-usign.pub"
        run "mkdir -p /etc/opkg/keys"
        run "cp /tmp/aspic-usign.pub \"/etc/opkg/keys/\$(usign -F -p /tmp/aspic-usign.pub)\""
        run ". /etc/openwrt_release && printf 'src/gz aspic $ASPIC/openwrt/%s\\n' \"\$DISTRIB_ARCH\" >> /etc/opkg/customfeeds.conf"
        run "opkg update"
        ;;
    pkgin)
        run "printf '%s\\n' '$ASPIC/netbsd/All' >> /usr/pkg/etc/pkgin/repositories.conf"
        run "pkgin -y update || true"
        ;;
esac

# ---------------------------------------------------------------- upgrade
head_ "upgrading breeze-core"
case "$FAMILY" in
    apt)    run "apt-get install -y --only-upgrade breeze-core || apt-get install -y breeze-core" ;;
    dnf)    run "dnf -y upgrade breeze-core || dnf -y install breeze-core" ;;
    zypper) run "zypper --non-interactive install breeze-core" ;;
    pacman) run "pacman -S --noconfirm breeze-core" ;;
    apk)    run "apk upgrade breeze-core || apk add breeze-core" ;;
    opkg)   run "opkg upgrade breeze-core || opkg install breeze-core" ;;
    pkgin)  run "pkg_add -U '$ASPIC/netbsd/All/breeze-core-$WANT_VERSION.tgz'" ;;
esac

# ---------------------------------------------------------------- verify
head_ "checking"
NOW=""
if command -v breeze-core >/dev/null 2>&1; then
    NOW="$(breeze-core --version 2>/dev/null | head -1 | awk '{print $2}' || true)"
elif [ -x /usr/pkg/bin/breeze-core ]; then
    NOW="$(/usr/pkg/bin/breeze-core --version 2>/dev/null | head -1 | awk '{print $2}' || true)"
fi
case "$NOW" in
    "$WANT_VERSION") ok "breeze-core $NOW is installed" ;;
    "")  die "breeze-core does not answer --version. Nothing else has been started;
        your backup and its ROLLBACK.sh are in $BACKUP" ;;
    *)   warn "expected $WANT_VERSION but got $NOW - carrying on, but check it" ;;
esac

if [ -f "$CONFDIR/config.json" ]; then
    ok "$CONFDIR/config.json is still there"
else
    warn "$CONFDIR/config.json is missing - restore it from $BACKUP before starting"
fi

# ---------------------------------------------------------------- start
if [ "$SVC_RUNNING" = 1 ]; then
    head_ "starting the service again"
    svc start
    # A short wait, then ask the server itself. Bound address unknown from here,
    # so loopback is the one thing worth trying automatically.
    i=0
    while [ "$i" -lt 10 ]; do
        sleep 1
        if fetch_to "http://127.0.0.1:8420/api/health" /tmp/bc-health 2>/dev/null &&
           grep -q ok /tmp/bc-health 2>/dev/null; then
            ok "it answers /api/health"
            break
        fi
        i=$((i + 1))
    done
    rm -f /tmp/bc-health
    if [ "$i" = 10 ]; then
        warn "no answer on 127.0.0.1:8420 - that is expected if it binds a LAN"
        warn "address; check with: breeze-core diag --config $CONFDIR/config.json"
    fi
else
    warn "the service was not running before, so it has been left stopped"
fi

head_ "done"
say ""
say "  breeze-core $NOW, from aspic."
say "  Backup:   $BACKUP"
say "  Rollback: sh $BACKUP/ROLLBACK.sh"
say ""
say "  That backup holds your API key and every unit's V3 credentials. It is"
say "  mode 600 in a root-only directory; keep a copy somewhere off this machine"
say "  and delete it here when you are satisfied."
say ""
