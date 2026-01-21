//! vcontrold TCP client with persistent connection
//!
//! Manages a persistent TCP connection to vcontrold, with automatic reconnection.

use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

use crate::error::VcontroldError;

use super::protocol::{
    extract_response, format_command, format_quit, has_prompt, parse_response, validate_command,
    CommandResult,
};

/// Default vcontrold port
pub const DEFAULT_PORT: u16 = 3002;

/// Connection timeout
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Read timeout for responses
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// vcontrold client with persistent connection
pub struct VcontroldClient {
    host: String,
    port: u16,
    connection: Mutex<Option<Connection>>,
}

struct Connection {
    reader: BufReader<tokio::io::ReadHalf<TcpStream>>,
    writer: tokio::io::WriteHalf<TcpStream>,
}

impl VcontroldClient {
    /// Create a new client (does not connect immediately)
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            connection: Mutex::new(None),
        }
    }

    /// Create a client for localhost
    pub fn localhost() -> Self {
        Self::new("127.0.0.1", DEFAULT_PORT)
    }

    /// Ensure we have an active connection, reconnecting if necessary
    async fn ensure_connected(&self) -> Result<(), VcontroldError> {
        let mut conn_guard = self.connection.lock().await;
        if conn_guard.is_none() {
            info!("Connecting to vcontrold at {}:{}", self.host, self.port);
            let connection = self.connect_internal().await?;
            *conn_guard = Some(connection);
        }
        Ok(())
    }

    /// Internal connection logic
    async fn connect_internal(&self) -> Result<Connection, VcontroldError> {
        let addr = format!("{}:{}", self.host, self.port);

        let stream = timeout(CONNECT_TIMEOUT, TcpStream::connect(&addr))
            .await
            .map_err(|_| VcontroldError::ConnectionFailed("connection timeout".to_string()))?
            .map_err(|e| VcontroldError::ConnectionFailed(e.to_string()))?;

        let (read_half, write_half) = tokio::io::split(stream);
        let mut reader = BufReader::new(read_half);

        // Wait for initial prompt
        let mut buffer = String::new();
        loop {
            let read_result = timeout(READ_TIMEOUT, reader.read_line(&mut buffer)).await;

            match read_result {
                Ok(Ok(0)) => {
                    return Err(VcontroldError::ConnectionFailed(
                        "connection closed".to_string(),
                    ))
                }
                Ok(Ok(_)) => {
                    if has_prompt(&buffer) {
                        debug!("Received initial prompt from vcontrold");
                        break;
                    }
                }
                Ok(Err(e)) => return Err(VcontroldError::Io(e)),
                Err(_) => {
                    return Err(VcontroldError::Timeout);
                }
            }
        }

        Ok(Connection {
            reader,
            writer: write_half,
        })
    }

    /// Execute a single command and return the result
    pub async fn execute(&self, command: &str) -> Result<CommandResult, VcontroldError> {
        validate_command(command)?;
        self.ensure_connected().await?;

        let mut conn_guard = self.connection.lock().await;
        let conn = conn_guard
            .as_mut()
            .ok_or(VcontroldError::ConnectionLost)?;

        // Send command
        let cmd_str = format_command(command);
        debug!("Sending command: {}", command);
        conn.writer
            .write_all(cmd_str.as_bytes())
            .await
            .map_err(|e| {
                error!("Failed to send command: {}", e);
                VcontroldError::Io(e)
            })?;
        conn.writer.flush().await.map_err(VcontroldError::Io)?;

        // Read response until prompt
        let mut buffer = String::new();
        loop {
            let read_result = timeout(READ_TIMEOUT, conn.reader.read_line(&mut buffer)).await;

            match read_result {
                Ok(Ok(0)) => {
                    // Connection closed, mark as disconnected
                    drop(conn_guard);
                    *self.connection.lock().await = None;
                    return Err(VcontroldError::ConnectionLost);
                }
                Ok(Ok(_)) => {
                    if has_prompt(&buffer) {
                        break;
                    }
                }
                Ok(Err(e)) => {
                    drop(conn_guard);
                    *self.connection.lock().await = None;
                    return Err(VcontroldError::Io(e));
                }
                Err(_) => {
                    return Err(VcontroldError::Timeout);
                }
            }
        }

        // Parse response
        let response = extract_response(&buffer).unwrap_or("");
        debug!("Received response: {}", response);
        Ok(parse_response(command, response))
    }

    /// Execute multiple commands and return all results
    pub async fn execute_batch(&self, commands: &[String]) -> Vec<Result<CommandResult, VcontroldError>> {
        let mut results = Vec::with_capacity(commands.len());
        for cmd in commands {
            results.push(self.execute(cmd).await);
        }
        results
    }

    /// Disconnect from vcontrold gracefully
    pub async fn disconnect(&self) {
        let mut conn_guard = self.connection.lock().await;
        if let Some(mut conn) = conn_guard.take() {
            debug!("Disconnecting from vcontrold");
            let _ = conn.writer.write_all(format_quit().as_bytes()).await;
            let _ = conn.writer.flush().await;
        }
    }

    /// Check if vcontrold is responding (for readiness probes)
    pub async fn is_ready(&self) -> bool {
        // Try to connect and receive initial prompt
        match self.connect_internal().await {
            Ok(_) => true,
            Err(e) => {
                debug!("Readiness check failed: {}", e);
                false
            }
        }
    }

    /// Mark connection as lost (called when we detect issues)
    #[allow(dead_code)]
    pub async fn mark_disconnected(&self) {
        let mut conn_guard = self.connection.lock().await;
        if conn_guard.take().is_some() {
            warn!("Connection marked as disconnected");
        }
    }
}

impl Drop for VcontroldClient {
    fn drop(&mut self) {
        // Note: async disconnect not possible in drop, connection will just close
    }
}
