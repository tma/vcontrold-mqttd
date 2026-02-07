# Agent Instructions for vcontrold-mqttd

## Project Overview

This is a Rust-based MQTT bridge for vcontrold (Viessmann heating controller interface). It runs as a Docker container that:

1. Manages the vcontrold daemon lifecycle
2. Maintains a persistent TCP connection to vcontrold (port 3002)
3. Polls heating controller parameters periodically
4. Publishes values to MQTT topics
5. Provides a request/response bridge via MQTT

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│ Docker Container                                        │
│                                                         │
│  ┌─────────────────┐    ┌─────────────────────────────┐│
│  │   vcontrold     │    │    vcontrold-mqttd (Rust)   ││
│  │   (daemon)      │◄──►│                             ││
│  │   Port 3002     │TCP │  ┌─────────┐  ┌──────────┐  ││
│  └─────────────────┘    │  │ Polling │  │Subscriber│  ││
│          │              │  │  Loop   │  │  Task    │  ││
│          ▼              │  └────┬────┘  └────┬─────┘  ││
│  ┌─────────────────┐    │       │            │        ││
│  │ /dev/vitocal    │    │       ▼            ▼        ││
│  │ (USB serial)    │    │  ┌─────────────────────┐    ││
│  └─────────────────┘    │  │    MQTT Client      │    ││
│                         │  │    (rumqttc v5)     │    ││
│                         │  └──────────┬──────────┘    ││
│                         └─────────────┼───────────────┘│
└───────────────────────────────────────┼────────────────┘
                                        │
                                        ▼
                               ┌─────────────────┐
                               │  MQTT Broker    │
                               └─────────────────┘
```

## Code Structure

```
src/
├── main.rs           # Entry point, tokio runtime, task orchestration
├── config.rs         # Environment variable parsing
├── error.rs          # Error types (thiserror)
├── polling.rs        # Command batching, periodic execution
├── process.rs        # Spawn/monitor vcontrold daemon
├── vcontrold/
│   ├── mod.rs
│   ├── client.rs     # Persistent TCP connection with reconnect
│   └── protocol.rs   # Protocol constants, response parsing
└── mqtt/
    ├── mod.rs
    ├── client.rs     # MQTT v5 client with TLS support
    ├── publisher.rs  # Publish polling results to topics
    └── subscriber.rs # Request/response bridge
```

## Key Design Decisions

### Native TCP vs vclient

We use direct TCP communication to vcontrold instead of shelling out to vclient:
- Single persistent connection (reduces latency)
- No process spawning overhead
- Better error handling and reconnection logic

### vcontrold Protocol

Simple text protocol over TCP:
- Send: `command\n`
- Receive: `value unit\n` followed by `vctrld>` prompt
- Errors: Lines starting with `ERR:`
- Quit: `quit\n` -> `good bye!\n`

### MQTT Implementation

- Library: rumqttc with MQTT v5 protocol
- TLS: rustls (not OpenSSL) for smaller binary and easier cross-compilation
- Single client with shared access via Arc
- Separate event loop task handles connection management

## Development Environment

### Using devcontainer (recommended)

```bash
# Build dev image
docker build -t vcontrold-mqttd-dev -f .devcontainer/Dockerfile .devcontainer

# Run tests
docker run --rm -v "$(pwd)":/workspace -w /workspace vcontrold-mqttd-dev cargo test

# Interactive development
docker run --rm -it -v "$(pwd)":/workspace -w /workspace vcontrold-mqttd-dev bash
```

### Building Production Image

```bash
docker build -t vcontrold-mqttd:latest .
```

## Testing

### Unit Tests

```bash
cargo test
```

Current tests cover:
- Command batching algorithm
- Protocol response parsing
- JSON response building
- Numeric value formatting

### Integration Testing

Requires a running vcontrold instance or mock server. The devcontainer includes:
- vcontrold binary
- mosquitto broker (localhost:1883)

## Common Tasks

### Adding a New Environment Variable

1. Add field to appropriate struct in `src/config.rs`
2. Parse in `Config::from_env()` or `MqttConfig::from_env()`
3. Update SPEC.md with the new variable

### Modifying vcontrold Protocol Handling

- Protocol constants: `src/vcontrold/protocol.rs`
- Connection/command execution: `src/vcontrold/client.rs`

### Changing MQTT Behavior

- Client setup/TLS: `src/mqtt/client.rs`
- Publishing logic: `src/mqtt/publisher.rs`
- Request/response: `src/mqtt/subscriber.rs`

## Dependencies

Key crates:
- `tokio` - Async runtime
- `rumqttc` - MQTT v5 client (with rustls TLS via tokio-rustls)
- `rustls` v0.23 - TLS implementation (used directly for `ClientConfig` building; must be compatible with tokio-rustls version used by rumqttc)
- `serde`/`serde_json` - JSON serialization
- `tracing` - Structured logging
- `thiserror` - Error type derivation

## Gotchas

1. **rustls version**: rumqttc 0.25 uses tokio-rustls 0.26, which depends on rustls 0.23. The direct `rustls` dependency in Cargo.toml (used for building `ClientConfig` in `mqtt/client.rs`) must resolve to the same major version.

2. **Transport enum**: `rumqttc::Transport` is at crate root, not in `rumqttc::v5`.

3. **Publish lifetime**: rumqttc's publish requires owned data (`Vec<u8>`), not borrowed slices.

4. **tracing macro shadowing**: Don't name variables `debug`, `info`, etc. - they conflict with tracing macros.

## Git Workflow

- Never auto-push to remote
- Commit by topic when committing without explicit approval
- Amend existing commits by topic when modifying after committing
- Ask before destructive git operations
