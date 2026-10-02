#!/usr/bin/env bash
# Pack the committed source for a build on another machine.
#
#   ./packaging/bsd/ship-source.sh OUT.tar.gz
#
# `git archive HEAD` -- never a tar of the working tree. The working tree holds
# packaging/repo/keys/, every private signing key this project has, and
# git-ignored is not tar-ignored: until 4.1.0 the BSD build scripts tarred `.`
# minus target/, packaging/out/ and .git/, and so copied all six private keys
# to every BSD machine on every build, where they stayed. Only tracked,
# committed files can leave this machine this way, whatever is lying in the
# directory.
#
# It also means a remote build is a build of HEAD, which is what a release
# needs anyway: `--version` names the commit, and a binary built from edits
# that were never committed reports a commit that does not contain them.
set -euo pipefail

out="${1:?usage: ship-source.sh OUT.tar.gz}"

if ! git diff --quiet HEAD --; then
  echo "!! the tree has uncommitted changes, and a remote build ships HEAD only." >&2
  echo "   Commit them first, or they will not be in what gets built." >&2
  exit 1
fi

git archive --format=tar.gz -o "$out" HEAD

# A key committed by mistake must still not travel.
if tar -tzf "$out" | grep -E '(^|/)packaging/repo/keys/|\.(sec|rsa|pem|key)$|private\.asc$'; then
  rm -f "$out"
  echo "!! refusing to ship: the archive contains key material (listed above)" >&2
  exit 1
fi
