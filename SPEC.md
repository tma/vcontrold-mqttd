# Specification: vcontrold MQTT Bridge

This document specifies the behavior and feature set of the vcontrold Docker container.

## Overview

A containerized wrapper around **vcontrold** (Viessmann heating controller interface) that provides:
- Serial communication with Viessmann heating systems via Optolink/FTDI USB adapter
- Automated periodic polling of heating controller parameters
- MQTT bridge for remote query and control
- JSON response formatting

## Implementation Status

| Feature | Status |
|---------|--------|
| vcontrold daemon management | Done |
| Native TCP client (persistent connection) | Done |
| Command batching algorithm | Done |
| Periodic polling loop | Done |
| MQTT v5 publishing | Done |
| Request/response bridge | Done |
| TLS support (rustls) | Done |
| Client certificate auth | Done |
| Insecure TLS mode | Done |
| Environment variable parsing | Done |
| Debug logging | Done |
| Unit tests | Done |
| Production Dockerfile | Done |
| Devcontainer | Done |

## Environment Variables

### Required

| Variable | Description |
|----------|-------------|
| `MQTT_HOST` | Broker hostname/IP |
| `MQTT_TOPIC` | Base topic prefix (e.g., `vcontrold`) |

### Optional

| Variable | Default | Description |
|----------|---------|-------------|
| `USB_DEVICE` | `/dev/vitocal` | Serial device path inside container |
| `MAX_LENGTH` | `512` | Max character length per command batch |
| `MQTT_SUBSCRIBE` | `false` | Enable request/response bridge
| `MQTT_PORT` | `1883` | Broker TCP port |
| `MQTT_USER` | `""` | Username (empty = anonymous) |
| `MQTT_PASSWORD` | `""` | Password |
| `MQTT_CLIENT_ID_PREFIX` | `vcontrold` | Prefix for MQTT client IDs |
| `MQTT_TIMEOUT` | `10` | Publish timeout in seconds |
| `MQTT_TLS` | `false` | Enable TLS encryption |
| `MQTT_CAFILE` | `""` | CA certificate file path |
| `MQTT_CAPATH` | `""` | CA certificate directory path |
| `MQTT_CERTFILE` | `""` | Client certificate path |
| `MQTT_KEYFILE` | `""` | Private key path |
| `MQTT_TLS_VERSION` | `""` | TLS version hint (e.g., `tlsv1.2`) |
| `MQTT_TLS_INSECURE` | `false` | Skip certificate validation |
| `INTERVAL` | `60` | Seconds between polling cycles |
| `COMMANDS` | `""` | Comma-separated list of command names to poll |
| `DEBUG` | `false` | Enable verbose logging |

## vcontrold Daemon

Runs vcontrold with the user-provided XML configuration:

- Normal: `vcontrold -n -x /config/vcontrold.xml`
- Debug (`DEBUG=true`): `vcontrold -n -x /config/vcontrold.xml --verbose --debug`

The container exits if vcontrold dies.

## MQTT Topic Structure

### Periodic Publishing

When `COMMANDS` is set:

**Topic**: `${MQTT_TOPIC}/command/<command_name>`
**Payload**: Numeric or string value only
**Retained**: Yes
**Protocol**: MQTT v5

Example:
```
Topic: vcontrold/command/getTempWWObenIst
Payload: 48.1
```

### Request/Response Bridge

When `MQTT_SUBSCRIBE=true`:

**Request Topic**: `${MQTT_TOPIC}/request`
**Response Topic**: `${MQTT_TOPIC}/response`
**Response Retained**: Yes

#### Request Format

Single command:
```
getTempWWObenIst
```

Multiple commands (comma-separated):
```
getTempWWObenIst,getTempWWsoll
```

Write command (space-separated value):
```
setTempWWsoll 50
```

Multiple mixed operations:
```
set1xWW 2,setTempWWsoll 50,getTempA
```

#### Response Format

JSON with flat structure (vclient `-j` style):

```json
{"getTempWWObenIst":48.1}
```

```json
{"getTempWWObenIst":48.1,"getTempWWsoll":50}
```

```json
{"setTempWWsoll":"OK"}
```

Errors are included with error message as value.

## Native TCP Communication

The Rust implementation uses direct TCP communication to vcontrold instead of shelling out to vclient:

### Protocol

```
Client -> Server: command\n
Server -> Client: value unit\n
Server -> Client: vctrld>
```

Error responses start with `ERR:`.

### Benefits

- Single persistent connection (reduces latency)
- No process spawning overhead per command
- Better error handling and automatic reconnection
- Reduced resource usage

## Polling Loop Behavior

1. Parse `COMMANDS` as comma-separated list
2. Batch commands into groups respecting `MAX_LENGTH` character limit
3. For each batch:
   - Execute commands via persistent TCP connection
   - Parse responses
   - Publish each value to `${MQTT_TOPIC}/command/<name>`
4. Sleep `INTERVAL` seconds
5. Repeat

### Command Batching Algorithm

```
batch = ""
for each command in COMMANDS:
    if length(batch + "," + command) > MAX_LENGTH:
        execute_batch(batch)
        batch = command
    else:
        batch = batch + "," + command
execute_batch(batch)
```

### Response Parsing

vcontrold returns responses in format:
```
value unit
```

The parser extracts:
- Numeric values (float or integer)
- String values (for status/error responses)
- Unit information (for logging)

## Subscriber Behavior

1. Connect to MQTT broker
2. Subscribe to `${MQTT_TOPIC}/request`
3. For each message:
   - Skip empty payloads
   - Parse comma-separated commands
   - Execute each command via TCP connection
   - Build JSON response
   - Publish response to `${MQTT_TOPIC}/response`
4. On disconnect: automatic reconnection via rumqttc

## Error Handling

### Startup Errors

| Condition | Behavior |
|-----------|----------|
| Missing `/config/vcontrold.xml` | Exit code 1, log error |
| vcontrold crashes on startup | Exit code 1, log error |
| vcontrold fails readiness probe (30s) | Exit code 1, log error |
| Missing `MQTT_HOST` or `MQTT_TOPIC` | Exit code 1, log error |

### Runtime Errors

| Condition | Behavior |
|-----------|----------|
| vcontrold process dies | Exit container immediately |
| TCP connection lost | Automatic reconnect on next command |
| Command execution fails | Log warning, continue polling |
| MQTT connection lost | Automatic reconnect via rumqttc |

## Debug Output

When `DEBUG=true`:

- vcontrold: `--verbose --debug` flags (protocol-level output)
- Polling: logs each command batch and response
- Publishing: logs topic and payload
- Subscriber: logs received payloads
- TCP: logs connection and command details

Example:
```
Debug mode enabled
Starting vcontrold with config: /config/vcontrold.xml (debug mode)
Polling 5 commands in 2 batches every 60 seconds
Executing batch 1: getTempWWObenIst,getTempWWsoll
Command getTempWWObenIst returned: 48.1
Publishing to vcontrold/command/getTempWWObenIst: 48.1
```

## Configuration Files

### /config/vcontrold.xml

Main vcontrold configuration. Must define:
- Serial device: `<tty>/dev/vitocal</tty>`
- TCP port: `<port>3002</port>`
- Allow localhost: `<allow ip='127.0.0.1'/>`
- Device ID matching your hardware
- Command definitions (or include vito.xml)

### /config/vito.xml

Device-specific command definitions:

```xml
<command name="getTempWWObenIst" protocmd="getaddr">
  <addr>010D</addr>
  <len>2</len>
  <unit>UT</unit>
</command>
```

## MQTT Client IDs

Generated client IDs to avoid collisions:

- Publisher: `${MQTT_CLIENT_ID_PREFIX}-${hostname}-${timestamp}`

## TLS Configuration

TLS is implemented using rustls (not OpenSSL) for:
- Smaller binary size
- Easier cross-compilation
- No system OpenSSL dependency

### Certificate Loading

1. If `MQTT_CAFILE` set: Load specific CA certificate
2. If `MQTT_CAPATH` set: Load all `.crt`/`.pem` files from directory
3. Otherwise: Use webpki-roots (Mozilla's root certificates)

### Client Authentication

When both `MQTT_CERTFILE` and `MQTT_KEYFILE` are set:
- Supports PKCS#1, PKCS#8, and SEC1 (EC) key formats
- PEM-encoded certificates and keys

### Insecure Mode

When `MQTT_TLS_INSECURE=true`:
- Skips server certificate validation
- Useful for self-signed certificates in development
- **Not recommended for production**

## Dependencies

Runtime (in container):
- vcontrold daemon
- libxml2 (vcontrold dependency)

Build-time:
- Rust toolchain
- See `Cargo.toml` for crate dependencies

## Process Architecture

```
vcontrold-mqttd (PID 1, Rust binary)
├── Spawns: vcontrold daemon
├── Task: vcontrold process monitor
├── Task: MQTT event loop
├── Task: Polling loop (if COMMANDS set)
└── Task: Subscriber (if MQTT_SUBSCRIBE=true)
```

All tasks run concurrently via tokio. If any critical task fails, the container exits.

## Container Requirements

- Serial device access (dialout group)
- Volume mount for `/config`
- Network access to MQTT broker
- Optional: TLS certificate mounts
