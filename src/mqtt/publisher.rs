//! MQTT publisher for polling results
//!
//! Publishes vcontrold command results to MQTT topics.

use tracing::{debug, error, warn};

use crate::error::MqttError;
use crate::vcontrold::{CommandResult, Value};

use super::client::MqttClient;

/// Publisher for vcontrold polling results
pub struct Publisher<'a> {
    client: &'a MqttClient,
}

impl<'a> Publisher<'a> {
    /// Create a new publisher
    pub fn new(client: &'a MqttClient) -> Self {
        Self { client }
    }

    /// Publish a single command result
    ///
    /// Topic: {base_topic}/command/{command_name}
    /// Payload: numeric or string value only
    /// Retained: yes
    pub async fn publish_result(&self, result: &CommandResult) -> Result<(), MqttError> {
        // Skip if there was an error
        if result.error.is_some() {
            warn!(
                "Skipping publish for {} due to error: {:?}",
                result.command, result.error
            );
            return Ok(());
        }

        // Skip if value is None
        let payload = match &result.value {
            Value::Number(n) => format_number(*n),
            Value::String(s) => s.clone(),
            Value::None => {
                debug!("Skipping publish for {} - no value", result.command);
                return Ok(());
            }
        };

        let topic = self.client.topic(&format!("command/{}", result.command));
        debug!("Publishing to {}: {}", topic, payload);

        self.client.publish_retained(&topic, &payload).await
    }

    /// Publish multiple command results
    pub async fn publish_results(&self, results: &[CommandResult]) {
        for result in results {
            if let Err(e) = self.publish_result(result).await {
                error!("Failed to publish {}: {}", result.command, e);
            }
        }
    }
}

/// Format a number for MQTT payload
///
/// Outputs integers without decimal places, floats with minimal precision
fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < i64::MAX as f64 {
        format!("{}", n as i64)
    } else {
        // Remove trailing zeros
        let s = format!("{:.6}", n);
        let s = s.trim_end_matches('0');
        let s = s.trim_end_matches('.');
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_number_integer() {
        assert_eq!(format_number(42.0), "42");
        assert_eq!(format_number(-10.0), "-10");
        assert_eq!(format_number(0.0), "0");
    }

    #[test]
    fn test_format_number_float() {
        assert_eq!(format_number(48.1), "48.1");
        assert_eq!(format_number(3.14159), "3.14159");
        assert_eq!(format_number(0.5), "0.5");
    }
}
