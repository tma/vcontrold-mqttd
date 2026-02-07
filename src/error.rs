//! Error types for vcontrold-mqttd

use thiserror::Error;

/// Main error type for the application
#[derive(Error, Debug)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(#[from] crate::config::ConfigError),

    #[error("vcontrold error: {0}")]
    Vcontrold(#[from] VcontroldError),

    #[error("MQTT error: {0}")]
    Mqtt(#[from] MqttError),

    #[error("process error: {0}")]
    Process(#[from] ProcessError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Errors related to vcontrold communication
#[derive(Error, Debug)]
pub enum VcontroldError {
    #[error("connection failed: {0}")]
    ConnectionFailed(String),

    #[error("connection lost")]
    ConnectionLost,

    #[error("protocol error: {0}")]
    #[allow(dead_code)]
    Protocol(String),

    #[error("command error: {0}")]
    Command(String),

    #[error("timeout waiting for response")]
    Timeout,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Errors related to MQTT operations
#[derive(Error, Debug)]
pub enum MqttError {
    #[error("connection failed: {0}")]
    ConnectionFailed(String),

    #[error("publish failed: {0}")]
    PublishFailed(String),

    #[error("client error: {0}")]
    #[allow(dead_code)]
    Client(String),
}

/// Errors related to process management
#[derive(Error, Debug)]
pub enum ProcessError {
    #[error("vcontrold failed to start: {0}")]
    StartFailed(String),

    #[error("failed waiting for vcontrold process: {0}")]
    WaitFailed(String),

    #[error("vcontrold exited unexpectedly with code {0:?}")]
    UnexpectedExit(Option<i32>),

    #[error("readiness probe failed after {0} seconds")]
    ReadinessTimeout(u64),

    #[error("config file not found: {0}")]
    ConfigNotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;
