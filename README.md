# vcontrold-mqttd

A Rust-based MQTT bridge for [vcontrold](https://github.com/openv/vcontrold) (Viessmann heating controller interface).

## Features

- **Native TCP communication** with vcontrold (no vclient subprocess spawning)
- **Persistent connection** with automatic reconnection
- **MQTT v5** protocol support via rumqttc
- **TLS encryption** with rustls (supports CA certs, client certs, insecure mode)
- **Periodic polling** of heating controller parameters
- **Request/response bridge** for ad-hoc commands via MQTT
- **Command batching** to respect protocol limits
- **Multi-architecture** Docker images (amd64, arm64, armhf)

## Quick Start

```bash
docker build -t vcontrold-mqttd:latest .
```

Create a `docker-compose.yml`:

```yaml
services:
  vcontrold-mqttd:
    image: vcontrold-mqttd:latest
    restart: unless-stopped
    devices:
      - /dev/ttyUSB0:/dev/vitocal
    volumes:
      - ./config:/config:ro
    environment:
      MQTT_HOST: mqtt.example.com
      MQTT_TOPIC: heating
      MQTT_SUBSCRIBE: "true"
      COMMANDS: getTempA,getTempWW,getTempWWsoll
      INTERVAL: "60"
      # DEBUG: "true"
      # MQTT_USER: myuser
      # MQTT_PASSWORD: mypassword
      # MQTT_TLS: "true"
```

```bash
docker compose up -d
```

## Configuration

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `MQTT_HOST` | - | MQTT broker hostname (**required**) |
| `MQTT_TOPIC` | - | Base topic prefix (**required**) |
| `MQTT_PORT` | `1883` | MQTT broker port |
| `MQTT_USER` | - | MQTT username |
| `MQTT_PASSWORD` | - | MQTT password |
| `MQTT_CLIENT_ID_PREFIX` | `vcontrold` | Client ID prefix |
| `MQTT_SUBSCRIBE` | `false` | Enable request/response bridge |
| `MQTT_TLS` | `false` | Enable TLS |
| `MQTT_CAFILE` | - | CA certificate file |
| `MQTT_CAPATH` | - | CA certificate directory |
| `MQTT_CERTFILE` | - | Client certificate file |
| `MQTT_KEYFILE` | - | Client private key file |
| `MQTT_TLS_INSECURE` | `false` | Skip certificate validation |
| `COMMANDS` | - | Comma-separated commands to poll |
| `INTERVAL` | `60` | Polling interval in seconds |
| `MAX_LENGTH` | `512` | Max batch length in characters |
| `USB_DEVICE` | `/dev/vitocal` | Serial device path inside container |
| `DEBUG` | `false` | Enable debug logging |

### Required Files

Mount your vcontrold configuration to `/config`:

```
/config/
├── vcontrold.xml    # Main configuration
└── vito.xml         # Command definitions (optional, can be included)
```

Example `vcontrold.xml`:
```xml
<?xml version="1.0"?>
<vcontrold>
  <unix>
    <config>
      <tty>/dev/vitocal</tty>
      <port>3002</port>
      <allow ip='127.0.0.1'/>
    </config>
  </unix>
  <device ID="2098"/>
  <commands>
    <include path="/config/vito.xml"/>
  </commands>
</vcontrold>
```

## MQTT Topics

### Polling

Values are published to:
```
${MQTT_TOPIC}/command/<command_name>
```

Example:
```
heating/command/getTempA -> 21.5
heating/command/getTempWW -> 48.1
```

### Request/Response (MQTT_SUBSCRIBE=true)

Send commands to:
```
${MQTT_TOPIC}/request
```

Receive responses on:
```
${MQTT_TOPIC}/response
```

Request format:
```
getTempA                          # Single command
getTempA,getTempWW                # Multiple commands
setTempWWsoll 50                  # Write command
getTempA,setTempWWsoll 50         # Mixed
```

Response format (JSON):
```json
{"getTempA":21.5,"getTempWW":48.1}
```

## Development

### Prerequisites

- Docker
- (Optional) Rust toolchain for local development

### Build Development Container

```bash
docker build -t vcontrold-mqttd-dev -f .devcontainer/Dockerfile .devcontainer
```

### Run Tests

```bash
docker run --rm -v "$(pwd)":/workspace -w /workspace vcontrold-mqttd-dev cargo test
```

### Build Release

```bash
docker run --rm -v "$(pwd)":/workspace -w /workspace vcontrold-mqttd-dev cargo build --release
```

### Interactive Development

```bash
docker run --rm -it -v "$(pwd)":/workspace -w /workspace vcontrold-mqttd-dev bash
```

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
