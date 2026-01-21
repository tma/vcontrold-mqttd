# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] - 2026-01-21

### Added

- Native Rust implementation of vcontrold MQTT bridge
- Persistent TCP connection to vcontrold with automatic reconnection
- MQTT v5 protocol support via rumqttc
- TLS encryption with rustls (CA certs, client certs, insecure mode)
- Periodic polling of heating controller parameters
- Request/response bridge for ad-hoc commands via MQTT
- Command batching to respect protocol limits
- Multi-architecture Docker images (amd64, arm64)
- Cross-compilation for fast CI builds
- Container image signing with cosign (Sigstore)
- Dependabot for automated dependency updates

### Technical Details

- vcontrold daemon lifecycle management (spawn, monitor, graceful shutdown)
- Proper signal handling (SIGTERM, SIGINT) for Docker container shutdown
- JSON response formatting compatible with vclient `-j` output
- Environment variable based configuration
