//! vcontrold TCP client with persistent connection
//!
//! Manages a persistent TCP connection to vcontrold, with automatic reconnection.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, MutexGuard};
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

use crate::error::VcontroldError;

use super::protocol::{
    extract_response, format_command, format_quit, is_fatal_error_response, parse_response,
    validate_command, CommandResult, PROMPT,
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
    /// Tracks whether the persistent TCP connection is alive.
    /// Updated on connect/disconnect; exposed for health checks.
    connected: Arc<AtomicBool>,
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
            connected: Arc::new(AtomicBool::new(false)),
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
            self.connected.store(true, Ordering::Relaxed);
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

        // Wait for initial prompt (no newline, so read byte by byte)
        let mut buffer = String::new();
        let result = timeout(READ_TIMEOUT, read_until_prompt(&mut reader, &mut buffer)).await;

        match result {
            Ok(Ok(())) => {
                debug!("Received initial prompt from vcontrold");
            }
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(VcontroldError::Timeout),
        }

        Ok(Connection {
            reader,
            writer: write_half,
        })
    }

    /// Execute a single command and return the result
    pub async fn execute(&self, command: &str) -> Result<CommandResult, VcontroldError> {
        enum ExecuteOutcome {
            Success(CommandResult),
            FatalResponse(CommandResult),
            Error {
                error: VcontroldError,
                send_quit: bool,
            },
        }

        validate_command(command)?;
        self.ensure_connected().await?;

        let mut conn_guard = self.connection.lock().await;
        let outcome = {
            let conn = conn_guard.as_mut().ok_or(VcontroldError::ConnectionLost)?;

            // Send command
            let cmd_str = format_command(command);
            debug!("Sending command: {}", command);
            if let Err(e) = conn.writer.write_all(cmd_str.as_bytes()).await {
                error!("Failed to send command: {}", e);
                ExecuteOutcome::Error {
                    error: VcontroldError::Io(e),
                    send_quit: false,
                }
            } else if let Err(e) = conn.writer.flush().await {
                error!("Failed to flush command: {}", e);
                ExecuteOutcome::Error {
                    error: VcontroldError::Io(e),
                    send_quit: false,
                }
            } else {
                // Read response until prompt
                let mut buffer = String::new();
                let read_result = timeout(
                    READ_TIMEOUT,
                    read_until_prompt(&mut conn.reader, &mut buffer),
                )
                .await;

                match read_result {
                    Ok(Ok(())) => {
                        let response = extract_response(&buffer).unwrap_or("");
                        debug!("Received response: {}", response);

                        let result = parse_response(command, response);
                        if result.error.as_deref().is_some_and(is_fatal_error_response) {
                            ExecuteOutcome::FatalResponse(result)
                        } else {
                            ExecuteOutcome::Success(result)
                        }
                    }
                    Ok(Err(VcontroldError::ConnectionLost)) => ExecuteOutcome::Error {
                        error: VcontroldError::ConnectionLost,
                        send_quit: false,
                    },
                    Ok(Err(e)) => ExecuteOutcome::Error {
                        error: e,
                        send_quit: true,
                    },
                    Err(_) => ExecuteOutcome::Error {
                        error: VcontroldError::Timeout,
                        send_quit: true,
                    },
                }
            }
        };

        match outcome {
            ExecuteOutcome::Success(result) => Ok(result),
            ExecuteOutcome::FatalResponse(result) => {
                warn!(
                    "Fatal vcontrold session error for {} - resetting connection before the next command",
                    command
                );
                invalidate_locked_connection(&mut conn_guard, self.connected.as_ref(), true).await;
                Ok(result)
            }
            ExecuteOutcome::Error { error, send_quit } => {
                invalidate_locked_connection(&mut conn_guard, self.connected.as_ref(), send_quit)
                    .await;
                Err(error)
            }
        }
    }

    /// Execute multiple commands and return all results
    pub async fn execute_batch(
        &self,
        commands: &[String],
    ) -> Vec<Result<CommandResult, VcontroldError>> {
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
        self.connected.store(false, Ordering::Relaxed);
    }

    /// Check if vcontrold is responding (for readiness probes)
    pub async fn is_ready(&self) -> bool {
        // Try to connect and receive initial prompt
        match self.connect_internal().await {
            Ok(mut conn) => {
                // Gracefully disconnect so vcontrold doesn't accumulate
                // abandoned connections during the readiness probe loop.
                let _ = conn.writer.write_all(format_quit().as_bytes()).await;
                let _ = conn.writer.flush().await;
                true
            }
            Err(e) => {
                debug!("Readiness check failed: {}", e);
                false
            }
        }
    }

    /// Get a shared reference to the connection-alive flag (for health checks)
    pub fn connected_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.connected)
    }

    /// Mark connection as lost (called when we detect issues)
    #[allow(dead_code)]
    pub async fn mark_disconnected(&self) {
        let mut conn_guard = self.connection.lock().await;
        if conn_guard.take().is_some() {
            warn!("Connection marked as disconnected");
        }
        self.connected.store(false, Ordering::Relaxed);
    }
}

async fn invalidate_locked_connection(
    conn_guard: &mut MutexGuard<'_, Option<Connection>>,
    connected: &AtomicBool,
    send_quit: bool,
) {
    if let Some(mut conn) = conn_guard.take() {
        if send_quit {
            let _ = conn.writer.write_all(format_quit().as_bytes()).await;
            let _ = conn.writer.flush().await;
        }
    }

    connected.store(false, Ordering::Relaxed);
}

/// Read from reader until the prompt is found
///
/// Accumulates raw bytes and checks for the prompt in the byte stream.
/// Once found, the buffer is converted to a (lossy) UTF-8 string.
/// This avoids silently dropping non-ASCII bytes (e.g. `°` in unit
/// strings) that would break prompt detection.
async fn read_until_prompt<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    buffer: &mut String,
) -> Result<(), VcontroldError> {
    let prompt_bytes = PROMPT.as_bytes();
    let mut raw = Vec::new();
    let mut byte_buf = [0u8; 1];
    loop {
        match reader.read(&mut byte_buf).await {
            Ok(0) => return Err(VcontroldError::ConnectionLost),
            Ok(_) => {
                raw.push(byte_buf[0]);
                if raw.ends_with(prompt_bytes) {
                    *buffer = String::from_utf8_lossy(&raw).into_owned();
                    return Ok(());
                }
            }
            Err(e) => return Err(VcontroldError::Io(e)),
        }
    }
}

impl Drop for VcontroldClient {
    fn drop(&mut self) {
        // Note: async disconnect not possible in drop, connection will just close
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcontrold::Value;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};

    async fn write_prompt(stream: &mut TcpStream) {
        stream.write_all(PROMPT.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();
    }

    #[tokio::test]
    async fn execute_keeps_connection_after_non_fatal_error_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            write_prompt(&mut stream).await;

            let mut reader = BufReader::new(stream);
            let mut command = String::new();
            reader.read_line(&mut command).await.unwrap();
            assert_eq!(command, "badCommand\n");

            let mut stream = reader.into_inner();
            stream
                .write_all(b"ERR: command unknown\nvctrld>")
                .await
                .unwrap();
            stream.flush().await.unwrap();

            let mut reader = BufReader::new(stream);
            let mut command = String::new();
            reader.read_line(&mut command).await.unwrap();
            assert_eq!(command, "getTempWWsoll\n");

            let mut stream = reader.into_inner();
            stream
                .write_all(b"48.1 Grad Celsius\nvctrld>")
                .await
                .unwrap();
            stream.flush().await.unwrap();

            let mut reader = BufReader::new(stream);
            let mut quit = String::new();
            reader.read_line(&mut quit).await.unwrap();
            assert_eq!(quit, "quit\n");
        });

        let client = VcontroldClient::new("127.0.0.1", port);

        let first = client.execute("badCommand").await.unwrap();
        assert_eq!(first.error.as_deref(), Some("ERR: command unknown"));
        assert!(client.connected_flag().load(Ordering::Relaxed));

        let second = client.execute("getTempWWsoll").await.unwrap();
        assert!(matches!(second.value, Value::Number(n) if (n - 48.1).abs() < 0.001));
        assert!(second.error.is_none());
        assert!(client.connected_flag().load(Ordering::Relaxed));

        client.disconnect().await;
        server.await.unwrap();
    }

    #[tokio::test]
    async fn execute_resets_connection_after_fatal_error_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            let (mut first_stream, _) = listener.accept().await.unwrap();
            write_prompt(&mut first_stream).await;

            let mut first_reader = BufReader::new(first_stream);
            let mut command = String::new();
            first_reader.read_line(&mut command).await.unwrap();
            assert_eq!(command, "getTempA\n");

            let mut first_stream = first_reader.into_inner();
            first_stream
                .write_all(
                    b"ERR: >FRAMER: Error 0x05 != 0x06 (P300_INIT_OK)\nError in send, terminating\nError executing getTempA\nvctrld>",
                )
                .await
                .unwrap();
            first_stream.flush().await.unwrap();

            let mut quit = String::new();
            let mut first_reader = BufReader::new(first_stream);
            first_reader.read_line(&mut quit).await.unwrap();
            assert_eq!(quit, "quit\n");

            let (mut second_stream, _) = listener.accept().await.unwrap();
            write_prompt(&mut second_stream).await;

            let mut second_reader = BufReader::new(second_stream);
            let mut command = String::new();
            second_reader.read_line(&mut command).await.unwrap();
            assert_eq!(command, "getTempWWsoll\n");

            let mut second_stream = second_reader.into_inner();
            second_stream
                .write_all(b"48.1 Grad Celsius\nvctrld>")
                .await
                .unwrap();
            second_stream.flush().await.unwrap();
        });

        let client = VcontroldClient::new("127.0.0.1", port);

        let first = client.execute("getTempA").await.unwrap();
        assert!(
            first.error.as_deref().is_some_and(is_fatal_error_response),
            "first response should preserve the fatal vcontrold error text"
        );
        assert!(!client.connected_flag().load(Ordering::Relaxed));

        let second = client.execute("getTempWWsoll").await.unwrap();
        assert!(matches!(second.value, Value::Number(n) if (n - 48.1).abs() < 0.001));
        assert!(second.error.is_none());
        assert!(client.connected_flag().load(Ordering::Relaxed));

        client.disconnect().await;
        server.await.unwrap();
    }
}
