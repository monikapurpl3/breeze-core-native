#!/usr/bin/env bash
# Refuse to build from files whose line endings are not what they should be.
#
# This repository is checked out on Windows with core.autocrlf=true, and a
# checkout rewrites every text file that .gitattributes does not pin to LF.
# That is how the first 4.3.1 build shipped a breeze-core.env with a CR on
# every line. BREEZE_PORT became "8420\r", and every init that sources the file
# (Termux, OpenRC, procd) failed to start the server with "'8420' is not a
# port". systemd strips the CR, so the Debian and RPM checks still passed. The
# index was LF all along; only the copies on disk, which the builders read,
# were not.
#
# Everything a package or the published tree takes from the working tree is
# pinned to LF in .gitattributes. This checks the files on disk agree.
#
#   packaging/check-eol.sh        (the builders call it; exits 1 on a mismatch)
set -euo pipefail
cd "$(dirname "$0")/.."

# `git ls-files --eol` prints "i/<index> w/<worktree> attr/<attributes>\t<path>".
bad="$(git ls-files --eol | awk -F'\t' '$1 ~ /w\/crlf/ && $1 ~ /eol=lf/ {print $2}')"
if [ -n "$bad" ]; then
  echo "!! pinned to LF in .gitattributes, but CRLF on disk:"
  printf '     %s\n' $bad
  echo "   A package built now would carry the CRs. Rewrite them from the index:"
  echo "     rm $(echo $bad) && git checkout -- $(echo $bad)"
  exit 1
fi
