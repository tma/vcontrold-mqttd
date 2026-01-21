//! vcontrold-mqttd - MQTT bridge for vcontrold
//!
//! A containerized wrapper around vcontrold that provides:
//! - Serial communication with Viessmann heating systems via Optolink/FTDI USB adapter
//! - Automated periodic polling of heating controller parameters
//! - MQTT bridge for remote query and control
//! - JSON response formatting

mod config;
mod error;
mod mqtt;
mod polling;
mod process;
mod vcontrold;

use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::mqtt::{run_event_loop, run_subscriber, MqttClient};
use crate::polling::run_polling_loop;
use crate::process::VcontroldProcess;
use crate::vcontrold::VcontroldClient;

#[tokio::main]
async fn main() {
    // Initialize logging
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        if std::env::var("DEBUG")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false)
        {
            EnvFilter::new("debug")
        } else {
            EnvFilter::new("info")
        }
    });

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    if let Err(e) = run().await {
        error!("Fatal error: {}", e);
        std::process::exit(1);
    }
}

/// Wait for shutdown signal (SIGTERM or SIGINT)
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");
        tokio::select! {
            _ = sigterm.recv() => info!("Received SIGTERM"),
            _ = sigint.recv() => info!("Received SIGINT"),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
        info!("Received Ctrl+C");
    }
}

async fn run() -> Result<()> {
    // Load configuration
    let config = Config::from_env()?;

    if config.debug {
        info!("Debug mode enabled");
    }

    // Start vcontrold process
    let mut vcontrold_process = VcontroldProcess::spawn(None, config.debug).await?;

    // Wait for vcontrold to be ready
    vcontrold_process.wait_ready().await?;

    // Create vcontrold client
    let vcontrold_client = Arc::new(VcontroldClient::localhost());

    // Create MQTT client
    let publisher_client_id = config.publisher_client_id();
    let (mqtt_client, eventloop) = MqttClient::new(&config.mqtt, &publisher_client_id)?;
    let mqtt_client = Arc::new(mqtt_client);

    // Channel for subscriber messages (if enabled)
    let (message_tx, message_rx) = if config.mqtt_subscribe {
        let (tx, rx) = mpsc::channel(100);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    // Spawn MQTT event loop
    let eventloop_handle = tokio::spawn(run_event_loop(eventloop, message_tx));

    // Spawn polling loop (if commands are configured)
    let polling_handle = if !config.commands.is_empty() {
        let config_clone = config.clone();
        let vcontrold_clone = Arc::clone(&vcontrold_client);
        let mqtt_clone = Arc::clone(&mqtt_client);
        Some(tokio::spawn(async move {
            run_polling_loop(&config_clone, vcontrold_clone, mqtt_clone).await;
        }))
    } else {
        info!("No commands configured, polling disabled");
        None
    };

    // Spawn subscriber (if enabled)
    let subscriber_handle = if config.mqtt_subscribe {
        let mqtt_clone = Arc::clone(&mqtt_client);
        let vcontrold_clone = Arc::clone(&vcontrold_client);
        let rx = message_rx.unwrap();
        info!("Request/response bridge enabled");
        Some(tokio::spawn(async move {
            run_subscriber(mqtt_clone, vcontrold_clone, rx).await;
        }))
    } else {
        None
    };

    info!("vcontrold-mqttd started");

    // Wait for any task to complete or shutdown signal
    let exit_error = tokio::select! {
        result = vcontrold_process.wait() => {
            match result {
                Ok(code) => {
                    error!("vcontrold exited with code: {:?}", code);
                    Some(Error::Process(crate::error::ProcessError::UnexpectedExit(code)))
                }
                Err(e) => {
                    error!("Error waiting for vcontrold: {}", e);
                    Some(Error::Process(e))
                }
            }
        }
        _ = eventloop_handle => {
            error!("MQTT event loop exited unexpectedly");
            None
        }
        _ = async {
            if let Some(handle) = polling_handle {
                handle.await
            } else {
                std::future::pending::<()>().await;
                Ok(())
            }
        } => {
            error!("Polling loop exited unexpectedly");
            None
        }
        _ = async {
            if let Some(handle) = subscriber_handle {
                handle.await
            } else {
                std::future::pending::<()>().await;
                Ok(())
            }
        } => {
            error!("Subscriber exited unexpectedly");
            None
        }
        _ = shutdown_signal() => {
            None
        }
    };

    // Cleanup: kill vcontrold process
    info!("Shutting down vcontrold...");
    vcontrold_process.kill().await;

    // Disconnect TCP client
    vcontrold_client.disconnect().await;

    if let Some(e) = exit_error {
        return Err(e);
    }

    Ok(())
}
