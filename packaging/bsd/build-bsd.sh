#!/usr/bin/env bash
# Package and sign the three BSD builds -- on this workstation.
#
#   ./packaging/bsd/build-bsd.sh                     # freebsd netbsd openbsd
#   ./packaging/bsd/build-bsd.sh openbsd
#
# Input:  packaging/out/bin/{freebsd,netbsd,openbsd}-amd64/breeze-core, which
#         packaging/build-binaries.sh cross-compiles.
# Output: packaging/out/bsd/, laid out as build-repo.sh carries it to aspic:
#           freebsd/                       the package, its signed pkg catalogue
#           netbsd/All/                    the package and pkg_summary(.gz)
#           openbsd/<rel>/packages/amd64/  the signify-signed package
#           openbsd/aspic-pkg.pub          ...and the key it names
#
# Until 4.3.2 each BSD was compiled, packaged and signed on a VM of that BSD,
# and two signing keys travelled to those VMs to be used and shredded. Now the
# binaries are cross-compiled and the packaging tools come to the keys instead:
# the bc-bsdpkg image (the Dockerfile here) holds FreeBSD's pkg, pkgsrc's
# pkg_install and signify, all on Linux. The keys are streamed into a
# throwaway container per run and never written into an image or onto a VM.
# The VMs only run the verify-*.sh scripts.
#
# Everything is streamed rather than bind-mounted, as in every other packaging
# step here: Docker Desktop on this machine does not reliably see files the
# host wrote moments earlier through a writable mount.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
bash packaging/check-eol.sh
VER="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
# One place says which OpenBSD release the binary was linked against.
OBSD_REL="$(sed -n 's/^OPENBSD_REL="\(.*\)"$/\1/p' packaging/build-binaries.sh)"
[ -n "$OBSD_REL" ] || { echo "!! could not read OPENBSD_REL from build-binaries.sh"; exit 1; }
KEYS="packaging/repo/keys"
OUT="packaging/out/bsd"
export MSYS_NO_PATHCONV=1

want=("$@")
[ ${#want[@]} -gt 0 ] || want=(freebsd netbsd openbsd)
for os in "${want[@]}"; do
  case "$os" in freebsd|netbsd|openbsd) ;; *) echo "unknown: $os"; exit 2 ;; esac
  bin="packaging/out/bin/$os-amd64/breeze-core"
  [ -f "$bin" ] || { echo "!! no $bin -- run: ./packaging/build-binaries.sh $os-amd64"; exit 1; }
  # Refuse to wrap a stale binary (see nfpm/build-packages.sh for the story).
  newer="$(find crates static Cargo.toml Cargo.lock -newer "$bin" -type f -print -quit 2>/dev/null || true)"
  [ -z "$newer" ] || { echo "!! $bin is older than the source ($newer)"; exit 1; }
done
has() { local w; for w in "${want[@]}"; do [ "$w" = "$1" ] && return 0; done; return 1; }

keys=()
has freebsd && keys+=(aspic-freebsd.rsa)
has openbsd && keys+=(aspic-openbsd.sec aspic-openbsd.pub)
for k in "${keys[@]}"; do
  [ -f "$KEYS/$k" ] || { echo "!! no $KEYS/$k (packaging/repo/README.md says where keys come from)"; exit 1; }
done

docker build -q -t bc-bsdpkg packaging/bsd >/dev/null
echo "breeze-core $VER for ${want[*]}; packager image ready"

# The packing lists record file times; the commit's is reproducible.
SOURCE_DATE_EPOCH="$(git log -1 --format=%ct)"

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
# Two archives back to back, the second only for the keys, so no key is ever
# copied to a file outside packaging/repo/keys; tar -i reads straight through.
{
  tar -cf - LICENSE crates/breeze-core/Cargo.toml packaging/bsd packaging/nfpm/breeze-core.env \
    $(for os in "${want[@]}"; do printf ' packaging/out/bin/%s-amd64/breeze-core' "$os"; done)
  [ ${#keys[@]} -eq 0 ] || tar -cf - -C "$KEYS" "${keys[@]}"
} | docker run --rm -i -e WANT="${want[*]}" -e VER="$VER" -e OBSD_REL="$OBSD_REL" \
      -e SOURCE_DATE_EPOCH="$SOURCE_DATE_EPOCH" bc-bsdpkg sh -euc '
  exec 3>&1 1>&2
  mkdir -p /work/src /tmp/out && cd /work/src && tar -xif -
  chmod 600 aspic-* 2>/dev/null || true
  for os in $WANT; do
    bin="packaging/out/bin/$os-amd64/breeze-core"
    echo "=== $os"
    case "$os" in
      freebsd)
        BC_BIN="$bin" sh packaging/bsd/mkpkg-freebsd.sh /tmp/fbsd | tail -1
        # pkg repo writes the catalogue (packagesite, data, meta) beside the
        # package and signs it with the RSA key.
        pkg repo /tmp/fbsd aspic-freebsd.rsa | tail -2
        mkdir -p /tmp/out/freebsd && cp /tmp/fbsd/* /tmp/out/freebsd/
        ;;
      netbsd)
        PATH="$PATH:/opt/pkgsrc/sbin" BC_BIN="$bin" sh packaging/bsd/mkpkg-netbsd.sh /tmp/nbsd | tail -1
        mkdir -p /tmp/out/netbsd/All && cp /tmp/nbsd/breeze-core-$VER.tgz /tmp/out/netbsd/All/
        ( cd /tmp/out/netbsd/All && pkg_info -X "breeze-core-$VER.tgz" > pkg_summary && gzip -9kf pkg_summary )
        ;;
      openbsd)
        dir="/tmp/out/openbsd/$OBSD_REL/packages/amd64"
        mkdir -p "$dir"
        python3 packaging/bsd/mkpkg-openbsd.py --repo . --bin "$bin" --version "$VER" \
          --out /tmp/unsigned.tgz
        # The key is copied under the name the signature must carry: pkg_add
        # opens /etc/signify/<that name>.pub.
        cp aspic-openbsd.sec /tmp/aspic-pkg.sec && chmod 600 /tmp/aspic-pkg.sec
        signify-openbsd -zS -s /tmp/aspic-pkg.sec -m /tmp/unsigned.tgz -x "$dir/breeze-core-$VER.tgz"
        signify-openbsd -zV -p aspic-openbsd.pub -x "$dir/breeze-core-$VER.tgz" -m /dev/null >/dev/null
        echo "  signed and verified: breeze-core-$VER.tgz"
        ;;
    esac
  done
  tar -cf - -C /tmp/out . >&3
' > "$stage/out.tar"

for os in "${want[@]}"; do rm -rf "${OUT:?}/$os"; done
mkdir -p "$OUT"
tar -xf "$stage/out.tar" -C "$OUT"

# The public halves travel with what they verify.
if has freebsd; then
  cp "$KEYS/aspic-freebsd.pub" "$OUT/freebsd/aspic-freebsd.pub"
fi
if has openbsd; then
  cp "$KEYS/aspic-openbsd.pub" "$OUT/openbsd/aspic-pkg.pub"
fi
chmod -R a+rX "$OUT"

echo
find "$OUT" -type f | sort | sed 's|^|  |'
echo
echo "verify on the VMs with packaging/bsd/verify-{freebsd,netbsd,openbsd}.sh"
