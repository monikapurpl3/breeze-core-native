#!/usr/bin/env bash
# Publish to the aspic host (static files only).
#
#   ./site/publish.sh                          # the pages, and nothing else
#   ./site/publish.sh --tree packaging/out/aspic   # pages + signed repositories
#   ASPIC_HOST=myhost ./site/publish.sh
#
# Uploads to a timestamped release directory and atomically swaps the `current`
# symlink nginx serves, keeping the last three releases for instant rollback.
# Needs plain ssh; no sudo — the web root is owned by the pushing user (see
# site/install-host.sh).
#
# Rolling back is deliberately not a flag here, because it is one command on the
# host and a flag would have to be as careful as everything below:
#
#   ssh mrrp 'cd /var/www/aspic && ln -sfn releases/<ts> current.new && mv -Tf current.new current'
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
HOST="${ASPIC_HOST:-mrrp}"
ROOT="${ASPIC_ROOT:-/var/www/aspic}"
URL="${ASPIC_URL:-https://aspic.salataputarica.hr.eu.org}"
TS="$(date -u +%Y%m%d-%H%M%S)"

# What the page-only mode publishes, named one by one.
#
# An allow-list, not an exclude-list: this directory also holds the nginx vhost
# and these scripts, and the failure mode of an exclude-list is that the next
# file added here gets published by accident. Adding a page means adding it here,
# which is the moment to think about whether it should be public at all.
WEB_FILES="index.html aspic.css favicon.svg breeze-core/index.html"

TREE=""
ALLOW_REMOVALS=0
while [ $# -gt 0 ]; do
  case "$1" in
    --tree) TREE="${2:?--tree needs a directory}"; shift 2 ;;
    # Only for a deliberate removal: see the vanish guard below.
    --allow-removals) ALLOW_REMOVALS=1; shift ;;
    *) echo "unknown argument: $1"; exit 2 ;;
  esac
done

STAGE="$(mktemp -d)"
PUBLISHED=0
cleanup() {
  rm -rf "$STAGE"
  [ "$PUBLISHED" = 1 ] || ssh "$HOST" \
    "case \"$TS\" in 20??????-??????) rm -rf \"$ROOT/releases/$TS\" ;; esac" >/dev/null 2>&1 || true
}
# A transfer that dies would otherwise leave a release directory that was never
# linked into `current`: harmless in itself, but it consumes one of the three
# rollback slots, and two failures in a row would prune the last good release.
trap cleanup EXIT

echo "=== staging ==="
if [ -n "$TREE" ]; then
  [ -d "$TREE" ] || { echo "PUBLISH ABORTED: $TREE is not a directory"; exit 1; }
  [ -f "$TREE/index.html" ] || { echo "PUBLISH ABORTED: $TREE has no index.html"; exit 1; }
  cp -R "$TREE"/. "$STAGE/"
  printf '  whole tree from %s (%s files, %s KB)\n' "$TREE" \
    "$(find "$STAGE" -type f | wc -l | tr -d ' ')" \
    "$(( $(du -sk "$STAGE" | cut -f1) ))"
else
  for f in $WEB_FILES; do
    [ -f "$HERE/$f" ] || { echo "PUBLISH ABORTED: $f is missing from $HERE"; exit 1; }
    mkdir -p "$STAGE/$(dirname "$f")"
    cp "$HERE/$f" "$STAGE/$f"
    printf '  %-24s %6s bytes\n' "$f" "$(wc -c < "$HERE/$f" | tr -d ' ')"
  done
fi

# Refuse to publish an unrendered template. Nothing here templates anything
# today, but the sibling host shipped a page reading "v@VERSION@" to the public
# internet once, and this is the check that would have caught it.
if grep -rl '@[A-Z_]\+@' "$STAGE" --include='*.html' >/dev/null 2>&1; then
  echo "PUBLISH ABORTED: unrendered placeholders:"
  grep -rn '@[A-Z_]\+@' "$STAGE" --include='*.html' | head -5 | sed 's/^/  /'
  exit 1
fi

# Refuse to publish a page that links to something not being published.
#
# The upload replaces the whole tree, so anything absent locally is absent live —
# there is no merge. On the sibling host that is exactly how four sections came
# to 404 for a while, unnoticed because the smoke check only tested the front
# page. Only root-relative links are checkable; external ones are somebody
# else's uptime.
missing=""
for link in $(grep -rhoE 'href="/[^"#]*"' "$STAGE" --include='*.html' | sed 's/href="//;s/"$//' | sort -u); do
  case "$link" in /) continue ;; esac
  [ -e "$STAGE${link%/}" ] || missing="$missing $link"
done
if [ -n "$missing" ]; then
  echo "PUBLISH ABORTED: a page links to paths that are not being published:"
  for m in $missing; do echo "  $m"; done
  echo "  Add them to WEB_FILES, publish the whole tree with --tree, or fix the link."
  exit 1
fi

# Cache-bust the stylesheet by content hash.
#
# The pages are sent no-cache but the stylesheet is cached for an hour, and every
# colour on them lives in that stylesheet — so a visitor arriving just after a
# publish can get new HTML with old CSS and a page drained of colour. There is no
# build step here to content-hash filenames with, so the hash goes in a query
# string instead, resolved at publish time rather than remembered by hand.
if [ -f "$STAGE/aspic.css" ]; then
  ver="$(sha256sum "$STAGE/aspic.css" | cut -c1-8)"
  find "$STAGE" -name '*.html' -type f | while read -r page; do
    sed "s|href=\"/aspic.css\"|href=\"/aspic.css?v=$ver\"|" "$page" > "$page.new"
    mv "$page.new" "$page"
  done
  echo "  stylesheet -> /aspic.css?v=$ver"
fi

# Refuse to publish key material. This tree goes to a public web root, which is
# the worst possible place for one — and this tree is now assembled by a script
# that also handles private keys, which makes the check less theoretical than it
# looks. Cheap, and the one failure here that is unrecoverable rather than
# merely embarrassing.
echo "=== checking for key material ==="
# The `|| true` is load-bearing: find and grep exit non-zero when they match
# nothing, and under `set -e` the happy path would end the script right here.
leaks="$(find "$STAGE" \( -name '*.key' -o -name '*.rsa' -o -name '*.sec' \
        -o -name 'id_*' -o -name 'gpg-private*' \) -print 2>/dev/null || true)"
leaks="$leaks$(grep -rl -- '-----BEGIN [A-Z ]*PRIVATE KEY-----' "$STAGE" 2>/dev/null || true)"
if [ -n "$(printf '%s' "$leaks" | tr -d '[:space:]')" ]; then
  echo "PUBLISH ABORTED: key material in the staged tree:"
  printf '%s\n' "$leaks" | sed '/^$/d;s/^/  /'
  exit 1
fi
echo "  none"

# Refuse to make something that is live disappear.
#
# Publishing replaces the whole tree, so a page-only push would silently delete
# every repository under it — one careless `./site/publish.sh` and `apt update`
# stops working for everyone. Compare the top level of what is live against what
# is staged, and stop if anything would vanish.
echo "=== checking nothing live would vanish ==="
live="$(ssh "$HOST" "ls -1 '$ROOT/current/' 2>/dev/null" || true)"
vanishing=""
for entry in $live; do
  [ -e "$STAGE/$entry" ] || vanishing="$vanishing $entry"
done
if [ -n "$vanishing" ]; then
  if [ "$ALLOW_REMOVALS" = 1 ]; then
    echo "  removing (asked for):$vanishing"
  else
    echo "PUBLISH ABORTED: these are live and are not in what you are publishing:"
    for v in $vanishing; do echo "  $v"; done
    echo "  Publish the whole tree instead:  ./site/publish.sh --tree packaging/out/aspic"
    echo "  Or, if the removal is intended:  --allow-removals"
    exit 1
  fi
else
  echo "  nothing"
fi

echo "=== publishing to $HOST:$ROOT/releases/$TS ==="
# tar, not rsync: the pages are kilobytes and the repository tree is tens of
# megabytes of already-compressed packages, so the delta-transfer machinery (and
# the Windows msys/rsync pairing it drags in) buys little. If this tree grows to
# the point where a republish is slow, copy the rsync branch out of the Python
# project's packaging/repo/publish.sh, which solved exactly that at 1.2 GB.
#
# No -z: the payload is packages that are already compressed.
tar -C "$STAGE" -cf - . | ssh "$HOST" "
  set -e
  mkdir -p '$ROOT/releases/$TS'
  tar -xf - -C '$ROOT/releases/$TS'
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
paths="/ /aspic.css /favicon.svg /breeze-core/"
# One entry point per repository family, so a family that failed to build is
# caught here rather than by the first person to try installing from it. The
# xbps and portage entries earn their place twice over: neither is a path any
# page links to, so the link checker above cannot see them, and the Gentoo one
# is a file git needs (info/refs) rather than one a browser would ever ask for.
[ -n "$TREE" ] && paths="$paths /aspic.asc /deb/dists/stable/InRelease /rpm/aspic.repo
  /alpine/x86_64/APKINDEX.tar.gz /xbps/x86_64-repodata /aspic-xbps.fingerprint
  /portage/breeze.git/info/refs"
for u in $paths; do
  printf '  %-34s ' "$u"
  curl -fsS --max-time 20 -o /dev/null -w '%{http_code}\n' "$URL$u" \
    || echo "(unreachable — check later)"
done
echo "published."
