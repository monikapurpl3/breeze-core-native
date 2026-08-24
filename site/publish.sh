#!/usr/bin/env bash
# Publish the aspic project index to the host (static files only).
#
#   ./site/publish.sh                    # push to the default host
#   ASPIC_HOST=myhost ./site/publish.sh
#
# Uploads to a timestamped release directory and atomically swaps the `current`
# symlink nginx serves, keeping the last three releases for instant rollback.
# Needs plain ssh; no sudo — the web root is owned by the pushing user (see
# site/install-host.sh).
#
# Rolling back is deliberately not a flag here, because it is one command on the
# host and a flag would need to be as careful as everything below:
#
#   ssh mrrp 'cd /var/www/aspic && ln -sfn releases/<ts> current.new && mv -Tf current.new current'
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
HOST="${ASPIC_HOST:-mrrp}"
ROOT="${ASPIC_ROOT:-/var/www/aspic}"
URL="${ASPIC_URL:-https://aspic.salataputarica.hr.eu.org}"
TS="$(date -u +%Y%m%d-%H%M%S)"

# What goes on the public web, named one by one.
#
# An allow-list, not an exclude-list: this directory also holds the nginx vhost
# and these scripts, and the failure mode of an exclude-list is that the next
# file added here is published by accident. Adding a page means adding it here,
# which is the moment to think about whether it should be public at all.
WEB_FILES="index.html aspic.css favicon.svg"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

echo "=== staging ==="
for f in $WEB_FILES; do
  [ -f "$HERE/$f" ] || { echo "PUBLISH ABORTED: $f is missing from $HERE"; exit 1; }
  cp "$HERE/$f" "$STAGE/$f"
  printf '  %-14s %6s bytes\n' "$f" "$(wc -c < "$HERE/$f" | tr -d ' ')"
done

# Refuse to publish an unrendered template. Nothing here templates anything
# today, but bolero shipped a page reading "v@VERSION@" to the public internet
# once, and this is the check that would have caught it.
if grep -rl '@[A-Z_]\+@' "$STAGE" >/dev/null 2>&1; then
  echo "PUBLISH ABORTED: unrendered placeholders:"
  grep -rn '@[A-Z_]\+@' "$STAGE" | head -5 | sed 's/^/  /'
  exit 1
fi

# Refuse to publish a page that links to something not being published.
#
# The upload replaces the whole tree, so anything absent locally is absent
# live — there is no merge. On the sibling host that is exactly how four
# sections came to 404 for a while, unnoticed because the smoke check only
# tested the front page. Only root-relative links are checkable; external ones
# are somebody else's uptime.
missing=""
for link in $(grep -ohE 'href="/[^"#]*"' "$STAGE"/*.html | sed 's/href="//;s/"$//' | sort -u); do
  case "$link" in
    /) continue ;;                       # the front page itself
  esac
  [ -e "$STAGE${link%/}" ] || missing="$missing $link"
done
if [ -n "$missing" ]; then
  echo "PUBLISH ABORTED: the page links to paths that are not being published:"
  for m in $missing; do echo "  $m"; done
  echo "  Add them to WEB_FILES, or fix the link."
  exit 1
fi

# Cache-bust the stylesheet, after the link check above has seen the clean href.
#
# The page is sent no-cache but the stylesheet is cached for an hour, and every
# accent colour on the page lives in that stylesheet — so a visitor who arrives
# just after a publish can get new HTML with old CSS and a page drained of every
# colour it has. There is no build step here to content-hash filenames with, so
# the content hash goes in a query string instead: a different cache key for a
# different file, resolved at publish time rather than remembered by hand.
if grep -q 'href="/aspic.css"' "$STAGE/index.html"; then
  ver="$(sha256sum "$STAGE/aspic.css" | cut -c1-8)"
  sed "s|href=\"/aspic.css\"|href=\"/aspic.css?v=$ver\"|" "$STAGE/index.html" > "$STAGE/index.new"
  mv "$STAGE/index.new" "$STAGE/index.html"
  echo "  stylesheet -> /aspic.css?v=$ver"
fi

# Refuse to publish key material. This tree goes to a public web root, which is
# the worst possible place for one. Cheap, and the one check whose absence is
# unrecoverable rather than merely embarrassing.
echo "=== checking for key material ==="
# The `|| true` is load-bearing: grep and find exit non-zero when they match
# nothing, and under `set -e` the happy path would end the script right here.
leaks="$(find "$STAGE" \( -name '*.key' -o -name '*.rsa' -o -name '*.sec' -o -name 'id_*' \) -print 2>/dev/null || true)"
leaks="$leaks$(grep -rl -- '-----BEGIN [A-Z ]*PRIVATE KEY-----' "$STAGE" 2>/dev/null || true)"
if [ -n "$(printf '%s' "$leaks" | tr -d '[:space:]')" ]; then
  echo "PUBLISH ABORTED: key material in the staged tree:"
  printf '%s\n' "$leaks" | sed '/^$/d;s/^/  /'
  exit 1
fi
echo "  none"

echo "=== publishing to $HOST:$ROOT/releases/$TS ==="
# tar, not rsync: this tree is a handful of kilobytes, so the delta-transfer
# machinery (and the Windows msys/rsync pairing it drags in) buys nothing. When
# the first package repository lands here that calculus changes — copy the rsync
# branch out of the Python project's packaging/repo/publish.sh at that point.
#
# A transfer that dies would otherwise leave a release directory that was never
# linked into `current`: harmless in itself, but it consumes one of the three
# rollback slots, and two failures in a row would prune the last good release.
PUBLISHED=0
trap 'rm -rf "$STAGE"; [ "$PUBLISHED" = 1 ] || ssh "$HOST" "case \"$TS\" in 20??????-??????) rm -rf \"$ROOT/releases/$TS\" ;; esac" >/dev/null 2>&1 || true' EXIT

tar -C "$STAGE" -czf - . | ssh "$HOST" "
  set -e
  mkdir -p '$ROOT/releases/$TS'
  tar -xzf - -C '$ROOT/releases/$TS'
  chmod -R u+rwX,go+rX '$ROOT/releases/$TS'
  # tar restores the SOURCE directory's mtime onto the release directory, so a
  # fresh release can look older than its predecessors. That is why the prune
  # below sorts by name: sorting by time once deleted the new release out from
  # under the symlink and left the site 404ing.
  touch '$ROOT/releases/$TS'
  ln -sfn 'releases/$TS' '$ROOT/current.new' && mv -Tf '$ROOT/current.new' '$ROOT/current'
  cd '$ROOT/releases'
  keep=\"\$(basename \"\$(readlink '$ROOT/current')\")\"
  ls -1d */ | sed 's#/\$##' | sort -r | tail -n +4 | while read -r d; do
    [ \"\$d\" = \"\$keep\" ] || rm -rf -- \"\$d\"
  done
  test -f '$ROOT/current/index.html' || { echo 'PUBLISH BROKEN: current/index.html missing'; exit 1; }
  echo 'live releases:' && ls -1d */ | sort -r | head -3
" || { echo "PUBLISH FAILED — check '$ROOT/current' on $HOST"; exit 1; }

PUBLISHED=1

echo "=== smoke check ==="
# Non-fatal: the swap has already happened, and this host is on a domestic
# connection reached through a NAT hairpin, so a slow curl is not a failure.
for u in / /aspic.css /favicon.svg; do
  printf '  %-14s ' "$u"
  curl -fsS --max-time 20 -o /dev/null -w '%{http_code}\n' "$URL$u" \
    || echo "(unreachable — check later)"
done
echo "published."
