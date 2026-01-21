//! Configuration module for vcontrold-mqttd
//!
//! Parses environment variables into a strongly-typed configuration struct.

use std::env;
use std::path::PathBuf;
use std::time::Duration;

/// Main configuration struct containing all settings
#[derive(Debug, Clone)]
pub struct Config {
    /// Serial device path inside container (reserved for future use)
    #[allow(dead_code)]
    pub usb_device: PathBuf,
    /// Max character length per vclient batch
    pub max_length: usize,
    /// Enable request/response bridge
    pub mqtt_subscribe: bool,
    /// MQTT broker configuration
    pub mqtt: MqttConfig,
    /// Seconds between polling cycles
    pub interval: Duration,
    /// Comma-separated list of command names to poll
    pub commands: Vec<String>,
    /// Enable verbose logging
    pub debug: bool,
}

/// MQTT-specific configuration
#[derive(Debug, Clone)]
pub struct MqttConfig {
    /// Broker hostname/IP
    pub host: String,
    /// Broker TCP port
    pub port: u16,
    /// Base topic prefix
    pub topic: String,
    /// Username (empty = anonymous)
    pub user: Option<String>,
    /// Password
    pub password: Option<String>,
    /// Prefix for MQTT client IDs
    pub client_id_prefix: String,
    /// Publish timeout (reserved for future use)
    #[allow(dead_code)]
    pub timeout: Duration,
    /// TLS configuration
    pub tls: Option<TlsConfig>,
}

/// TLS configuration for MQTT
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// CA certificate file path
    pub ca_file: Option<PathBuf>,
    /// CA certificate directory path
    pub ca_path: Option<PathBuf>,
    /// Client certificate path
    pub cert_file: Option<PathBuf>,
    /// Private key path
    pub key_file: Option<PathBuf>,
    /// TLS version hint (reserved for future use, rustls handles this automatically)
    #[allow(dead_code)]
    pub tls_version: Option<String>,
    /// Skip certificate validation
    pub insecure: bool,
}

/// Configuration error type
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing required environment variable: {0}")]
    MissingRequired(&'static str),
    #[error("invalid value for {0}: {1}")]
    InvalidValue(&'static str, String),
}

impl Config {
    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self, ConfigError> {
        let mqtt_subscribe = parse_bool("MQTT_SUBSCRIBE", false);

        // MQTT_HOST and MQTT_TOPIC are always required
        let mqtt_host =
            env::var("MQTT_HOST").map_err(|_| ConfigError::MissingRequired("MQTT_HOST"))?;
        let mqtt_topic =
            env::var("MQTT_TOPIC").map_err(|_| ConfigError::MissingRequired("MQTT_TOPIC"))?;

        let tls_enabled = parse_bool("MQTT_TLS", false);
        let tls = if tls_enabled {
            Some(TlsConfig {
                ca_file: env::var("MQTT_CAFILE").ok().map(PathBuf::from),
                ca_path: env::var("MQTT_CAPATH").ok().map(PathBuf::from),
                cert_file: env::var("MQTT_CERTFILE").ok().map(PathBuf::from),
                key_file: env::var("MQTT_KEYFILE").ok().map(PathBuf::from),
                tls_version: env::var("MQTT_TLS_VERSION").ok().filter(|s| !s.is_empty()),
                insecure: parse_bool("MQTT_TLS_INSECURE", false),
            })
        } else {
            None
        };

        let commands_str = env::var("COMMANDS").unwrap_or_default();
        let commands: Vec<String> = commands_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        Ok(Config {
            usb_device: PathBuf::from(
                env::var("USB_DEVICE").unwrap_or_else(|_| "/dev/vitocal".to_string()),
            ),
            max_length: parse_usize("MAX_LENGTH", 512)?,
            mqtt_subscribe,
            mqtt: MqttConfig {
                host: mqtt_host,
                port: parse_u16("MQTT_PORT", 1883)?,
                topic: mqtt_topic,
                user: env::var("MQTT_USER").ok().filter(|s| !s.is_empty()),
                password: env::var("MQTT_PASSWORD").ok().filter(|s| !s.is_empty()),
                client_id_prefix: env::var("MQTT_CLIENT_ID_PREFIX")
                    .unwrap_or_else(|_| "vcontrold".to_string()),
                timeout: Duration::from_secs(parse_u64("MQTT_TIMEOUT", 10)?),
                tls,
            },
            interval: Duration::from_secs(parse_u64("INTERVAL", 60)?),
            commands,
            debug: parse_bool("DEBUG", false),
        })
    }

    /// Generate a unique client ID for the publisher
    pub fn publisher_client_id(&self) -> String {
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        let pid = std::process::id();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!(
            "{}-pub-{}-{}-{}",
            self.mqtt.client_id_prefix, hostname, pid, timestamp
        )
    }

    /// Generate a unique client ID for the subscriber (reserved for future use)
    #[allow(dead_code)]
    pub fn subscriber_client_id(&self) -> String {
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        format!("{}-sub-{}", self.mqtt.client_id_prefix, hostname)
    }
}

fn parse_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .map(|v| matches!(v.to_lowercase().as_str(), "true" | "1" | "yes"))
        .unwrap_or(default)
}

fn parse_u16(name: &'static str, default: u16) -> Result<u16, ConfigError> {
    match env::var(name) {
        Ok(v) if !v.is_empty() => v
            .parse()
            .map_err(|_| ConfigError::InvalidValue(name, v)),
        _ => Ok(default),
    }
}

fn parse_u64(name: &'static str, default: u64) -> Result<u64, ConfigError> {
    match env::var(name) {
        Ok(v) if !v.is_empty() => v
            .parse()
            .map_err(|_| ConfigError::InvalidValue(name, v)),
        _ => Ok(default),
    }
}

fn parse_usize(name: &'static str, default: usize) -> Result<usize, ConfigError> {
    match env::var(name) {
        Ok(v) if !v.is_empty() => v
            .parse()
            .map_err(|_| ConfigError::InvalidValue(name, v)),
        _ => Ok(default),
    }
}
