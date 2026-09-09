#!/usr/bin/env bash
# Build Void Linux .xbps packages from the static binaries.
#
#   ./packaging/xbps/build-xbps.sh            # every architecture
#   ./packaging/xbps/build-xbps.sh amd64      # just one
#
# Runs xbps-create inside a Void container, because the tool exists nowhere
# else and there is no cross-build story to arrange: an .xbps is a tar of files
# plus a props.plist, and the architecture is a declared string rather than
# anything read out of the binary. No QEMU.
#
# Why Void is packaged at all now, when the Python line never was: an .xbps has
# to declare its dependencies, and the Python line's dependency was "an
# interpreter plus forty libraries at the versions Void happens to carry". A
# static binary declares none, which is the whole reason this is thirty lines of
# arch table rather than a fork of the dependency tree.
#
# TWO packages per architecture, glibc and musl. Void ships both libcs and
# treats them as separate architectures with separate repodata, and xbps
# refuses a package whose declared arch does not match the host's. The bytes
# are identical in both — the binary is static, so it genuinely does not care
# which libc the host has — but the label has to match or the package is
# invisible to half of Void.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"

BIN="packaging/out/bin"
OUT="packaging/out/xbps"
VERSION="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
# xbps revision, the `_1` in breeze-core-4.0.2_1. Bumped when the packaging
# changes but the program does not; reset to 1 on a new version.
REVISION="${BC_XBPS_REVISION:-1}"
IMAGE="ghcr.io/void-linux/void-glibc-full:latest"

# our label | Void architectures (glibc and musl share one binary)
#
# No s390x row: Void has never had an s390x port, so there is no arch string to
# put here. riscv64 and ppc64le are second-tier on Void itself and are built
# but untested on real hardware, exactly as they are for every other packager
# in this repository.
ARCHES="
amd64|x86_64 x86_64-musl
arm64|aarch64 aarch64-musl
armv7|armv7l armv7l-musl
riscv64|riscv64 riscv64-musl
ppc64le|ppc64le ppc64le-musl
"

MOUNT="$REPO"
case "$MOUNT" in /[a-z]/*) MOUNT="$(echo "$MOUNT" | sed -E 's#^/([a-z])/#\U\1:/#')" ;; esac
export MSYS_NO_PATHCONV=1

want=("$@")
selected() {
  [ ${#want[@]} -eq 0 ] && return 0
  for w in "${want[@]}"; do [ "$w" = "$1" ] && return 0; done
  return 1
}

rm -rf "$OUT"
mkdir -p "$OUT"
made=0

echo "=== breeze-core ${VERSION}_${REVISION} → .xbps"

while IFS='|' read -r label voidarches; do
  [ -n "$label" ] || continue
  selected "$label" || continue
  binary="$BIN/$label/breeze-core"
  if [ ! -f "$binary" ]; then
    echo "!! no binary for $label — run packaging/build-binaries.sh $label"
    continue
  fi
  # Refuse to wrap a stale binary, same guard as the nfpm builder: the
  # cross-built binaries are not rebuilt by `cargo build`, so editing the
  # source and running only this script would package the previous build under
  # the new version number.
  newer="$(find crates static Cargo.toml Cargo.lock -newer "$binary" -type f -print -quit 2>/dev/null || true)"
  if [ -n "$newer" ]; then
    echo "!! the $label binary is older than the source ($newer)"
    echo "   run: ./packaging/build-binaries.sh $label"
    exit 1
  fi

  echo
  echo "=== $label → $voidarches"

  # One container per label, both libc variants built inside it. Output leaves
  # as a tar on fd 3 rather than through a writable bind mount, for the reason
  # written out in packaging/nfpm/build-packages.sh: Docker Desktop here does
  # not reliably show the container a directory the host just created.
  docker run --rm \
    -v "$MOUNT:/work:ro" \
    -w /work \
    "$IMAGE" sh -eu -c "
      exec 3>&1 1>&2
      mkdir -p /tmp/out

      # --- stage the file tree exactly as it should land on the target ---
      D=/tmp/destdir
      rm -rf \$D
      install -Dm0755 '/work/$binary'                     \$D/usr/bin/breeze-core
      install -Dm0755 /work/deploy/init/runit-run          \$D/etc/sv/breeze-core/run
      install -Dm0755 /work/deploy/init/runit-log-run      \$D/etc/sv/breeze-core/log/run
      install -Dm0640 /work/packaging/nfpm/breeze-core.env \$D/etc/breeze-core/breeze-core.env
      install -Dm0644 /work/LICENSE                        \$D/usr/share/licenses/breeze-core/LICENSE
      install -Dm0644 /work/README.md                      \$D/usr/share/doc/breeze-core/README.md

      # Void's supervise-on-tmpfs convention: the supervise directory is state,
      # not configuration, so the service directory carries a symlink into
      # /run and runit creates the real thing at boot. A package that shipped a
      # real directory here would leave a stale one behind every reboot.
      ln -s /run/runit/supervise/breeze-core     \$D/etc/sv/breeze-core/supervise
      ln -s /run/runit/supervise/breeze-core-log \$D/etc/sv/breeze-core/log/supervise

      # INSTALL/REMOVE are read from the destdir root by xbps-create and are
      # recorded as scripts rather than installed as files.
      install -m0755 /work/packaging/xbps/INSTALL \$D/INSTALL
      install -m0755 /work/packaging/xbps/REMOVE  \$D/REMOVE

      # xbps-create writes the .xbps into the CURRENT DIRECTORY and has no flag
      # to say otherwise, so this has to be somewhere writable. /work is the
      # repository mounted read-only, and the failure it gives from there is
      # 'mkstemp: Read-only file system', which names neither the file nor the
      # directory it was trying to create.
      cd /tmp/out
      for a in $voidarches; do
        # No -D: a static binary has no dependencies, which is the entire
        # reason this package can exist. Do not add one 'just in case' —
        # xbps-install would then pull a library nothing links against.
        xbps-create \
          -A \"\$a\" \
          -n \"breeze-core-${VERSION}_${REVISION}\" \
          -s 'Self-hosted LAN-first REST API and web panel for Midea air conditioners' \
          -S 'Controls multiple Midea air conditioners over the LAN with no cloud dependency after pairing: a REST API, a web panel compiled into the binary, and a diagnostic CLI. One static executable with no runtime dependencies.' \
          -H 'https://github.com/monikapurpl3/breeze-core-native' \
          -l 'AGPL-3.0-or-later' \
          -m 'Monika <monika.purpl3@gmail.com>' \
          -F '/etc/breeze-core/breeze-core.env' \
          --compression zstd \
          \$D
        # No mv: xbps-create wrote it straight into /tmp/out.
      done

      tar -cf - -C /tmp/out . >&3
    " < /dev/null | tar -xf - -C "$OUT"
  made=$((made + 1))
done <<< "$ARCHES"

[ "$made" -gt 0 ] || { echo "nothing to package"; exit 1; }

echo
echo "=== packages ==="
ls -1 "$OUT" | while read -r f; do
  printf '  %-46s %7s KB\n' "$f" "$(( ($(wc -c < "$OUT/$f") + 1023) / 1024 ))"
done
echo
echo "next: ./packaging/xbps/verify-xbps.sh   (installs one in a Void container)"
