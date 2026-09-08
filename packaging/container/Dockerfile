# Breeze Core — distroless.
#
#   docker buildx build -f packaging/container/Dockerfile \
#     --platform linux/amd64,linux/arm64 \
#     -t ghcr.io/monikapurpl3/breeze-core:4.0.1 packaging/out
#
# The context is `packaging/out`, so the image ships the **same binary the
# packages ship** rather than one compiled here. A container that is a different
# build from the .deb is a container that can fail differently, and this project
# has already spent a day on a bug that only appeared in one environment.
#
# The Python line needed five images — Alpine, Alpine+nginx, UBI v2, UBI v3 —
# because an interpreter needs a distribution around it: the base decided which
# wheels installed and which CPUs the extensions ran on. A static musl binary
# has no such opinion, so the base is no longer a compatibility decision and
# `scratch` is the honest answer.
#
# What is in here, and why each is unavoidable:
#
#   * the binary. Static musl, no libc at runtime, panel compiled in.
#   * **tzdata**. Not optional: everything scheduled here uses the server's
#     local clock, and without zoneinfo a container with TZ=Europe/Zagreb set
#     reports `utc_offset_seconds: 0` and fires every schedule two hours off,
#     silently. Measured, not assumed.
#   * an empty, writable /etc/breeze-core. `scratch` has no mkdir, so the
#     directory has to arrive as a COPY.
#
# What is deliberately NOT in here:
#
#   * ca-certificates. The TLS roots are compiled into the binary
#     (webpki-roots, and zero rustls-native-certs), so cloud pairing works with
#     no trust store on disk.
#   * /etc/passwd. It runs as a numeric uid, which needs no passwd entry.
#   * a shell. Use the `:debug` tag for that.

# --- tzdata, and a place to prepare the things scratch cannot create ---------
FROM alpine:3.20 AS prep
RUN apk add --no-cache tzdata
# 1.5 MB, and the whole set rather than a trimmed one: guessing which zones
# somebody needs is how you ship a container that is correct for you and two
# hours wrong for them.
ARG TARGETARCH
COPY bin/${TARGETARCH}/breeze-core /staged/breeze-core
# chmod here rather than trusting the context: the build context is produced on
# Windows, which records no POSIX execute bit, and a non-executable entrypoint
# fails with a message about the *file format* rather than the permission.
RUN chmod 0755 /staged/breeze-core \
 && mkdir -p /staged/etc/breeze-core \
 && chown -R 1001:1001 /staged/etc/breeze-core \
 && chmod 0750 /staged/etc/breeze-core

# --- the image --------------------------------------------------------------
FROM scratch

COPY --from=prep /usr/share/zoneinfo /usr/share/zoneinfo
COPY --from=prep /staged/breeze-core /usr/bin/breeze-core
COPY --from=prep --chown=1001:1001 /staged/etc/breeze-core /etc/breeze-core

# Numeric, because there is no /etc/passwd to name. 1001 is the uid the Python
# images used, so a volume written by one is writable by the other.
USER 1001:1001

# The four stores live here. Declared so `docker run` without a -v still keeps
# state for the life of the container rather than silently losing a pairing.
VOLUME ["/etc/breeze-core"]

EXPOSE 8420

# Loopback is wrong for a container -- nothing outside the namespace could reach
# it -- so the image default differs from the package default here, and only
# here. Override with BREEZE_HOST if the container has its own address.
ENV BREEZE_HOST=0.0.0.0 \
    BREEZE_PORT=8420 \
    AC_CONFIG_DIR=/etc/breeze-core

# The binary is the entrypoint, so `docker run <image> pair` runs the pairing
# tool. That replaces the Python images' `docker exec breeze-setup`, which
# needed a shell to wrap it.
ENTRYPOINT ["/usr/bin/breeze-core"]
CMD ["serve"]

LABEL org.opencontainers.image.title="Breeze Core" \
      org.opencontainers.image.description="LAN-first REST API and web panel for Midea air conditioners" \
      org.opencontainers.image.source="https://github.com/monikapurpl3/breeze-core-native" \
      org.opencontainers.image.licenses="AGPL-3.0-or-later"
