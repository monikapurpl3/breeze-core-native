#!/usr/bin/env bash
# Build and optionally push the two container images.
#
#   ./packaging/container/build-images.sh              # build locally, no push
#   ./packaging/container/build-images.sh --push        # build and push to GHCR
#
# Two images, amd64 + arm64 each:
#   ghcr.io/monikapurpl3/breeze-core-native:<ver>         distroless, no shell
#   ghcr.io/monikapurpl3/breeze-core-native:<ver>-debug   the same, plus busybox
#
# This script exists because the images were built by hand for 4.0.1. The one
# artifact in this repository with no build step was the Windows zip, and it was
# the one artifact that shipped stale — a release step that lives only in
# somebody's shell history is a release step that eventually gets skipped or
# typed differently.
#
# The build context is packaging/out, so the image ships the SAME binary the
# packages ship rather than one compiled here.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"

VERSION="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
IMAGE="ghcr.io/monikapurpl3/breeze-core-native"
PLATFORMS="linux/amd64,linux/arm64"
PUSH=0
[ "${1:-}" = "--push" ] && PUSH=1

# amd64 and arm64 only. armv7, riscv64, ppc64le and s390x all have packages,
# but a container on those is a rounding error next to the registry storage and
# the build time, and nobody has asked.
for a in amd64 arm64; do
  [ -f "packaging/out/bin/$a/breeze-core" ] || {
    echo "!! no $a binary — run ./packaging/build-binaries.sh $a"; exit 1; }
  # Same staleness guard as the package builders: the cross-built binaries are
  # not rebuilt by `cargo build`, so editing the source and running only this
  # would ship the previous build under the new tag.
  newer="$(find crates static Cargo.toml Cargo.lock -newer "packaging/out/bin/$a/breeze-core" -type f -print -quit 2>/dev/null || true)"
  [ -z "$newer" ] || { echo "!! the $a binary is older than the source ($newer)"; exit 1; }
done

echo "=== breeze-core $VERSION → $IMAGE"
echo "    platforms: $PLATFORMS"
[ "$PUSH" -eq 1 ] && echo "    pushing" || echo "    local build only (--push to publish)"

# buildx needs a builder that can do more than the host architecture. The
# default "docker" driver cannot, and fails with "multiple platforms feature is
# currently not supported" rather than falling back.
if ! docker buildx inspect bc-multi >/dev/null 2>&1; then
  echo "=== creating the bc-multi buildx builder"
  docker buildx create --name bc-multi --driver docker-container --bootstrap >/dev/null
fi

build() {
  local dockerfile="$1" suffix="$2"
  local tag="$IMAGE:$VERSION$suffix"
  echo
  echo "=== $tag"
  local out=(--output "type=image,push=false")
  [ "$PUSH" -eq 1 ] && out=(--push)
  # No `latest`. Every tag here names a version on purpose: `latest` on a
  # daemon that controls heating is an upgrade nobody asked for at a moment
  # nobody chose.
  docker buildx build \
    --builder bc-multi \
    -f "$dockerfile" \
    --platform "$PLATFORMS" \
    --provenance=false \
    -t "$tag" \
    "${out[@]}" \
    packaging/out
}

build packaging/container/Dockerfile       ""
build packaging/container/Dockerfile.debug "-debug"

if [ "$PUSH" -eq 0 ]; then
  echo
  echo "built, not pushed. Re-run with --push when you mean it."
  exit 0
fi

# --- the check that matters --------------------------------------------------
#
# These images must be PULLABLE BY ANYONE. They were private while the project
# was unreleased; since 4.0.2 the containers wiki page documents a bare
# `docker pull` with no `docker login`, so a package that has silently gone
# private makes published instructions wrong.
#
# Worth knowing in both directions: GHCR visibility is per-package and
# independent of the repository's, and it is sticky — pushing does not change
# it. "New packages default to private" is true only of a package that does not
# exist yet. There is also no REST endpoint for it: `gh api -X PATCH
# user/packages/container/<name> -f visibility=…` returns 404. It is web-UI
# only, which is why this script checks rather than sets.
#
# Tested with an ANONYMOUS token fetched from ghcr.io/token, not with
# `docker manifest inspect`: Docker Desktop's credential helper bypasses
# DOCKER_CONFIG, so a "logged out" docker command here is still authenticated
# and cannot tell public from private at all.
echo
echo "=== anonymous pull check (these images must be public)"
unreachable=0
for tag in "$VERSION" "$VERSION-debug"; do
  tok="$(curl -fsS "https://ghcr.io/token?service=ghcr.io&scope=repository:monikapurpl3/breeze-core-native:pull" 2>/dev/null \
        | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')"
  code="$(curl -s -o /dev/null -w '%{http_code}' \
    -H "Authorization: Bearer ${tok:-none}" \
    -H "Accept: application/vnd.oci.image.index.v1+json" \
    "https://ghcr.io/v2/monikapurpl3/breeze-core-native/manifests/$tag" || true)"
  if [ "$code" = "200" ]; then
    echo "  ok  $IMAGE:$tag pulls anonymously (HTTP 200)"
  else
    echo "  !! $IMAGE:$tag is NOT anonymously pullable (HTTP $code)"
    unreachable=1
  fi
done

if [ "$unreachable" -eq 1 ]; then
  echo
  echo "The published install instructions assume no login. Set the package"
  echo "public at (there is no API for this):"
  echo "  https://github.com/users/monikapurpl3/packages/container/breeze-core-native/settings"
  exit 1
fi

echo
echo "pushed, and both tags verified anonymously pullable."
