//! MQTT subscriber for request/response bridge
//!
//! Handles incoming MQTT requests and forwards them to vcontrold.

use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use crate::vcontrold::{build_json_response, VcontroldClient};

use super::client::{IncomingMessage, MqttClient};

/// Request topic suffix
const REQUEST_SUFFIX: &str = "request";
/// Response topic suffix
const RESPONSE_SUFFIX: &str = "response";

/// Subscriber for request/response bridge
pub struct Subscriber {
    base_topic: String,
}

impl Subscriber {
    /// Create a new subscriber
    pub fn new(base_topic: &str) -> Self {
        Self {
            base_topic: base_topic.to_string(),
        }
    }

    /// Get the request topic
    pub fn request_topic(&self) -> String {
        format!("{}/{}", self.base_topic, REQUEST_SUFFIX)
    }

    /// Get the response topic
    pub fn response_topic(&self) -> String {
        format!("{}/{}", self.base_topic, RESPONSE_SUFFIX)
    }

    /// Check if a message is a request
    pub fn is_request(&self, topic: &str) -> bool {
        topic == self.request_topic()
    }

    /// Parse commands from request payload
    ///
    /// Formats:
    /// - Single command: "getTempWWObenIst"
    /// - Multiple commands: "getTempWWObenIst,getTempWWsoll"
    /// - Write command: "setTempWWsoll 50"
    /// - Mixed: "set1xWW 2,setTempWWsoll 50,getTempA"
    pub fn parse_commands(payload: &str) -> Vec<String> {
        payload
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

/// Run the subscriber task
///
/// Listens for incoming MQTT messages, executes commands on vcontrold,
/// and publishes responses.
pub async fn run_subscriber(
    subscriber: Subscriber,
    mqtt_client: Arc<MqttClient>,
    vcontrold: Arc<VcontroldClient>,
    mut message_rx: mpsc::Receiver<IncomingMessage>,
) {
    let request_topic = subscriber.request_topic();
    let response_topic = subscriber.response_topic();

    info!("Subscriber ready, listening on {}", request_topic);

    while let Some(msg) = message_rx.recv().await {
        // Only process messages on the request topic
        if !subscriber.is_request(&msg.topic) {
            continue;
        }

        // Skip empty payloads
        if msg.payload.trim().is_empty() {
            debug!("Skipping empty request payload");
            continue;
        }

        debug!("Received request: {}", msg.payload);

        // Parse commands
        let commands = Subscriber::parse_commands(&msg.payload);
        if commands.is_empty() {
            warn!("No valid commands in request");
            continue;
        }

        // Execute commands
        let results = vcontrold.execute_batch(&commands).await;

        // Build response
        let successful_results: Vec<_> = results
            .into_iter()
            .filter_map(|r| r.ok())
            .collect();

        if successful_results.is_empty() {
            warn!("All commands failed");
            continue;
        }

        let json_response = build_json_response(&successful_results);
        debug!("Sending response: {}", json_response);

        // Publish response (not retained: this is a point-in-time response
        // to a specific request, not a persistent state value)
        if let Err(e) = mqtt_client.publish(&response_topic, &json_response).await {
            error!("Failed to publish response: {}", e);
        }
    }

    warn!("Subscriber message channel closed");
}
