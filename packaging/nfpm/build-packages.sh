#!/usr/bin/env bash
# Wrap the built binaries as native packages: .deb, .rpm, .pkg.tar.zst, .apk
# and .ipk. Runs nfpm in a container so file modes and ownership are a Linux
# filesystem's rather than whatever a Windows checkout happens to have.
#
#   ./packaging/nfpm/build-packages.sh              # every built architecture
#   ./packaging/nfpm/build-packages.sh amd64
#
# Input:  packaging/out/bin/<label>/breeze-core   (build-binaries.sh)
# Output: packaging/out/pkg/
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
BIN="packaging/out/bin"
OUT="packaging/out/pkg"
VERSION="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
# Package release -- the trailing -N. Bump it when the packaged bytes change
# but the version does not, so a package manager can see the rebuild as newer:
#   BC_RELEASE=2 ./packaging/nfpm/build-packages.sh
RELEASE="${BC_RELEASE:-1}"

# label | nfpm arch | OpenWrt arch labels (space-separated, or "-")
#
# One binary per architecture serves every packager, because the Linux builds
# are static musl. The OpenWrt column exists because opkg matches its own
# architecture names, which nfpm has never heard of: the ipks differ only in
# that field, so they are produced by re-running nfpm with BC_ARCH set to each.
ARCHES="
amd64|amd64|x86_64
arm64|arm64|aarch64_generic aarch64_cortex-a53 aarch64_cortex-a72
armv7|arm7|arm_cortex-a7_neon-vfpv4
riscv64|riscv64|riscv64_riscv64
ppc64le|ppc64le|-
s390x|s390x|-
"

want=("$@")
selected() {
  [ ${#want[@]} -eq 0 ] && return 0
  for w in "${want[@]}"; do [ "$w" = "$1" ] && return 0; done
  return 1
}

# The nfpm image, built once. Pinned: nfpm's apk and ipk packagers are young
# enough that a floating "latest" has changed output between patch releases.
docker build -q -t bc-nfpm packaging/nfpm >/dev/null
echo "packaging image ready"

# Docker Desktop on Windows wants C:/... where git-bash says /c/...
MOUNT="$REPO"
case "$MOUNT" in /[a-z]/*) MOUNT="$(echo "$MOUNT" | sed -E 's#^/([a-z])/#\U\1:/#')" ;; esac

# ...and git-bash rewrites any argument that looks like a Unix path on its way
# to a Windows executable, so `-w /work` arrived as `C:/Program Files/Git/work`
# and docker refused it. This turns that translation off for the whole script;
# the paths here are already in the form docker wants.
export MSYS_NO_PATHCONV=1

mkdir -p "$OUT"
made=0

while IFS='|' read -r label nfpm_arch owrt; do
  [ -n "$label" ] || continue
  selected "$label" || continue
  binary="$BIN/$label/breeze-core"
  if [ ! -f "$binary" ]; then
    # Loud, not skipped: a missing architecture that says nothing is how a
    # release ships with one fewer package than its own page advertises.
    echo "!! no binary for $label — run packaging/build-binaries.sh $label"
    continue
  fi
  # Refuse to wrap a stale binary.
  #
  # The cross-built binaries are not rebuilt by `cargo build`, so editing the
  # source and running only this script packages the previous build under the
  # new version number. That happened: a .deb went into a verification
  # container carrying a CLI three commits old, and the only reason it was
  # caught is that the test asserted on the help text. Mtimes are enough here —
  # every source change touches a file.
  newer="$(find crates static Cargo.toml Cargo.lock -newer "$binary" -type f -print -quit 2>/dev/null || true)"
  if [ -n "$newer" ]; then
    echo "!! the $label binary is older than the source ($newer)"
    echo "   run: ./packaging/build-binaries.sh $label"
    exit 1
  fi

  echo
  echo "=== $label ($nfpm_arch)"
  # One container invocation per architecture: the repository read-only at
  # /work, the binary staged at the fixed /stage path nfpm.yaml names, and the
  # finished packages streamed back out as a tar on fd 3.
  #
  # Nothing is bind-mounted writable, because Docker Desktop on this machine
  # does not reliably see a directory the host created moments earlier: writing
  # into one failed here as "Open /out/breeze-core_4.0.0_amd64.deb:
  # input/output error" and produced zero packages, and the repository builder
  # hit the same thing as a phantom mkdir failure. Streaming has no such race.
  # No -i, and stdin from /dev/null: the loop around this reads $ARCHES from a
  # here-string, and a container that inherits that stdin eats the remaining
  # architectures. The first version of this built amd64 and then stopped
  # without a word.
  docker run --rm \
    -v "$MOUNT:/work:ro" \
    -e BC_VERSION="$VERSION" \
    -e BC_RELEASE="$RELEASE" \
    -w /work \
    bc-nfpm sh -eu -c "
      exec 3>&1 1>&2
      mkdir -p /stage /tmp/out
      install -m 0755 '/work/$binary' /stage/breeze-core
      for packager in deb rpm archlinux apk; do
        BC_ARCH='$nfpm_arch' nfpm package -f packaging/nfpm/nfpm.yaml -p \$packager -t /tmp/out
      done
      # ipk, once per OpenWrt architecture label.
      if [ '$owrt' != '-' ]; then
        for a in $owrt; do
          BC_ARCH=\"\$a\" nfpm package -f packaging/nfpm/nfpm.yaml -p ipk -t /tmp/out
        done
      fi
      tar -cf - -C /tmp/out . >&3
    " < /dev/null | tar -xf - -C "$OUT"
  made=$((made + 1))
done <<< "$ARCHES"

[ "$made" -gt 0 ] || { echo "nothing to package"; exit 1; }

echo
echo "=== packages ==="
ls -1 "$OUT" | while read -r f; do
  printf '  %-52s %7s KB\n' "$f" "$(( ($(wc -c < "$OUT/$f") + 1023) / 1024 ))"
done

# Verify the compression actually took. nfpm accepts unknown keys silently, so
# "compression: zstd" in the config proves nothing on its own -- and a package
# is the wrong place to find out that a claim in the release notes was wrong.
echo
echo "=== compression (asserted, not assumed) ==="
docker run --rm -v "$MOUNT/$OUT:/pkg:ro" bc-nfpm sh -eu -c '
  cd /pkg
  for f in *.deb; do
    [ -e "$f" ] || continue
    inner=$(ar t "$f" | grep "^data.tar" || true)
    printf "  %-46s %s\n" "$f" "$inner"
    case "$inner" in *.zst) ;; *) echo "  !! $f is not zstd"; exit 1 ;; esac
  done
  for f in *.rpm; do
    [ -e "$f" ] || continue
    # Asked of rpm itself rather than guessed at from the bytes: the compressor
    # is a header tag, and an earlier version of this check grepped the first
    # 4 KB for the word and reported a false failure on a package that was
    # perfectly fine.
    compressor=$(rpm -qp --qf '%{PAYLOADCOMPRESSOR}' "$f" 2>/dev/null)
    printf "  %-46s payload %s\n" "$f" "$compressor"
    [ "$compressor" = zstd ] || { echo "  !! $f is $compressor, not zstd"; exit 1; }
  done
  for f in *.pkg.tar.zst; do
    [ -e "$f" ] || continue
    printf "  %-46s zstd (by format)\n" "$f"
  done
  for f in *.apk *.ipk; do
    [ -e "$f" ] || continue
    printf "  %-46s gzip (the format has no zstd)\n" "$f"
  done
'
echo "done."
