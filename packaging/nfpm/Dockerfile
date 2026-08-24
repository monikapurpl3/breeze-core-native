# syntax=docker/dockerfile:1
# Packaging container: nfpm plus the few tools build-packages.sh uses to inspect
# what it produced. Alpine because the image is small and nothing here needs
# glibc — the binary being packaged does not run in this container, it is only
# copied into an archive.
FROM alpine:3.20

# Pinned. nfpm's apk and ipk packagers are young enough that a floating tag has
# changed its output between patch releases, and a repository index is built
# from these filenames.
ARG NFPM_VERSION=2.47.0
RUN apk add --no-cache bash tar zstd binutils curl rpm \
    && arch="$(apk --print-arch)" \
    && case "$arch" in x86_64) na=x86_64 ;; aarch64) na=arm64 ;; *) na="$arch" ;; esac \
    && curl -fsSL "https://github.com/goreleaser/nfpm/releases/download/v${NFPM_VERSION}/nfpm_${NFPM_VERSION}_Linux_${na}.tar.gz" \
       | tar -xz -C /usr/local/bin nfpm \
    && nfpm --version
WORKDIR /work
