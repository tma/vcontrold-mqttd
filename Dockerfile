# syntax=docker/dockerfile:1.4

# Stage 1: Download vcontrold for target architecture
FROM debian:bookworm-slim AS vcontrold-downloader

ARG TARGETARCH
ARG TARGETVARIANT
ARG VCONTROLD_VERSION=0.98.12
ARG VCONTROLD_DEB_REVISION=16

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        wget \
    && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    case "${TARGETARCH}${TARGETVARIANT:+-${TARGETVARIANT}}" in \
    "amd64") DEB_ARCH="amd64" ;; \
    "arm64") DEB_ARCH="arm64" ;; \
    "arm-v7") DEB_ARCH="armhf" ;; \
    *) echo "Unsupported arch: ${TARGETARCH}${TARGETVARIANT:+-${TARGETVARIANT}}"; exit 1 ;; \
    esac; \
    wget -O /vcontrold.deb \
    "https://github.com/openv/vcontrold/releases/download/v${VCONTROLD_VERSION}/vcontrold_${VCONTROLD_VERSION}-${VCONTROLD_DEB_REVISION}_${DEB_ARCH}.deb"

# Stage 2: Final image
FROM debian:bookworm-slim

# Copy vcontrold from downloader stage
COPY --from=vcontrold-downloader /vcontrold.deb /tmp/vcontrold.deb

# Install runtime dependencies and vcontrold
RUN apt-get update \
    && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        libxml2 \
    && dpkg -i /tmp/vcontrold.deb \
    && rm /tmp/vcontrold.deb \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

# Create non-root user
RUN groupadd -r vcontrold \
    && useradd --no-log-init -r -g vcontrold -G dialout vcontrold \
    && mkdir -p /config \
    && chown -R vcontrold:vcontrold /config

# Copy the pre-built Rust binary from build context
COPY vcontrold-mqttd /usr/local/bin/vcontrold-mqttd

USER vcontrold

VOLUME ["/config"]

ENTRYPOINT ["/usr/local/bin/vcontrold-mqttd"]
