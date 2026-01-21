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

# Stage 2: Build Rust binary with cross-compilation
FROM rust:bookworm AS rust-builder

ARG TARGETARCH
ARG TARGETVARIANT

WORKDIR /build

# Install cross-compilation toolchains
RUN set -eux; \
    case "${TARGETARCH}${TARGETVARIANT:+-${TARGETVARIANT}}" in \
    "amd64") \
        RUST_TARGET="x86_64-unknown-linux-gnu" \
        ;; \
    "arm64") \
        RUST_TARGET="aarch64-unknown-linux-gnu" \
        && apt-get update \
        && apt-get install -y --no-install-recommends \
            gcc-aarch64-linux-gnu \
            libc6-dev-arm64-cross \
        && rm -rf /var/lib/apt/lists/* \
        ;; \
    "arm-v7") \
        RUST_TARGET="armv7-unknown-linux-gnueabihf" \
        && apt-get update \
        && apt-get install -y --no-install-recommends \
            gcc-arm-linux-gnueabihf \
            libc6-dev-armhf-cross \
        && rm -rf /var/lib/apt/lists/* \
        ;; \
    *) echo "Unsupported arch: ${TARGETARCH}${TARGETVARIANT:+-${TARGETVARIANT}}"; exit 1 ;; \
    esac; \
    rustup target add ${RUST_TARGET}; \
    echo "[target.aarch64-unknown-linux-gnu]" >> /usr/local/cargo/config.toml; \
    echo 'linker = "aarch64-linux-gnu-gcc"' >> /usr/local/cargo/config.toml; \
    echo "[target.armv7-unknown-linux-gnueabihf]" >> /usr/local/cargo/config.toml; \
    echo 'linker = "arm-linux-gnueabihf-gcc"' >> /usr/local/cargo/config.toml

# Copy source code
COPY Cargo.toml Cargo.lock* ./
COPY src ./src

# Build for target architecture
RUN set -eux; \
    case "${TARGETARCH}${TARGETVARIANT:+-${TARGETVARIANT}}" in \
    "amd64") RUST_TARGET="x86_64-unknown-linux-gnu" ;; \
    "arm64") RUST_TARGET="aarch64-unknown-linux-gnu" ;; \
    "arm-v7") RUST_TARGET="armv7-unknown-linux-gnueabihf" ;; \
    esac; \
    cargo build --release --target ${RUST_TARGET}; \
    cp /build/target/${RUST_TARGET}/release/vcontrold-mqttd /build/vcontrold-mqttd

# Stage 3: Final image
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

# Copy the Rust binary
COPY --from=rust-builder /build/vcontrold-mqttd /usr/local/bin/vcontrold-mqttd

USER vcontrold

VOLUME ["/config"]

ENTRYPOINT ["/usr/local/bin/vcontrold-mqttd"]
