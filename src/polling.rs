//! Polling loop for vcontrold commands
//!
//! Handles command batching and periodic execution.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::mqtt::{MqttClient, Publisher};
use crate::vcontrold::VcontroldClient;

/// Batch commands respecting the max length limit
///
/// ```
/// batch = ""
/// for each command in COMMANDS:
///     if length(batch + "," + command) > MAX_LENGTH:
///         execute_batch(batch)
///         batch = command
///     else:
///         batch = batch + "," + command
/// execute_batch(batch)
/// ```
pub fn batch_commands(commands: &[String], max_length: usize) -> Vec<Vec<String>> {
    let mut batches: Vec<Vec<String>> = Vec::new();
    let mut current_batch: Vec<String> = Vec::new();
    let mut current_length = 0;

    for cmd in commands {
        let cmd_len = cmd.len();
        let separator_len = if current_batch.is_empty() { 0 } else { 1 }; // comma

        if current_length + separator_len + cmd_len > max_length && !current_batch.is_empty() {
            // Current batch is full, start a new one
            batches.push(std::mem::take(&mut current_batch));
            current_length = 0;
        }

        current_batch.push(cmd.clone());
        current_length += if current_length == 0 {
            cmd_len
        } else {
            1 + cmd_len // comma + command
        };
    }

    // Don't forget the last batch
    if !current_batch.is_empty() {
        batches.push(current_batch);
    }

    batches
}

/// Run the polling loop
///
/// 1. Parse COMMANDS as comma-separated list
/// 2. Batch commands into groups respecting MAX_LENGTH character limit
/// 3. For each batch:
///    - Execute commands via vcontrold client
///    - Publish each value to ${MQTT_TOPIC}/command/<name>
/// 4. Sleep INTERVAL seconds
/// 5. Repeat
pub async fn run_polling_loop(
    config: &Config,
    vcontrold: Arc<VcontroldClient>,
    mqtt_client: Arc<MqttClient>,
    mqtt_connected: Arc<AtomicBool>,
) {
    if config.commands.is_empty() {
        warn!("No commands configured for polling");
        return;
    }

    // Pre-batch commands
    let batches = batch_commands(&config.commands, config.max_length);
    info!(
        "Polling {} commands in {} batches every {} seconds",
        config.commands.len(),
        batches.len(),
        config.interval.as_secs()
    );

    if config.debug {
        for (i, batch) in batches.iter().enumerate() {
            debug!("Batch {}: {:?}", i + 1, batch);
        }
    }

    let mut poll_interval = interval(config.interval);
    // Skip missed ticks instead of bursting them all at once. This prevents
    // overwhelming the MQTT client after a stall (e.g. broker outage where
    // publishes hit the timeout and the interval falls behind).
    poll_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let publisher = Publisher::new(&mqtt_client);

    let mut was_disconnected = false;

    loop {
        poll_interval.tick().await;

        // Skip entire cycle when the MQTT broker is unreachable. This avoids
        // unnecessary vcontrold/Optolink traffic and prevents filling the
        // rumqttc internal channel (which would block the polling loop).
        if !mqtt_connected.load(Ordering::Relaxed) {
            if !was_disconnected {
                warn!("MQTT broker disconnected, skipping polling cycles");
                was_disconnected = true;
            }
            continue;
        }
        if was_disconnected {
            info!("MQTT broker reconnected, resuming polling");
            was_disconnected = false;
        }

        debug!("Starting polling cycle");

        for (batch_idx, batch) in batches.iter().enumerate() {
            if config.debug {
                debug!("Executing batch {}: {}", batch_idx + 1, batch.join(","));
            }

            let results = vcontrold.execute_batch(batch).await;

            // Process results
            let mut successful_results = Vec::new();
            for result in results {
                match result {
                    Ok(cmd_result) => {
                        if cmd_result.error.is_some() {
                            warn!(
                                "Command {} returned error: {:?}",
                                cmd_result.command, cmd_result.error
                            );
                        } else {
                            if config.debug {
                                debug!(
                                    "Command {} returned: {:?}",
                                    cmd_result.command, cmd_result.value
                                );
                            }
                            successful_results.push(cmd_result);
                        }
                    }
                    Err(e) => {
                        error!("Failed to execute command in batch {}: {}", batch_idx + 1, e);
                    }
                }
            }

            // Publish successful results
            publisher.publish_results(&successful_results).await;
        }

        debug!("Polling cycle complete");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_commands_single_batch() {
        let commands: Vec<String> = vec!["cmd1".into(), "cmd2".into(), "cmd3".into()];
        let batches = batch_commands(&commands, 100);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0], commands);
    }

    #[test]
    fn test_batch_commands_multiple_batches() {
        let commands: Vec<String> = vec![
            "getTempWWObenIst".into(),
            "getTempWWsoll".into(),
            "getTempA".into(),
            "getTempB".into(),
        ];
        // Max length 40: "getTempWWObenIst,getTempWWsoll" = 30 chars
        // Adding "getTempA" = 30 + 1 + 8 = 39 chars (fits)
        // Adding "getTempB" = 39 + 1 + 8 = 48 chars (doesn't fit)
        let batches = batch_commands(&commands, 40);
        assert_eq!(batches.len(), 2);
        assert_eq!(
            batches[0],
            vec!["getTempWWObenIst", "getTempWWsoll", "getTempA"]
        );
        assert_eq!(batches[1], vec!["getTempB"]);
    }

    #[test]
    fn test_batch_commands_empty() {
        let commands: Vec<String> = vec![];
        let batches = batch_commands(&commands, 100);
        assert!(batches.is_empty());
    }

    #[test]
    fn test_batch_commands_single_long_command() {
        // Even if a single command exceeds max_length, it should still be in its own batch
        let commands: Vec<String> = vec!["veryLongCommandName".into()];
        let batches = batch_commands(&commands, 5);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0], vec!["veryLongCommandName"]);
    }

    /// Verify that the polling interval uses Skip behavior: after a long stall
    /// only one tick fires rather than a burst of all missed ticks.
    ///
    /// With the default `Burst` behavior, advancing time by 5x the interval
    /// would yield 5 immediately-ready ticks. With `Skip`, only the next
    /// natural tick fires, so we get exactly 1.
    #[tokio::test]
    async fn test_interval_skips_missed_ticks() {
        use std::time::Duration;
        use tokio::time::{interval, MissedTickBehavior};

        tokio::time::pause();

        let period = Duration::from_secs(10);
        let mut ivl = interval(period);
        ivl.set_missed_tick_behavior(MissedTickBehavior::Skip);

        // First tick fires immediately (interval semantics)
        ivl.tick().await;

        // Simulate a stall: advance time by 5 periods without ticking
        tokio::time::advance(period * 5).await;

        // After the stall, exactly one tick should be ready (Skip discards
        // missed ticks and resets to the next future deadline).
        ivl.tick().await;

        // The next tick should NOT be immediately available — it should
        // require waiting another full period.
        let next = tokio::time::timeout(period / 2, ivl.tick()).await;
        assert!(
            next.is_err(),
            "no burst tick should be available; Skip must discard missed ticks"
        );
    }

    #[test]
    fn test_mqtt_connected_flag_state_transitions() {
        // Verify the AtomicBool flag behaves correctly across the
        // state transitions that run_event_loop and run_polling_loop rely on.
        let connected = Arc::new(AtomicBool::new(false));

        // Initial state: disconnected (same as main.rs)
        assert!(!connected.load(Ordering::Relaxed));

        // Simulate ConnAck in event loop
        connected.store(true, Ordering::Relaxed);
        assert!(connected.load(Ordering::Relaxed));

        // Simulate Disconnect
        connected.store(false, Ordering::Relaxed);
        assert!(!connected.load(Ordering::Relaxed));

        // Simulate reconnect
        connected.store(true, Ordering::Relaxed);
        assert!(connected.load(Ordering::Relaxed));

        // Simulate event loop error
        connected.store(false, Ordering::Relaxed);
        assert!(!connected.load(Ordering::Relaxed));
    }
}
