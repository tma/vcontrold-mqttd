//! Process management for vcontrold daemon
//!
//! Spawns and monitors the vcontrold process.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::time::sleep;
use tracing::{error, info, warn};

use crate::error::ProcessError;
use crate::vcontrold::VcontroldClient;

/// Default config file path
const DEFAULT_CONFIG_PATH: &str = "/config/vcontrold.xml";

/// Readiness probe timeout in seconds
const READINESS_TIMEOUT_SECS: u64 = 30;

/// Interval between readiness probe attempts
const READINESS_PROBE_INTERVAL: Duration = Duration::from_secs(1);

/// vcontrold process manager
pub struct VcontroldProcess {
    child: Child,
}

impl VcontroldProcess {
    /// Spawn vcontrold with the given configuration
    ///
    /// - Normal: `vcontrold -n -x /config/vcontrold.xml`
    /// - Debug: `vcontrold -n -x /config/vcontrold.xml --verbose --debug`
    pub async fn spawn(config_path: Option<&Path>, debug_mode: bool) -> Result<Self, ProcessError> {
        let config_path = config_path
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| Path::new(DEFAULT_CONFIG_PATH).to_path_buf());

        // Check if config file exists
        if !config_path.exists() {
            return Err(ProcessError::ConfigNotFound(
                config_path.display().to_string(),
            ));
        }

        let mut cmd = Command::new("vcontrold");
        cmd.arg("-n") // Don't fork into background
            .arg("-x")
            .arg(&config_path);

        if debug_mode {
            cmd.arg("--verbose").arg("--debug");
        }

        // Capture stdout/stderr
        cmd.stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .stdin(Stdio::null());

        info!(
            "Starting vcontrold with config: {}{}",
            config_path.display(),
            if debug_mode { " (debug mode)" } else { "" }
        );

        let child = cmd
            .spawn()
            .map_err(|e| ProcessError::StartFailed(e.to_string()))?;

        info!("vcontrold started with PID {}", child.id().unwrap_or(0));

        Ok(Self { child })
    }

    /// Wait for vcontrold to be ready (TCP port responding)
    pub async fn wait_ready(&self) -> Result<(), ProcessError> {
        let client = VcontroldClient::localhost();
        let start = std::time::Instant::now();

        info!("Waiting for vcontrold to be ready...");

        while start.elapsed().as_secs() < READINESS_TIMEOUT_SECS {
            if client.is_ready().await {
                info!(
                    "vcontrold is ready after {} seconds",
                    start.elapsed().as_secs()
                );
                return Ok(());
            }
            sleep(READINESS_PROBE_INTERVAL).await;
        }

        Err(ProcessError::ReadinessTimeout(READINESS_TIMEOUT_SECS))
    }

    /// Check if the process is still running
    #[allow(dead_code)]
    pub fn is_running(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(Some(_)) => false, // Process has exited
            Ok(None) => true,     // Still running
            Err(_) => false,      // Error checking status
        }
    }

    /// Wait for the process to exit and return the exit code
    pub async fn wait(&mut self) -> Result<Option<i32>, ProcessError> {
        let status = self
            .child
            .wait()
            .await
            .map_err(|e| ProcessError::StartFailed(e.to_string()))?;

        Ok(status.code())
    }

    /// Kill the process
    #[allow(dead_code)]
    pub async fn kill(&mut self) {
        if let Err(e) = self.child.kill().await {
            warn!("Failed to kill vcontrold: {}", e);
        }
    }

    /// Get the process ID
    #[allow(dead_code)]
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }
}

/// Monitor task that watches vcontrold and signals if it exits
pub async fn monitor_process(mut process: VcontroldProcess) -> ProcessError {
    match process.wait().await {
        Ok(code) => {
            error!("vcontrold exited with code: {:?}", code);
            ProcessError::UnexpectedExit(code)
        }
        Err(e) => {
            error!("Error waiting for vcontrold: {}", e);
            e
        }
    }
}
