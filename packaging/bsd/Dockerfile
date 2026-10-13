# syntax=docker/dockerfile:1
# The BSD packager: each BSD's own packaging tools, running on Linux, so the
# packages are made and signed on the workstation instead of on a VM per BSD.
#
#   FreeBSD   pkg 2.7.5 (what FreeBSD 15.1 ships), built from source the way
#             pkg's own CI builds it on Ubuntu 24.04: `pkg create` and
#             `pkg repo`, which also signs the catalogue.
#   NetBSD    pkgsrc's pkg_install (pkg_create, pkg_info -X for pkg_summary),
#             from a pkgsrc bootstrap.
#   OpenBSD   signify, for its gzip signature mode (-z); the package itself is
#             written by mkpkg-openbsd.py, since pkg_create is Perl that only
#             runs on OpenBSD.
#
# The binaries are not built here -- build-binaries.sh cross-compiles them --
# and the signing keys are streamed in per run, never baked into the image.
FROM ubuntu:24.04

ARG FREEBSD_PKG=2.7.5
ARG FREEBSD_PKG_SHA256=51d726f52ad750fcde14d5f041d43df5b12ea4a6f143777c6b02ed7df16ceb39
ARG PKGSRC=pkgsrc-2026Q3

RUN apt-get update -qq \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
      build-essential pkg-config m4 ca-certificates curl xz-utils zstd file python3 \
      libsqlite3-dev libbsd-dev libarchive-dev libssl-dev liblzma-dev liblua5.2-dev \
      liblzo2-dev libattr1-dev libacl1-dev zlib1g-dev libbz2-dev \
      signify-openbsd >/dev/null \
 && rm -rf /var/lib/apt/lists/*

RUN set -eu; mkdir -p /src && cd /src; \
    curl -fsSL -o pkg.tar.gz "https://github.com/freebsd/pkg/archive/refs/tags/${FREEBSD_PKG}.tar.gz"; \
    echo "${FREEBSD_PKG_SHA256}  pkg.tar.gz" | sha256sum -c -; \
    tar -xzf pkg.tar.gz; cd "pkg-${FREEBSD_PKG}"; \
    ./configure --prefix=/opt/freebsd-pkg >/dev/null; \
    make -j"$(nproc)" >/dev/null; make install >/dev/null; \
    cd / && rm -rf /src; \
    /opt/freebsd-pkg/sbin/pkg -v

# pkgsrc's quarterly tarball is re-rolled as the branch gets fixes, so it is
# checked against the SHA512 published beside it rather than a pinned hash.
RUN set -eu; mkdir -p /src && cd /src; \
    base="https://cdn.netbsd.org/pub/pkgsrc/${PKGSRC}"; \
    curl -fsSLO "$base/pkgsrc.tar.xz"; curl -fsSLO "$base/pkgsrc.tar.xz.SHA512"; \
    want="$(grep -oE '[0-9a-f]{128}' pkgsrc.tar.xz.SHA512)"; \
    echo "$want  pkgsrc.tar.xz" | sha512sum -c -; \
    tar -xJf pkgsrc.tar.xz; \
    cd pkgsrc/bootstrap; \
    ./bootstrap --prefix=/opt/pkgsrc --make-jobs="$(nproc)" >/tmp/bootstrap.log 2>&1 \
      || { tail -40 /tmp/bootstrap.log; exit 1; }; \
    cd / && rm -rf /src /tmp/bootstrap.log; \
    /opt/pkgsrc/sbin/pkg_info -V

ENV PATH=/opt/freebsd-pkg/sbin:/opt/freebsd-pkg/bin:/opt/pkgsrc/sbin:/opt/pkgsrc/bin:$PATH
WORKDIR /work
