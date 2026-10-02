#!/usr/bin/env bash
# One-time host setup for aspic.salataputarica.hr.eu.org. Idempotent: safe to
# re-run after editing aspic.conf, and it will do nothing it has already done.
#
#   scp -r site/ mrrp:~/aspic-site/
#   ssh mrrp 'sudo bash ~/aspic-site/install-host.sh'
#
# What it does, in order, and why that order:
#
#   1. the web root, owned by the pushing user so publishing needs no sudo
#   2. the SELinux label, because /var/www on this box is var_t and a directory
#      created under it inherits that — which nginx cannot read
#   3. the certificate, which needs a :80 server block to answer the ACME
#      challenge, but which the real vhost cannot be installed without: so a
#      bootstrap HTTP-only vhost goes in first and is replaced afterwards
#   4. the real vhost
#   5. renewal that actually works — a deploy hook to reload nginx, and the
#      cron entry that invokes certbot
#   6. the scanner jail, which watches a fixed list of logs and would otherwise
#      never see this vhost's dedicated one
set -euo pipefail

DOMAIN="${DOMAIN:-aspic.salataputarica.hr.eu.org}"
ROOT="${ROOT:-/var/www/aspic}"
CONF="${CONF:-$(cd "$(dirname "$0")" && pwd)/aspic.conf}"
VHOST="/etc/nginx/conf.d/aspic.conf"
LIVE="/etc/letsencrypt/live/$DOMAIN"
# The account that will publish. Under sudo that is the invoking user, not root:
# the whole point of the ownership is that publish.sh never needs sudo.
PUSH_USER="${PUSH_USER:-${SUDO_USER:-$(id -un)}}"

[ "$(id -u)" = 0 ] || { echo "run me under sudo — this edits /etc"; exit 1; }
[ -f "$CONF" ] || { echo "vhost source not found: $CONF"; exit 1; }
command -v nginx >/dev/null || { echo "nginx is not installed"; exit 1; }
CERTBOT="$(command -v certbot || true)"
[ -n "$CERTBOT" ] || { echo "certbot is not on PATH"; exit 1; }
id "$PUSH_USER" >/dev/null || { echo "no such user: $PUSH_USER"; exit 1; }

say() { printf '\n=== %s\n' "$*"; }

# --- 1. web root ------------------------------------------------------------
say "web root $ROOT (owned by $PUSH_USER)"
mkdir -p "$ROOT/releases"
chown "$PUSH_USER":"$(id -gn "$PUSH_USER")" "$ROOT" "$ROOT/releases"
chmod 755 "$ROOT" "$ROOT/releases"
ls -ld "$ROOT" "$ROOT/releases" | sed 's/^/  /'

# --- 2. SELinux -------------------------------------------------------------
# /var/www is var_t on this host, so a new subdirectory inherits var_t and
# nginx (httpd_t) is denied every read under it — a 403 with nothing in the
# nginx error log to explain it. The generic /var/www(/.*)? rule would give the
# right type, but only if something asks for it, so ask explicitly and pin it
# the way /var/www/bac-calculator already is.
if command -v selinuxenabled >/dev/null && selinuxenabled; then
  say "SELinux label for $ROOT"
  if semanage fcontext -l 2>/dev/null | grep -qF "${ROOT}(/.*)?"; then
    echo "  fcontext rule already present"
  else
    semanage fcontext -a -t httpd_sys_content_t "${ROOT}(/.*)?" \
      && echo "  added fcontext: ${ROOT}(/.*)? -> httpd_sys_content_t"
  fi
  restorecon -RF "$ROOT"
  ls -Zd "$ROOT" | sed 's/^/  /'
else
  say "SELinux disabled — nothing to label"
fi

# --- 3. certificate ---------------------------------------------------------
# The real vhost references the certificate, so nginx will not even validate
# its config until the certificate exists. Hence the bootstrap: a :80-only
# server block, which is all the ACME http-01 challenge needs.
if [ -s "$LIVE/fullchain.pem" ]; then
  say "certificate already present for $DOMAIN"
  openssl x509 -noout -subject -enddate -in "$LIVE/cert.pem" | sed 's/^/  /'
else
  say "bootstrap vhost (HTTP only) so the ACME challenge can be answered"
  cat > "$VHOST" <<BOOTSTRAP
# TEMPORARY bootstrap vhost written by site/install-host.sh — it exists only so
# certbot has a server block for $DOMAIN to answer the http-01 challenge in.
# The real configuration replaces this a few seconds later.
server {
    listen 80;
    listen [::]:80;
    server_name $DOMAIN;
    server_tokens off;
    location ^~ /.well-known/acme-challenge/ { root /var/www/letsencrypt; }
    location / { return 404; }
}
BOOTSTRAP
  nginx -t && systemctl reload nginx

  say "requesting the certificate"
  # --nginx as the *authenticator* only: certbot must not edit the vhost, which
  # is deployed from the repo. The reload after a renewal is the deploy hook
  # installed below, which covers every certificate on this host rather than
  # depending on certbot's installer being recorded for each one.
  "$CERTBOT" certonly --nginx --non-interactive --key-type ecdsa -d "$DOMAIN"
  openssl x509 -noout -subject -enddate -in "$LIVE/cert.pem" | sed 's/^/  /'
fi

# --- 4. the real vhost ------------------------------------------------------
say "installing $VHOST"
install -m 0644 -o root -g root "$CONF" "$VHOST"
nginx -t
systemctl reload nginx
echo "  reloaded"

# --- 5. renewal that works --------------------------------------------------
# Two separate problems, both silent until a certificate expires.
#
# A renewed certificate is useless while nginx holds the old one in memory. A
# deploy hook in this directory runs for every certificate this host renews, so
# it fixes the reload for the older vhosts too — several of which were issued
# with `certonly` and have no installer recorded to do it for them.
HOOK=/etc/letsencrypt/renewal-hooks/deploy/reload-nginx.sh
say "renewal deploy hook"
mkdir -p "$(dirname "$HOOK")"
if [ -x "$HOOK" ]; then
  echo "  already installed"
else
  cat > "$HOOK" <<'HOOKEOF'
#!/bin/sh
# Reload nginx after any certificate renewal. Installed by breeze-core-native
# site/install-host.sh. Runs once per renewed lineage; a reload is cheap and
# idempotent, so no attempt is made to deduplicate.
set -e
nginx -t && systemctl reload nginx
HOOKEOF
  chmod 0755 "$HOOK"
  echo "  installed $HOOK"
fi

# And the renewal has to actually run. This host renews from root's crontab,
# and the path in it can go stale — certbot moved from the distro package in
# /usr/bin to a pip virtualenv exposed as /usr/local/bin/certbot, which left the
# cron entry pointing at a file that no longer exists. Nothing complains: the
# certificates simply stop renewing until one expires.
say "renewal schedule"
CRON="$(crontab -l 2>/dev/null || true)"
if printf '%s\n' "$CRON" | grep -q 'certbot renew'; then
  bad="$(printf '%s\n' "$CRON" | grep 'certbot renew' | grep -oE '(/[^ ]*)?certbot' | sort -u)"
  fixed=0
  for p in $bad; do
    case "$p" in
      /*) [ -x "$p" ] || { CRON="$(printf '%s\n' "$CRON" | sed "s#$p renew#$CERTBOT renew#")"; fixed=1;
                           echo "  $p does not exist -> $CERTBOT"; } ;;
    esac
  done
  if [ "$fixed" = 1 ]; then
    printf '%s\n' "$CRON" | crontab -
    echo "  root crontab updated"
  else
    echo "  ok: $(printf '%s\n' "$CRON" | grep 'certbot renew' | head -1)"
  fi
  # Prove it, rather than assuming: a dry run exercises the whole path.
  "$CERTBOT" renew --dry-run --cert-name "$DOMAIN" 2>&1 | tail -3 | sed 's/^/  /'
else
  echo "  WARNING: no 'certbot renew' entry in root's crontab and no systemd"
  echo "  timer was found. This certificate will expire in 90 days."
fi

# --- 6. scanner jail --------------------------------------------------------
# The nginx-scan jail reads a fixed logpath, and every vhost on this box that
# was given its own access log fell out of its view — including the one that is
# being probed for /.git/config right now. Add this vhost's log; the filter
# matches specific probe paths (wp-, .env, .git, cgi-bin …) and nothing a
# package manager ever requests, so a repository client cannot trip it.
JAIL=/etc/fail2ban/jail.local
LOG=/var/log/nginx/aspic.access.log
say "fail2ban nginx-scan jail"
if [ ! -f "$JAIL" ]; then
  echo "  no $JAIL — skipping"
elif grep -q "$LOG" "$JAIL"; then
  echo "  already watched"
else
  # Append to the jail's existing logpath as a continuation line, which is how
  # fail2ban expresses several paths for one jail.
  #
  # The variable is not called `log`: that is a gawk built-in function, and
  # passing -v log=… is a fatal error rather than a shadowed name.
  if awk -v logfile="$LOG" '
    /^\[nginx-scan\]/ { injail = 1 }
    injail && /^logpath/ { print; print "           " logfile; inserted = 1; injail = 0; next }
    { print }
    END { if (!inserted) exit 3 }
  ' "$JAIL" > "$JAIL.new"; then
    # Backed up only now that there is a replacement worth making: an earlier
    # draft copied first and left a stray .bak behind on every failed attempt.
    cp -a "$JAIL" "$JAIL.bak.$(date +%Y%m%d-%H%M%S)"
    mv "$JAIL.new" "$JAIL"
    chmod 0644 "$JAIL"
    touch "$LOG"
    systemctl reload fail2ban || systemctl restart fail2ban
    fail2ban-client status nginx-scan | sed 's/^/  /'
  else
    # Left untouched rather than guessed at: this file is what bans people.
    echo "  could not find the nginx-scan logpath to extend — left untouched"
    rm -f "$JAIL.new"
  fi
fi

say "done"
echo "  https://$DOMAIN/ serves $ROOT/current"
if [ ! -e "$ROOT/current" ]; then
  echo "  nothing published yet — run site/publish.sh from the repo"
fi
