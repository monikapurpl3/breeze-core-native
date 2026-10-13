#!/usr/bin/env bash
# Run every built Linux binary, under QEMU where it is foreign: serve, answer
# /api/health and the panel, pass `diag`, stop.
#
#   ./packaging/cross/smoke-linux.sh                  # every label in packaging/out/bin
#   ./packaging/cross/smoke-linux.sh ppc64 armv6
#
# This is what turns "cross-built" into "cross-built and run". Each binary is
# streamed into a stock Alpine (x86-64) container and executed there; the kernel
# hands a foreign ELF to the QEMU registered for it, so the same container image
# serves every architecture. Packages are verified separately, by
# packaging/repo/verify-repo.sh, in each distribution's own image; this checks
# the binaries themselves, including the ones no packaged image exists for.
#
# Before running: Docker Desktop's VM has QEMU for arm, aarch64, riscv64,
# ppc64le, s390x and mips64; ppc64 and loongarch64 come from
# install-qemu-binfmt.sh, and 32-bit MIPS from packaging/mips/prepare-qemu.sh.
# None of those registrations survive Docker restarting its VM.
#
# mips64 (Octeon) is skipped, and says so: the OpenWrt Octeon toolchain emits
# Octeon-only instructions, which QEMU's generic MIPS64 lacks, and QEMU's
# Octeon model has no FPU -- nor does a real Octeon, where the kernel emulates
# floating point. Only a real EdgeRouter can run it.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
BIN="packaging/out/bin"
export MSYS_NO_PATHCONV=1

want=("$@")
if [ ${#want[@]} -eq 0 ]; then
  for d in "$BIN"/*/; do
    l="$(basename "$d")"
    case "$l" in
      macos-*|windows|freebsd-*|netbsd-*|openbsd-*|termux-*) ;;  # not Linux ELF for Alpine
      *) want+=("$l") ;;
    esac
  done
fi

# The script run inside: a throwaway config, a spare port, and diag with HOME
# pointed into the throwaway directory so its CLI login lands there too.
read -r -d '' SMOKE <<'SH' || true
set -u
# wget where there is one (Alpine); bash's /dev/tcp where there is not (Debian).
get() {
  if command -v wget >/dev/null 2>&1; then wget -qO- "http://127.0.0.1:18431$1"
  else
    exec 3<>/dev/tcp/127.0.0.1/18431
    printf 'GET %s HTTP/1.0\r\nHost: localhost\r\n\r\n' "$1" >&3
    cat <&3; exec 3<&-
  fi
}
D="$(mktemp -d)"
printf '{"api_key":"smoke-test-not-a-secret","units":[]}\n' > "$D/config.json"
AC_CONFIG_DIR="$D" AC_CONFIG="$D/config.json" /t/breeze-core serve --host 127.0.0.1 --port 18431 > "$D/log" 2>&1 &
PID=$!
ok=0
for i in 1 2 3 4 5 6 7 8 9 10; do
  get /api/health 2>/dev/null | grep -q '"ok"' && { ok=1; break; }
  sleep 1
done
[ "$ok" = 1 ] || { echo "no /api/health"; tail -5 "$D/log"; exit 1; }
get / | grep -q '<title>Breeze Core</title>' || { echo "no panel"; exit 1; }
HOME="$D" XDG_CONFIG_HOME="$D" AC_CONFIG_DIR="$D" /t/breeze-core diag \
  --base-url http://127.0.0.1:18431 --config "$D/config.json" > "$D/diag" 2>&1
grep -q "0 failure(s)" "$D/diag" || { echo "diag failed:"; tail -8 "$D/diag"; exit 1; }
kill "$PID"
echo "$(/t/breeze-core --version | head -1): served, answered, diag clean"
SH

pass=0; fail=0; failed=()
for l in "${want[@]}"; do
  b="$BIN/$l/breeze-core"
  if [ "$l" = mips64 ]; then
    printf '  %-12s skipped: Octeon cannot run under QEMU (see the header)\n' "$l"
    continue
  fi
  [ -f "$b" ] || { printf '  %-12s no %s\n' "$l" "$b"; fail=$((fail+1)); failed+=("$l"); continue; }
  arch="$(file -b "$b" | cut -d, -f2 | sed 's/^ //')"
  # Static binaries run in one x86-64 Alpine, whatever their architecture. A
  # dynamically linked one (s390x, the one glibc build) needs its own loader
  # and libc, so it runs in a Debian image of its own architecture instead.
  # amd64 named explicitly: the image store keeps one variant per tag, so a
  # bare tag can be whichever architecture was pulled last.
  image=alpine:3.24 platform="--platform linux/amd64" shell=sh
  if file -b "$b" | grep -q 'dynamically linked'; then
    case "$l" in
      s390x) platform="--platform linux/s390x" ;;
      *) printf '  %-12s FAIL  dynamically linked, and no image known for it\n' "$l"
         fail=$((fail+1)); failed+=("$l"); continue ;;
    esac
    image=debian:13 shell=bash
  fi
  stage="$(mktemp -d)"
  cp "$b" "$stage/breeze-core"
  printf '%s\n' "$SMOKE" > "$stage/smoke.sh"
  # shellcheck disable=SC2086
  if out="$(tar -C "$stage" -cf - breeze-core smoke.sh \
        | timeout 300 docker run --rm -i $platform "$image" \
            sh -c "mkdir /t && tar -xf - -C /t && chmod +x /t/breeze-core && $shell /t/smoke.sh" 2>&1)"; then
    printf '  %-12s ok    %-28s %s\n' "$l" "$arch" "$(printf '%s' "$out" | tail -1)"
    pass=$((pass+1))
  else
    printf '  %-12s FAIL  %s\n' "$l" "$arch"
    printf '%s\n' "$out" | tail -6 | sed 's/^/                /'
    fail=$((fail+1)); failed+=("$l")
  fi
  rm -rf "$stage"
done

echo
echo "$pass ran, $fail failed${failed:+: ${failed[*]}}"
[ "$fail" = 0 ]
