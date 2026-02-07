//! MQTT client wrapper for rumqttc
//!
//! Provides a simplified interface for MQTT v5 operations with TLS support.

use rumqttc::v5::mqttbytes::QoS;
use rumqttc::v5::{AsyncClient, Event, EventLoop, MqttOptions};
use rumqttc::Transport;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::ClientConfig;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::{MqttConfig, TlsConfig};
use crate::error::MqttError;

/// Message received from MQTT subscription
#[derive(Debug, Clone)]
pub struct IncomingMessage {
    pub topic: String,
    pub payload: String,
}

/// MQTT client wrapper
pub struct MqttClient {
    client: AsyncClient,
    base_topic: String,
}

impl MqttClient {
    /// Create a new MQTT client from configuration
    pub fn new(config: &MqttConfig, client_id: &str) -> Result<(Self, EventLoop), MqttError> {
        let mut options = MqttOptions::new(client_id, &config.host, config.port);
        options.set_keep_alive(Duration::from_secs(30));

        // Set credentials if provided
        if let (Some(user), Some(pass)) = (&config.user, &config.password) {
            options.set_credentials(user, pass);
        }

        // Configure TLS if enabled
        if let Some(tls_config) = &config.tls {
            let transport = build_tls_transport(&config.host, tls_config)?;
            options.set_transport(transport);
            info!("MQTT TLS enabled");
        }

        let (client, eventloop) = AsyncClient::new(options, 100);

        Ok((
            Self {
                client,
                base_topic: config.topic.clone(),
            },
            eventloop,
        ))
    }

    /// Get the base topic
    pub fn base_topic(&self) -> &str {
        &self.base_topic
    }

    /// Build a full topic path
    pub fn topic(&self, suffix: &str) -> String {
        format!("{}/{}", self.base_topic, suffix)
    }

    /// Publish a message with retain flag
    pub async fn publish_retained(&self, topic: &str, payload: &str) -> Result<(), MqttError> {
        debug!("Publishing to {}: {}", topic, payload);
        self.client
            .publish(topic, QoS::AtLeastOnce, true, payload.as_bytes().to_vec())
            .await
            .map_err(|e| MqttError::PublishFailed(e.to_string()))
    }

    /// Publish a message without retain flag
    #[allow(dead_code)]
    pub async fn publish(&self, topic: &str, payload: &str) -> Result<(), MqttError> {
        debug!("Publishing to {}: {}", topic, payload);
        self.client
            .publish(topic, QoS::AtLeastOnce, false, payload.as_bytes().to_vec())
            .await
            .map_err(|e| MqttError::PublishFailed(e.to_string()))
    }

    /// Get a clone of the underlying client (for use in multiple tasks)
    pub fn clone_client(&self) -> AsyncClient {
        self.client.clone()
    }
}

/// Build TLS transport configuration
fn build_tls_transport(host: &str, config: &TlsConfig) -> Result<Transport, MqttError> {
    let mut root_cert_store = rustls::RootCertStore::empty();

    // Load CA certificates
    if let Some(ca_file) = &config.ca_file {
        let certs = load_certs(ca_file)?;
        for cert in certs {
            root_cert_store
                .add(cert)
                .map_err(|e| MqttError::ConnectionFailed(format!("Failed to add CA cert: {}", e)))?;
        }
    } else if let Some(ca_path) = &config.ca_path {
        // Load all .crt and .pem files from directory
        if let Ok(entries) = std::fs::read_dir(ca_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "crt" || ext == "pem") {
                    if let Ok(certs) = load_certs(&path) {
                        for cert in certs {
                            let _ = root_cert_store.add(cert);
                        }
                    }
                }
            }
        }
    } else {
        // Use webpki roots as default
        root_cert_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }

    // Build client config
    let builder = ClientConfig::builder().with_root_certificates(root_cert_store);

    let tls_config = if let (Some(cert_file), Some(key_file)) =
        (&config.cert_file, &config.key_file)
    {
        // Client certificate authentication
        let certs = load_certs(cert_file)?;
        let key = load_private_key(key_file)?;
        builder
            .with_client_auth_cert(certs, key)
            .map_err(|e| MqttError::ConnectionFailed(format!("Failed to set client cert: {}", e)))?
    } else {
        // No client certificate
        builder.with_no_client_auth()
    };

    // Create rustls ClientConfig with dangerous verifier if insecure mode
    let tls_config = if config.insecure {
        warn!("TLS certificate validation disabled (insecure mode)");
        // For insecure mode, we need to rebuild with a custom verifier
        let mut dangerous_config = tls_config.clone();
        dangerous_config
            .dangerous()
            .set_certificate_verifier(Arc::new(InsecureServerCertVerifier));
        dangerous_config
    } else {
        tls_config
    };

    // Parse server name for SNI (validated but not used directly - rumqttc handles SNI)
    let _server_name: ServerName<'static> = host
        .to_string()
        .try_into()
        .map_err(|_| MqttError::ConnectionFailed(format!("Invalid server name: {}", host)))?;

    Ok(Transport::tls_with_config(rumqttc::TlsConfiguration::Rustls(Arc::new(tls_config))))
}

/// Load certificates from a PEM file
fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, MqttError> {
    let file = File::open(path)
        .map_err(|e| MqttError::ConnectionFailed(format!("Failed to open cert file: {}", e)))?;
    let mut reader = BufReader::new(file);
    let certs: Vec<_> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| MqttError::ConnectionFailed(format!("Failed to parse certs: {}", e)))?;
    Ok(certs)
}

/// Load a private key from a PEM file
fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>, MqttError> {
    let file = File::open(path)
        .map_err(|e| MqttError::ConnectionFailed(format!("Failed to open key file: {}", e)))?;
    let mut reader = BufReader::new(file);

    // Try to read PKCS#8 private key first, then RSA, then EC
    loop {
        match rustls_pemfile::read_one(&mut reader) {
            Ok(Some(rustls_pemfile::Item::Pkcs1Key(key))) => {
                return Ok(PrivateKeyDer::Pkcs1(key));
            }
            Ok(Some(rustls_pemfile::Item::Pkcs8Key(key))) => {
                return Ok(PrivateKeyDer::Pkcs8(key));
            }
            Ok(Some(rustls_pemfile::Item::Sec1Key(key))) => {
                return Ok(PrivateKeyDer::Sec1(key));
            }
            Ok(Some(_)) => continue, // Skip other items (certs, etc.)
            Ok(None) => break,
            Err(e) => {
                return Err(MqttError::ConnectionFailed(format!(
                    "Failed to parse private key: {}",
                    e
                )))
            }
        }
    }

    Err(MqttError::ConnectionFailed(
        "No private key found in file".to_string(),
    ))
}

/// Insecure server certificate verifier (for testing/development)
#[derive(Debug)]
struct InsecureServerCertVerifier;

impl rustls::client::danger::ServerCertVerifier for InsecureServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

/// Run the MQTT event loop and forward incoming messages
///
/// Re-subscribes to all topics on every ConnAck (reconnection), since
/// rumqttc uses `clean_start = true` by default and the broker discards
/// session state (including subscriptions) when the client reconnects.
pub async fn run_event_loop(
    mut eventloop: EventLoop,
    client: AsyncClient,
    subscribe_topics: Vec<String>,
    message_tx: Option<mpsc::Sender<IncomingMessage>>,
) {
    loop {
        match eventloop.poll().await {
            Ok(event) => {
                if let Event::Incoming(incoming) = event {
                    match incoming {
                        rumqttc::v5::Incoming::Publish(publish) => {
                            let topic = String::from_utf8_lossy(&publish.topic).to_string();
                            let payload = String::from_utf8_lossy(&publish.payload).to_string();
                            debug!("Received message on {}: {}", topic, payload);

                            if let Some(tx) = &message_tx {
                                let msg = IncomingMessage { topic, payload };
                                if tx.send(msg).await.is_err() {
                                    warn!("Failed to forward incoming message - receiver dropped");
                                }
                            }
                        }
                        rumqttc::v5::Incoming::ConnAck(_) => {
                            info!("Connected to MQTT broker");

                            // Re-subscribe to all topics on every (re)connection
                            for topic in &subscribe_topics {
                                info!("Subscribing to {}", topic);
                                if let Err(e) = client.subscribe(topic, QoS::AtLeastOnce).await {
                                    error!("Failed to subscribe to {}: {}", topic, e);
                                }
                            }
                        }
                        rumqttc::v5::Incoming::SubAck(_) => {
                            debug!("Subscription acknowledged");
                        }
                        rumqttc::v5::Incoming::PubAck(_) => {
                            // Normal acknowledgment, no action needed
                        }
                        rumqttc::v5::Incoming::Disconnect(_) => {
                            warn!("Disconnected from MQTT broker");
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                error!("MQTT event loop error: {}", e);
                // Wait before retrying
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    }
}
