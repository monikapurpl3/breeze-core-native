#!/usr/bin/env bash
# Write a Metalink 4 file (RFC 5854) beside each download in an aspic tree,
# for aria2 and other metalink-aware clients.
#
#   ./site/metalinks.sh packaging/out/aspic
#
# publish.sh runs this before every --tree publish; the rules are in
# site/metalinks.conf. For each file a rule matches, <file>.meta4 holds:
#
#   - its size and SHA-256, so the client checks the whole download;
#   - a SHA-256 per 1 MiB piece, so a segmented download is checked piece by
#     piece and one bad mirror's piece is fetched again from another;
#   - its aspic URL, and each mirror proven to serve the same bytes.
#
# With it, `aria2c https://aspic.../x.meta4` downloads from aspic and GitHub
# at once. aria2 recognises a metalink by the .meta4 suffix, so nothing in the
# vhost has to change for this.
#
# Deterministic: no timestamps, so an unchanged tree gives unchanged files
# and comparing a local tree with the live one still works.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
TREE="${1:?usage: metalinks.sh <tree>}"
CONF="${METALINKS_CONF:-$HERE/metalinks.conf}"
URL="${ASPIC_URL:-https://aspic.salataputarica.hr.eu.org}"
PIECE=$((1024 * 1024))

[ -d "$TREE" ] || { echo "!! $TREE is not a directory"; exit 1; }
cd "$TREE"

# Every metalink is regenerated: a file that is gone must not keep one.
find . -name '*.meta4' -type f -delete

# A GitHub release's assets and their SHA-256s, fetched once per release.
declare -A RELEASES
release_digest() { # owner/repo tag asset
  local key="$1@$2"
  if [ -z "${RELEASES[$key]+set}" ]; then
    RELEASES[$key]="$(gh api "repos/$1/releases/tags/$2" \
      --jq '.assets[] | "\(.name) \(.digest)"' 2>/dev/null || true)"
  fi
  printf '%s\n' "${RELEASES[$key]}" | awk -v n="$3" '$1 == n { sub(/^sha256:/, "", $2); print $2 }'
}

# Does $1 serve a file whose SHA-256 is $2?
serves_same() {
  local url="$1" want="$2" got=""
  if [[ "$url" =~ ^https://github\.com/([^/]+/[^/]+)/releases/download/([^/]+)/([^/]+)$ ]] &&
    command -v gh >/dev/null 2>&1; then
    got="$(release_digest "${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}" "${BASH_REMATCH[3]}")"
  else
    got="$(curl -fsSL --max-time 300 "$url" 2>/dev/null | sha256sum | cut -d' ' -f1 || true)"
  fi
  [ "$got" = "$want" ]
}

echo "=== metalinks ==="
written=0
while read -r glob mirror _; do
  [[ -z "${glob:-}" || "$glob" == \#* ]] && continue
  matched=0
  # The glob is matched against the tree, the same way a shell would.
  for f in $glob; do
    [ -f "$f" ] || continue
    matched=1
    name="$(basename "$f")"
    # Names go into XML and URLs unescaped, so only plain ones are taken.
    if ! [[ "$f" =~ ^[A-Za-z0-9._/+-]+$ ]]; then
      echo "  !! $f: a name this script won't put in XML unescaped; skipped"
      continue
    fi
    size="$(stat -c %s "$f")"
    sha="$(sha256sum "$f" | cut -d' ' -f1)"
    ver="$(printf '%s' "$name" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)"
    urls="    <url priority=\"1\">$URL/$f</url>"
    via="aspic"
    if [ "$mirror" != "-" ]; then
      if [ -z "$ver" ] && [[ "$mirror" == *"{ver}"* ]]; then
        echo "  !! $f: no version in the name for {ver}; aspic only"
      else
        m="${mirror//\{name\}/$name}"
        m="${m//\{ver\}/$ver}"
        if serves_same "$m" "$sha"; then
          urls+=$'\n'"    <url priority=\"2\">$m</url>"
          via="aspic + ${m#https://}"
          via="${via%%/releases/*}"
        else
          echo "  !! $m is not the same file, or not there; aspic only"
        fi
      fi
    fi
    pieces="$(split -b "$PIECE" --filter='sha256sum | cut -d" " -f1' "$f" |
      sed 's#.*#      <hash>&</hash>#')"
    cat > "$f.meta4" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="$name">
    <size>$size</size>
    <hash type="sha-256">$sha</hash>
    <pieces length="$PIECE" type="sha-256">
$pieces
    </pieces>
$urls
  </file>
</metalink>
EOF
    chmod 644 "$f.meta4"
    written=$((written + 1))
    printf '  %-58s %s\n' "$f" "$via"
  done
  [ "$matched" = 1 ] || echo "  !! no file matches $glob -- has it moved?"
done < "$CONF"
echo "  $written metalink(s)"
