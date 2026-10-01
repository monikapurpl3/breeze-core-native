#!/usr/bin/env bash
# Rebuild each source package the way a user would, in a clean container with
# that distribution's own Rust, then install the result and run it.
#
#   ./packaging/source/verify-source.sh                 # every case below
#   ./packaging/source/verify-source.sh alma ubuntu
#
# Each case also compares the rebuilt package's files and symlinks with the
# binary package from packaging/out/pkg/. "The same package as the
# repository's" is the promise every recipe makes; this is what keeps it true.
# Directories are left out of the comparison on purpose: the rpm recipe owns
# /usr/lib/breeze-core and its doc directory, which the nfpm package does not,
# and owning them is the better behaviour (no empty directories left behind).
#
# These compile the whole workspace and run its tests (%check, dh_auto_test,
# check()), so each case takes minutes, not seconds.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
VER="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
OUT="packaging/out/source"
PKG="packaging/out/pkg"
[ -d "$OUT/rpm" ] || { echo "no source packages -- run packaging/source/build-source.sh"; exit 1; }

MOUNT="$REPO"
case "$MOUNT" in /[a-z]/*) MOUNT="$(echo "$MOUNT" | sed -E 's#^/([a-z])/#\U\1:/#')" ;; esac
export MSYS_NO_PATHCONV=1

want=("$@")
selected() {
  [ ${#want[@]} -eq 0 ] && return 0
  for w in "${want[@]}"; do [ "$w" = "$1" ] && return 0; done
  return 1
}
pass=0; fail=0
run_case() {
  local name="$1" image="$2" script="$3"
  selected "$name" || return 0
  echo
  echo "=== $name ($image)"
  if printf 'VER=%s\n%s' "$VER" "$script" \
     | timeout 3600 docker run --rm -i -v "$MOUNT/$OUT:/src:ro" -v "$MOUNT/$PKG:/pkg:ro" \
         "$image" bash -eu -s 2>&1 | sed 's/^/    /'; then
    printf '  \033[32mPASS\033[0m  %s\n' "$name"; pass=$((pass+1))
  else
    printf '  \033[31mFAIL\033[0m  %s\n' "$name"; fail=$((fail+1))
  fi
}

# Shared by every case: the installed program answers, and the rebuilt package
# holds the same paths as the nfpm one. Both lists are sorted path lists.
COMMON='
same_files() {   # $1 rebuilt list, $2 nfpm list
  if diff -u "$2" "$1" > /tmp/files.diff; then
    echo "   same $(wc -l < "$1") paths as the binary package"
  else
    echo "   !! the file lists differ (- binary package, + rebuilt):"; sed "s/^/      /" /tmp/files.diff; exit 1
  fi
}
runs() {
  breeze-core --version | grep -q "breeze-core $VER"
  echo "   installed $(breeze-core --version | head -1), $(breeze-core --version | sed -n 2p)"
  # Linked against the distribution libc, which is the point of building here.
  if command -v ldd >/dev/null; then ldd "$(command -v breeze-core)" | grep -q "libc.so" \
    && echo "   dynamically linked against this system'"'"'s libc"; fi
}
'

# --- rpmbuild ----------------------------------------------------------------
RPM_CASE="$COMMON"'
  dnf -q -y install rpm-build dnf-plugins-core >/dev/null 2>&1 || dnf -q -y install rpm-build "dnf-command(builddep)" >/dev/null
  srpm=$(ls /src/rpm/breeze-core-$VER-*.src.rpm)
  echo "-- build dependencies, from the SRPM"
  dnf -q -y builddep "$srpm" >/dev/null
  echo "   rust $(rustc --version | cut -d" " -f2)"
  echo "-- rpmbuild --rebuild"
  rpmbuild --rebuild --define "_topdir /rb" "$srpm" > /tmp/build.log 2>&1 || { tail -40 /tmp/build.log; exit 1; }
  grep -E "^test result" /tmp/build.log | awk "{p+=\$4; f+=\$6} END {print \"   %check: \" p \" tests passed, \" f \" failed\"}"
  rpm=$(ls /rb/RPMS/*/breeze-core-$VER-*.rpm)
  list() { rpm -qp --qf "[%{FILEMODES:perms} %{FILENAMES}\n]" "$1" | grep -v "^d" | cut -d" " -f2 | sort; }
  list "$rpm" > /tmp/rebuilt
  list /pkg/breeze-core-$VER-*.x86_64.rpm > /tmp/binary
  same_files /tmp/rebuilt /tmp/binary
  dnf -q -y install "$rpm" >/dev/null
  runs
'
run_case alma   almalinux:9   "$RPM_CASE"
run_case fedora fedora:44     "$RPM_CASE"

# --- dpkg-buildpackage -------------------------------------------------------
DEB_CASE="$COMMON"'
  export DEBIAN_FRONTEND=noninteractive
  apt-get -qq update >/dev/null && apt-get -qq install -y dpkg-dev >/dev/null
  mkdir /b && cd /b
  dpkg-source -x /src/deb/breeze-core_$VER-*.dsc >/dev/null
  cd breeze-core-$VER
  echo "-- build dependencies, from debian/control"
  apt-get -qq build-dep -y . >/dev/null
  echo "   $( (command -v rustc-1.91 || command -v rustc-1.89 || command -v rustc) | xargs basename) $( (rustc-1.91 --version 2>/dev/null || rustc-1.89 --version 2>/dev/null || rustc --version) | cut -d" " -f2)"
  echo "-- dpkg-buildpackage -b"
  dpkg-buildpackage -b -us -uc > /tmp/build.log 2>&1 || { tail -40 /tmp/build.log; exit 1; }
  grep -E "^test result" /tmp/build.log | awk "{p+=\$4; f+=\$6} END {print \"   dh_auto_test: \" p \" tests passed, \" f \" failed\"}"
  deb=$(ls /b/breeze-core_$VER-*_amd64.deb)
  # Files and symlinks: dpkg-deb -c prints a mode column, then the path sixth.
  # debhelper adds copyright and changelog.Debian.gz to every package it
  # builds; those two are the expected difference.
  list() { dpkg-deb -c "$1" | grep -v "^d" | awk "{print \$6}" | sed "s#^\./#/#" \
             | grep -v "^/usr/share/doc/breeze-core/\(changelog\.Debian\.gz\|copyright\)\$" | sort; }
  list "$deb" > /tmp/rebuilt
  list /pkg/breeze-core_$VER-*_amd64.deb > /tmp/binary
  same_files /tmp/rebuilt /tmp/binary
  apt-get -qq install -y "$deb" >/dev/null
  runs
'
# 26.04: Rust 1.93 from its own archive. 24.04: 1.75 by default, which is too
# old; the build dependency alternatives pick the versioned rustc-1.91.
run_case ubuntu     ubuntu:26.04 "$DEB_CASE"
run_case ubuntu2404 ubuntu:24.04 "$DEB_CASE"

# Debian 13's own Rust is 1.85, three releases short, and it packages no newer
# one. The documented way round is rustup and `dpkg-buildpackage -d` (do not
# look for Debian's rustc); this checks that the recipe really works that way.
run_case debian13-rustup debian:13 "$COMMON"'
  export DEBIAN_FRONTEND=noninteractive
  apt-get -qq update >/dev/null && apt-get -qq install -y dpkg-dev debhelper gcc curl ca-certificates >/dev/null
  curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null 2>&1
  export PATH="$HOME/.cargo/bin:$PATH"
  echo "   rustup rust $(rustc --version | cut -d" " -f2) (Debian has $(apt-cache policy rustc | sed -n "s/ *Candidate: //p"))"
  mkdir /b && cd /b
  dpkg-source -x /src/deb/breeze-core_$VER-*.dsc >/dev/null
  cd breeze-core-$VER
  echo "-- dpkg-buildpackage -b -d"
  dpkg-buildpackage -b -d -us -uc > /tmp/build.log 2>&1 || { tail -40 /tmp/build.log; exit 1; }
  grep -E "^test result" /tmp/build.log | awk "{p+=\$4; f+=\$6} END {print \"   dh_auto_test: \" p \" tests passed, \" f \" failed\"}"
  apt-get -qq install -y /b/breeze-core_$VER-*_amd64.deb >/dev/null
  runs
'

# --- makepkg -----------------------------------------------------------------
run_case arch archlinux:base "$COMMON"'
  pacman -Syu --noconfirm --needed base-devel cargo >/dev/null 2>&1
  echo "   rust $(rustc --version | cut -d" " -f2)"
  useradd -m builder && mkdir /b && chown builder /b
  su builder -c "cd /b && tar -xf /src/arch/breeze-core-$VER-*.src.tar.gz"
  echo "-- makepkg, from the source tarball alone"
  su builder -c "cd /b/breeze-core && makepkg --noconfirm" > /tmp/build.log 2>&1 || { tail -40 /tmp/build.log; exit 1; }
  grep -E "^test result" /tmp/build.log | awk "{p+=\$4; f+=\$6} END {print \"   check(): \" p \" tests passed, \" f \" failed\"}"
  pkg=$(ls /b/breeze-core/breeze-core-$VER-*-x86_64.pkg.tar.zst)
  # Files and symlinks, without the .PKGINFO, .MTREE and friends pacman adds.
  list() { tar --zstd -tvf "$1" | grep -v "^d" | awk "{print \$6}" | grep -v "^\." | sed "s#^#/#" | sort; }
  list "$pkg" > /tmp/rebuilt
  list /pkg/breeze-core-$VER-*-x86_64.pkg.tar.zst > /tmp/binary
  same_files /tmp/rebuilt /tmp/binary
  pacman -U --noconfirm "$pkg" >/dev/null
  runs
'

echo
echo "=== $pass passed, $fail failed ==="
[ "$fail" -eq 0 ]
