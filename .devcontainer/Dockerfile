FROM mcr.microsoft.com/devcontainers/rust:1-bookworm

ARG TARGETARCH
ARG VCONTROLD_VERSION=0.98.12
ARG VCONTROLD_DEB_REVISION=16

# Install vcontrold for local testing (architecture-aware)
RUN set -eux; \
    case "${TARGETARCH}" in \
    "amd64") DEB_ARCH="amd64" ;; \
    "arm64") DEB_ARCH="arm64" ;; \
    *) echo "Unsupported arch: ${TARGETARCH}"; exit 1 ;; \
    esac; \
    wget -q -O /tmp/vcontrold.deb \
    "https://github.com/openv/vcontrold/releases/download/v${VCONTROLD_VERSION}/vcontrold_${VCONTROLD_VERSION}-${VCONTROLD_DEB_REVISION}_${DEB_ARCH}.deb" \
    && apt-get update \
    && apt-get install -y --no-install-recommends libxml2 \
    && dpkg -i /tmp/vcontrold.deb \
    && rm /tmp/vcontrold.deb \
    && rm -rf /var/lib/apt/lists/*

# Install mosquitto broker and clients for MQTT testing
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        mosquitto \
        mosquitto-clients \
    && rm -rf /var/lib/apt/lists/*

# Configure mosquitto to allow anonymous connections for local testing
RUN printf 'listener 1883\nallow_anonymous true\n' > /etc/mosquitto/conf.d/local.conf
