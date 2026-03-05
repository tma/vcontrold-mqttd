//! Health check HTTP endpoint
//!
//! Provides a lightweight HTTP health endpoint using raw TCP (no framework
//! dependencies). Returns 200 when all components are healthy, 503 otherwise.
//! Also provides a `check_health` client function used by the `--healthcheck`
//! CLI flag for Docker's `HEALTHCHECK CMD`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{debug, error, info, warn};

/// Shared health state observed by the HTTP endpoint
pub struct HealthState {
    /// vcontrold daemon process is alive
    pub vcontrold_running: Arc<AtomicBool>,
    /// Persistent TCP connection to vcontrold is established
    pub vcontrold_connected: Arc<AtomicBool>,
    /// MQTT broker connection is active
    pub mqtt_connected: Arc<AtomicBool>,
}

impl HealthState {
    /// Evaluate overall health (all components must be healthy)
    fn is_healthy(&self) -> bool {
        self.vcontrold_running.load(Ordering::Relaxed)
            && self.vcontrold_connected.load(Ordering::Relaxed)
            && self.mqtt_connected.load(Ordering::Relaxed)
    }

    /// Build a JSON response body
    fn to_json(&self) -> String {
        let vcontrold_running = self.vcontrold_running.load(Ordering::Relaxed);
        let vcontrold_connected = self.vcontrold_connected.load(Ordering::Relaxed);
        let mqtt_connected = self.mqtt_connected.load(Ordering::Relaxed);
        let healthy = vcontrold_running && vcontrold_connected && mqtt_connected;

        format!(
            r#"{{"healthy":{},"vcontrold_process":{},"vcontrold_connection":{},"mqtt_connected":{}}}"#,
            healthy, vcontrold_running, vcontrold_connected, mqtt_connected,
        )
    }
}

/// Run the health check HTTP server
///
/// Listens on `0.0.0.0:<port>` and responds to every request with the current
/// health status as JSON. No HTTP parsing beyond reading enough to drain the
/// request — any TCP connection gets a response.
pub async fn run_health_server(port: u16, state: Arc<HealthState>) {
    let addr = format!("0.0.0.0:{}", port);
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => {
            info!("Health endpoint listening on {}", addr);
            l
        }
        Err(e) => {
            error!("Failed to bind health endpoint on {}: {}", addr, e);
            return;
        }
    };

    serve_health(listener, state).await;
}

/// Accept loop for the health endpoint (separated for testability)
async fn serve_health(listener: TcpListener, state: Arc<HealthState>) {
    loop {
        let (mut stream, peer) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                warn!("Health endpoint accept error: {}", e);
                continue;
            }
        };

        let body = state.to_json();
        let status = if state.is_healthy() {
            "200 OK"
        } else {
            "503 Service Unavailable"
        };

        debug!("Health check from {}: {}", peer, status);

        let response = format!(
            "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            status,
            body.len(),
            body,
        );

        // Best-effort: read whatever the client sent (so we don't RST)
        let mut discard = [0u8; 1024];
        let _ = stream.read(&mut discard).await;

        if let Err(e) = stream.write_all(response.as_bytes()).await {
            debug!("Health endpoint write error: {}", e);
        }
    }
}

/// Synchronous health check client used by `--healthcheck` CLI flag.
///
/// Connects to the health endpoint, sends a minimal HTTP GET, and returns
/// `true` if the server responds with 200.
pub fn check_health(port: u16) -> bool {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let addr = format!("127.0.0.1:{}", port);
    let timeout = Duration::from_secs(5);

    let mut stream = match TcpStream::connect_timeout(&addr.parse().unwrap(), timeout) {
        Ok(s) => s,
        Err(_) => return false,
    };

    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let request = "GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }

    let mut response = String::new();
    if stream.read_to_string(&mut response).is_err() {
        return false;
    }

    response.starts_with("HTTP/1.1 200")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_state_all_healthy() {
        let state = HealthState {
            vcontrold_running: Arc::new(AtomicBool::new(true)),
            vcontrold_connected: Arc::new(AtomicBool::new(true)),
            mqtt_connected: Arc::new(AtomicBool::new(true)),
        };
        assert!(state.is_healthy());
        let json = state.to_json();
        assert!(json.contains(r#""healthy":true"#));
        assert!(json.contains(r#""vcontrold_process":true"#));
        assert!(json.contains(r#""vcontrold_connection":true"#));
        assert!(json.contains(r#""mqtt_connected":true"#));
    }

    #[test]
    fn health_state_mqtt_down() {
        let state = HealthState {
            vcontrold_running: Arc::new(AtomicBool::new(true)),
            vcontrold_connected: Arc::new(AtomicBool::new(true)),
            mqtt_connected: Arc::new(AtomicBool::new(false)),
        };
        assert!(!state.is_healthy());
        let json = state.to_json();
        assert!(json.contains(r#""healthy":false"#));
        assert!(json.contains(r#""mqtt_connected":false"#));
    }

    #[test]
    fn health_state_vcontrold_process_down() {
        let state = HealthState {
            vcontrold_running: Arc::new(AtomicBool::new(false)),
            vcontrold_connected: Arc::new(AtomicBool::new(true)),
            mqtt_connected: Arc::new(AtomicBool::new(true)),
        };
        assert!(!state.is_healthy());
        let json = state.to_json();
        assert!(json.contains(r#""healthy":false"#));
        assert!(json.contains(r#""vcontrold_process":false"#));
    }

    #[test]
    fn health_state_vcontrold_connection_down() {
        let state = HealthState {
            vcontrold_running: Arc::new(AtomicBool::new(true)),
            vcontrold_connected: Arc::new(AtomicBool::new(false)),
            mqtt_connected: Arc::new(AtomicBool::new(true)),
        };
        assert!(!state.is_healthy());
        let json = state.to_json();
        assert!(json.contains(r#""healthy":false"#));
        assert!(json.contains(r#""vcontrold_connection":false"#));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn health_server_returns_200_when_healthy() {
        let state = Arc::new(HealthState {
            vcontrold_running: Arc::new(AtomicBool::new(true)),
            vcontrold_connected: Arc::new(AtomicBool::new(true)),
            mqtt_connected: Arc::new(AtomicBool::new(true)),
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let state_clone = Arc::clone(&state);
        let server = tokio::spawn(async move {
            serve_health(listener, state_clone).await;
        });

        // Give the spawned task a chance to start polling accept()
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert!(check_health(port));

        server.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn health_server_returns_503_when_unhealthy() {
        let state = Arc::new(HealthState {
            vcontrold_running: Arc::new(AtomicBool::new(true)),
            vcontrold_connected: Arc::new(AtomicBool::new(false)),
            mqtt_connected: Arc::new(AtomicBool::new(true)),
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let state_clone = Arc::clone(&state);
        let server = tokio::spawn(async move {
            serve_health(listener, state_clone).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert!(!check_health(port));

        server.abort();
    }
}
