# syntax=docker/dockerfile:1
# The MIPS builder: a pinned nightly with rust-src, and OpenWrt's own
# toolchains for the two architectures most routers are.
#
#   mipsel_24kc   little-endian, ramips (MediaTek MT7621, MT76x8)
#   mips_24kc     big-endian, ath79 (Qualcomm Atheros)
#
# Why each piece:
#
#   * nightly + rust-src: every MIPS target is tier 3, so there is no prebuilt
#     std and it is compiled from source with -Z build-std. Pinned to a date,
#     so a new nightly cannot change a release build underneath it.
#   * OpenWrt's toolchain, not Zig: Zig 0.16's bundled mipsel musl emits
#     references to its own std internals and then fails to provide them. The
#     OpenWrt toolchain is the one the firmware itself is built with.
#
# The toolchain tarballs are checked against OpenWrt's published SHA-256s.
FROM debian:bookworm-slim

ARG NIGHTLY=nightly-2026-09-30
ARG OPENWRT=25.12.5
ARG GCC=gcc-14.3.0

RUN apt-get update -qq \
 && apt-get install -y -qq --no-install-recommends curl ca-certificates zstd xz-utils gcc libc6-dev file >/dev/null \
 && rm -rf /var/lib/apt/lists/*

# label | OpenWrt target | toolchain SHA-256 (from that target's sha256sums)
RUN set -eu; mkdir -p /opt/openwrt; cd /opt/openwrt; \
    for t in "ramips-mt7621|ramips/mt7621|04316e081dcaea0394d3a8eca761500c91d7d54e9d49ccb8f855ae257ea4f995" \
             "ath79-generic|ath79/generic|3903d0069c4833f45cd1fc5dfa7714e04a216476c32ec90b660477aa98826b6e"; do \
      name="${t%%|*}"; rest="${t#*|}"; path="${rest%%|*}"; sum="${rest#*|}"; \
      f="openwrt-toolchain-${OPENWRT}-${name}_${GCC}_musl.Linux-x86_64.tar.zst"; \
      curl -fsSLO "https://downloads.openwrt.org/releases/${OPENWRT}/targets/${path}/${f}"; \
      echo "${sum}  ${f}" | sha256sum -c -; \
      tar --zstd -xf "$f" && rm "$f"; \
    done; \
    ln -s /opt/openwrt/openwrt-toolchain-*-ramips-mt7621_*/toolchain-mipsel_24kc_* /opt/mipsel_24kc; \
    ln -s /opt/openwrt/openwrt-toolchain-*-ath79-generic_*/toolchain-mips_24kc_* /opt/mips_24kc; \
    /opt/mipsel_24kc/bin/mipsel-openwrt-linux-musl-gcc --version | head -1; \
    /opt/mips_24kc/bin/mips-openwrt-linux-musl-gcc --version | head -1

# A static musl build links `-lunwind`, from a libunwind.a that ships with a
# target's prebuilt std, and tier 3 targets have none. gcc's own static
# unwinder is the same _Unwind_* API -- it is what these targets use, as
# libgcc_s, when linked dynamically -- so each toolchain gets a directory in
# which libunwind.a IS its libgcc_eh.a.
RUN set -eu; for t in mipsel_24kc mips_24kc; do \
      mkdir -p "/opt/unwind/$t"; \
      ln -s "$(ls /opt/$t/lib/gcc/*/*/libgcc_eh.a)" "/opt/unwind/$t/libunwind.a"; \
    done

ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal \
      --default-toolchain "$NIGHTLY" -c rust-src >/dev/null \
 && rustc --version
# The OpenWrt gcc warns on every call without this; it names the directory the
# toolchain was unpacked into, which the build never writes to.
ENV STAGING_DIR=/opt/openwrt
WORKDIR /work
