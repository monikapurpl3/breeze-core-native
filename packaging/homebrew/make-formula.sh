#!/usr/bin/env bash
# Write the Homebrew formula for one release, for macOS and Linux alike.
#
#   packaging/homebrew/make-formula.sh VERSION BASE-URL DIST-DIR > breeze-core.rb
#
# DIST-DIR holds the four tarballs the formula installs, as build-binaries.sh
# names them: breeze-core-VERSION-{macos-arm64,macos-x86_64,linux-arm64,
# linux-amd64}.tar.zst. They are hashed here, so the formula pins exactly those
# bytes. BASE-URL is where they will be served: aspic's /homebrew/dist for a
# release (build-repo.sh), a file:// directory in CI.
#
# The service's breeze-core.env is the Linux packages' own
# (packaging/nfpm/breeze-core.env), so a setting means the same everywhere.
set -euo pipefail
[ $# -eq 3 ] || { echo "usage: $0 VERSION BASE-URL DIST-DIR" >&2; exit 2; }
ver="$1"; base="${2%/}"; dist="$3"
here="$(cd "$(dirname "$0")" && pwd)"

sha() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}
subst=()
for t in macos-arm64 macos-x86_64 linux-arm64 linux-amd64; do
  f="$dist/breeze-core-$ver-$t.tar.zst"
  [ -f "$f" ] || { echo "no $f" >&2; exit 1; }
  key="SHA_$(echo "$t" | tr 'a-z-' 'A-Z_')"
  subst+=(-e "s#@$key@#$(sha "$f")#g")
done

# The env file, indented into the formula's heredoc, its first line saying
# what reads it here. CRs dropped: the formula is Ruby, read on macOS or Linux.
env_file="$(mktemp)"
trap 'rm -f "$env_file"' EXIT
tr -d '\r' < "$here/../nfpm/breeze-core.env" \
  | sed '1s/.*/# Breeze Core service configuration, read when `brew services` starts it./' \
  | sed 's/^/      /; s/^ *$//' > "$env_file"

out="$(tr -d '\r' < "$here/breeze-core.rb.in" | awk -v envf="$env_file" '
  $0 == "@ENV@" { while ((getline line < envf) > 0) print line; next }
  { print }
' | sed -e "s#@VER@#$ver#g" -e "s#@BASE@#$base#g" "${subst[@]}")"

# A placeholder left over is a formula that installs nothing.
if grep -q '@[A-Z_0-9]*@' <<< "$out"; then
  echo "unfilled placeholders:" >&2; grep -o '@[A-Z_0-9]*@' <<< "$out" | sort -u >&2; exit 1
fi
printf '%s\n' "$out"
