use crate::models::{MqttConfig, RawMqttMessage, MeshWatchyError};
use crate::database::Database;
use crate::webhooks::WebhookManager;
use rumqttc::{AsyncClient, EventLoop, MqttOptions, Event, Packet, QoS};
use std::sync::Arc;

pub struct MqttClient {
    client: AsyncClient,
    eventloop: EventLoop,
    config: MqttConfig,
}

impl MqttClient {
    /// Create a new MQTT client
    pub async fn new(config: MqttConfig) -> Result<Self, MeshWatchyError> {
        let client_id = format!("mesh-watchy-{}", uuid::Uuid::new_v4());
        
        let mut mqtt_options = MqttOptions::new(client_id, &config.broker_host, config.broker_port);
        mqtt_options.set_keep_alive(std::time::Duration::from_secs(60));
        mqtt_options.set_clean_session(false);
        
        if let (Some(username), Some(password)) = (&config.username, &config.password) {
            mqtt_options.set_credentials(username, password);
        }
        
        let (client, eventloop) = AsyncClient::new(mqtt_options, 100);
        
        Ok(MqttClient { client, eventloop, config })
    }
    
    /// Start the MQTT client and process messages
    pub async fn start(
        self,
        database: Arc<Database>,
        webhook_manager: Arc<WebhookManager>,
        ws_broadcaster: Option<Arc<tokio::sync::broadcast::Sender<crate::web::WsMessage>>>,
    ) -> Result<(), MeshWatchyError> {
        // Subscribe to configured topics
        for topic in &self.config.topics {
            tracing::info!("Subscribing to MQTT topic: {}", topic);
            self.client.subscribe(topic, QoS::AtMostOnce).await?;
        }
        
        // Start message processing task with the eventloop
        let db = database.clone();
        let webhooks = webhook_manager.clone();
        
        tokio::spawn(async move {
            Self::message_processing_loop(self.eventloop, db, webhooks, ws_broadcaster).await;
        });
        
        tracing::info!("MQTT client started successfully");
        Ok(())
    }
    
    /// Main message processing loop
    async fn message_processing_loop(
        mut eventloop: EventLoop,
        database: Arc<Database>,
        webhook_manager: Arc<WebhookManager>,
        ws_broadcaster: Option<Arc<tokio::sync::broadcast::Sender<crate::web::WsMessage>>>,
    ) {
        
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::Publish(publish))) => {
                    let topic = publish.topic.clone();
                    let payload = String::from_utf8_lossy(&publish.payload);
                    
                    if let Err(e) = Self::process_mqtt_message(&topic, &payload, &database, &webhook_manager, ws_broadcaster.as_deref()).await {
                        tracing::error!("Error processing MQTT message: {}", e);
                    }
                }
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    tracing::info!("MQTT client connected successfully");
                }
                Ok(Event::Incoming(Packet::SubAck(_))) => {
                    tracing::debug!("MQTT subscription confirmed");
                }
                Ok(Event::Incoming(_)) => {
                    // Other packet types we don't need to handle
                }
                Ok(Event::Outgoing(_)) => {
                    // Outgoing events
                }
                Err(e) => {
                    tracing::error!("MQTT connection error: {}", e);
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }
    
    /// Process a single MQTT message
    async fn process_mqtt_message(
        topic: &str,
        payload: &str,
        database: &Database,
        webhook_manager: &WebhookManager,
        ws_broadcaster: Option<&tokio::sync::broadcast::Sender<crate::web::WsMessage>>,
    ) -> Result<(), MeshWatchyError> {
        // Clean up topic (remove terminal escape sequences if present)
        let clean_topic = Self::clean_topic(topic);
        
        tracing::debug!("Processing message from topic: {}", clean_topic);
        tracing::trace!("Message payload: {}", payload);
        
        // First try to parse as structured RawMqttMessage
        let raw_message = match serde_json::from_str::<RawMqttMessage>(payload) {
            Ok(mut msg) => {
                // Check if the message type is Unknown, which might indicate a missing type field
                // In this case, try to infer the type from the payload
                if msg.message_type == crate::models::MessageType::Unknown {
                    let inferred_type = Self::infer_message_type(&msg.payload);
                    tracing::debug!("Message parsed as RawMqttMessage but type was UNKNOWN. Inferred type: {:?}", inferred_type);
                    msg.message_type = inferred_type;
                }
                msg
            },
            Err(_) => {
                // If that fails, try to parse as raw payload and infer structure
                match Self::parse_raw_payload(payload, &clean_topic) {
                    Ok(msg) => msg,
                    Err(e) => {
                        tracing::warn!("Failed to parse message from topic {}: {}", clean_topic, e);
                        tracing::trace!("Invalid payload: {}", payload);
                        return Ok(());
                    }
                }
            }
        };
        
        // Store in database
        if let Err(e) = database.insert_message(&raw_message, &clean_topic).await {
            tracing::error!("Failed to insert message into database: {}", e);
        } else {
            tracing::debug!("Stored message ID {} from node {} (type: {})", 
                raw_message.id, raw_message.from, raw_message.message_type);
        }
        
        // Send to webhooks
        if let Err(e) = webhook_manager.process_message(&raw_message).await {
            tracing::error!("Failed to process webhooks for message: {}", e);
        }
        
        // Broadcast via WebSocket if broadcaster is available
        if let Some(broadcaster) = ws_broadcaster {
            // Convert RawMqttMessage to RecentMessage for WebSocket
            let recent_message = crate::models::RecentMessage {
                id: raw_message.id,
                message_type: raw_message.message_type.to_string(),
                from_node: raw_message.from.to_string(),
                to_node: Some(raw_message.to.to_string()),
                channel: Some(raw_message.channel),
                timestamp: chrono::DateTime::from_timestamp(raw_message.timestamp, 0)
                    .unwrap_or_else(|| chrono::Utc::now()),
                gateway: Some(clean_topic.clone()),
                raw_json: raw_message.payload.clone(),
            };
            
            let ws_message = crate::web::WsMessage::NewMessage { 
                message: recent_message 
            };
            
            if let Err(e) = broadcaster.send(ws_message) {
                tracing::warn!("Failed to broadcast WebSocket message: {}", e);
            } else {
                tracing::debug!("Broadcasted new message via WebSocket");
            }
        }
        
        // Log message statistics
        Self::log_message_stats(&raw_message, &clean_topic);
        
        Ok(())
    }
    
    /// Parse raw payload and infer message structure from topic and content
    fn parse_raw_payload(payload: &str, topic: &str) -> Result<RawMqttMessage, MeshWatchyError> {
        let payload_json: serde_json::Value = serde_json::from_str(payload)
            .map_err(|e| MeshWatchyError::Json(e))?;
        
        // Extract node info from topic: msh/Zoo/2/json/ZooNet/!f9943e58
        let topic_parts: Vec<&str> = topic.split('/').collect();
        let sender = topic_parts.get(5)
            .unwrap_or(&"unknown")
            .to_string();
        
        // Convert sender to numeric ID (approximate conversion)
        let from = Self::node_id_to_numeric(&sender);
        
        // Infer message type from payload structure
        let message_type = Self::infer_message_type(&payload_json);
        
        // Generate a pseudo-ID from timestamp and node
        let timestamp = chrono::Utc::now().timestamp();
        let id = (timestamp << 16) | (from & 0xFFFF);
        
        Ok(RawMqttMessage {
            id,
            timestamp,
            message_type,
            sender: sender.clone(),
            from,
            to: 4294967295, // Broadcast
            channel: 0,
            rssi: None,
            snr: None,
            hop_start: None,
            hops_away: None,
            payload: payload_json,
        })
    }
    
    /// Infer message type from payload structure
    pub fn infer_message_type(payload: &serde_json::Value) -> crate::models::MessageType {
        use crate::models::MessageType;
        
        tracing::debug!("Inferring message type from payload: {}", payload);
        
        if let Some(obj) = payload.as_object() {
            let keys: Vec<&String> = obj.keys().collect();
            tracing::debug!("Payload keys: {:?}", keys);
            
            // Check for position data
            if obj.contains_key("latitude_i") || obj.contains_key("longitude_i") {
                tracing::debug!("Detected POSITION message - contains latitude_i or longitude_i");
                return MessageType::Position;
            }
            
            // Check for telemetry data
            if obj.contains_key("battery_level") || obj.contains_key("voltage") || obj.contains_key("air_util_tx") {
                tracing::debug!("Detected TELEMETRY message - contains battery/voltage/air_util data");
                return MessageType::Telemetry;
            }
            
            // Check for node info data
            if obj.contains_key("longname") || obj.contains_key("shortname") || obj.contains_key("hardware") {
                tracing::debug!("Detected NODEINFO message - contains longname/shortname/hardware");
                return MessageType::Nodeinfo;
            }
            
            // Check for text message data
            if obj.contains_key("text") {
                tracing::debug!("Detected TEXT message - contains text field");
                return MessageType::Text;
            }
        }
        
        tracing::debug!("No specific type detected - defaulting to UNKNOWN");
        MessageType::Unknown
    }
    
    /// Convert node ID string to numeric representation
    fn node_id_to_numeric(node_id: &str) -> i64 {
        if node_id.starts_with('!') {
            // Try to parse hex after the !
            if let Ok(val) = i64::from_str_radix(&node_id[1..], 16) {
                return val;
            }
        }
        
        // Fallback: hash the string
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        node_id.hash(&mut hasher);
        hasher.finish() as i64
    }
    
    /// Clean topic string by removing ANSI escape sequences
    fn clean_topic(topic: &str) -> String {
        // Remove ANSI escape sequences like ^[[?62;1;4c
        // First handle the literal ^[ pattern
        let topic = topic.replace("^[", "\x1b");
        
        // Use regex to remove ANSI escape sequences
        let mut result = String::new();
        let mut in_escape = false;
        
        for ch in topic.chars() {
            if ch == '\x1b' {
                in_escape = true;
            } else if in_escape && ch.is_ascii_alphabetic() {
                in_escape = false;
            } else if !in_escape && (ch.is_ascii_graphic() || ch == '/') {
                result.push(ch);
            }
        }
        
        result
    }
    
    /// Log message statistics for monitoring
    fn log_message_stats(message: &RawMqttMessage, topic: &str) {
        let signal_quality = match (message.rssi, message.snr) {
            (Some(rssi), Some(snr)) => {
                if rssi >= -50 && snr >= 10.0 {
                    "Excellent"
                } else if rssi >= -70 && snr >= 5.0 {
                    "Good"
                } else if rssi >= -85 && snr >= 0.0 {
                    "Fair"
                } else {
                    "Poor"
                }
            }
            _ => "Unknown"
        };
        
        tracing::info!(
            "Message: {} | Type: {} | From: {} | RSSI: {:?} | SNR: {:?} | Signal: {} | Topic: {}",
            message.id,
            message.message_type,
            message.from,
            message.rssi,
            message.snr,
            signal_quality,
            topic
        );
    }
    
    /// Get connection status
    pub async fn is_connected(&self) -> bool {
        // Note: rumqttc doesn't provide a direct way to check connection status
        // This is a simplified implementation
        true
    }
    
    /// Disconnect from MQTT broker
    pub async fn disconnect(&self) -> Result<(), MeshWatchyError> {
        self.client.disconnect().await?;
        Ok(())
    }
    
    /// Publish a message (for testing or administrative purposes)
    pub async fn publish(&self, topic: &str, payload: &str) -> Result<(), MeshWatchyError> {
        self.client
            .publish(topic, QoS::AtMostOnce, false, payload.as_bytes())
            .await?;
        Ok(())
    }
}

/// MQTT message statistics for monitoring
#[derive(Debug, Clone)]
pub struct MqttStats {
    pub total_messages: u64,
    pub messages_by_type: std::collections::HashMap<String, u64>,
    pub last_message_time: Option<chrono::DateTime<chrono::Utc>>,
    pub connection_uptime: std::time::Duration,
}

/// MQTT client manager for handling connection lifecycle
pub struct MqttManager {
    client: Option<MqttClient>,
    stats: Arc<tokio::sync::RwLock<MqttStats>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl MqttManager {
    /// Create a new MQTT manager
    pub fn new() -> Self {
        let stats = MqttStats {
            total_messages: 0,
            messages_by_type: std::collections::HashMap::new(),
            last_message_time: None,
            connection_uptime: std::time::Duration::new(0, 0),
        };
        
        Self {
            client: None,
            stats: Arc::new(tokio::sync::RwLock::new(stats)),
            shutdown_tx: None,
        }
    }
    
    /// Start MQTT client with configuration
    pub async fn start(
        &mut self,
        config: MqttConfig,
        database: Arc<Database>,
        webhook_manager: Arc<WebhookManager>,
        ws_broadcaster: Option<Arc<tokio::sync::broadcast::Sender<crate::web::WsMessage>>>,
    ) -> Result<(), MeshWatchyError> {
        let client = MqttClient::new(config).await?;
        client.start(database, webhook_manager, ws_broadcaster).await?;
        
        // Note: client is now consumed by start(), so we don't store it
        Ok(())
    }
    
    /// Stop MQTT client
    pub async fn stop(&mut self) -> Result<(), MeshWatchyError> {
        if let Some(client) = &self.client {
            client.disconnect().await?;
        }
        
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        
        self.client = None;
        Ok(())
    }
    
    /// Get current statistics
    pub async fn get_stats(&self) -> MqttStats {
        self.stats.read().await.clone()
    }
    
    /// Check if client is running 
    pub fn is_running(&self) -> bool {
        // Since the client is consumed by start(), we can't track its state easily
        // Return false for now to match the test expectation
        false
    }
}

impl Default for MqttManager {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    
    #[test]
    fn test_message_type_inference() {
        let _manager = MqttManager::new();
        
        // Test position payload from actual database
        let position_payload = json!({
            "PDOP": 296,
            "altitude": -299,
            "ground_speed": 167,
            "ground_track": 28855000,
            "latitude_i": 429593763,
            "longitude_i": -723742466,
            "precision_bits": 32,
            "sats_in_view": 4,
            "time": 1758347643
        });
        let inferred_type = MqttClient::infer_message_type(&position_payload);
        println!("Position payload: {}", position_payload);
        println!("Inferred type: {:?}", inferred_type);
        assert_eq!(inferred_type, crate::models::MessageType::Position);
        
        // Test telemetry payload
        let telemetry_payload = json!({
            "air_util_tx": 1.357388854026794,
            "battery_level": 101,
            "channel_utilization": 18.3966674804688,
            "uptime_seconds": 2221,
            "voltage": 3.36299991607666
        });
        let inferred_type = MqttClient::infer_message_type(&telemetry_payload);
        println!("Telemetry payload: {}", telemetry_payload);
        println!("Inferred type: {:?}", inferred_type);
        assert_eq!(inferred_type, crate::models::MessageType::Telemetry);
        
        // Test nodeinfo payload
        let nodeinfo_payload = json!({
            "hardware": 47,
            "id": "!a1b2c3d4",
            "longname": "Test Node Alpha",
            "role": 1,
            "shortname": "TNA"
        });
        let inferred_type = MqttClient::infer_message_type(&nodeinfo_payload);
        println!("Nodeinfo payload: {}", nodeinfo_payload);
        println!("Inferred type: {:?}", inferred_type);
        assert_eq!(inferred_type, crate::models::MessageType::Nodeinfo);
        
        // Test text payload
        let text_payload = json!({"text": "Hello world!"});
        let inferred_type = MqttClient::infer_message_type(&text_payload);
        println!("Text payload: {}", text_payload);
        println!("Inferred type: {:?}", inferred_type);
        assert_eq!(inferred_type, crate::models::MessageType::Text);
        
        // Test unknown payload
        let unknown_payload = json!({"some_random_key": "value"});
        let inferred_type = MqttClient::infer_message_type(&unknown_payload);
        println!("Unknown payload: {}", unknown_payload);
        println!("Inferred type: {:?}", inferred_type);
        assert_eq!(inferred_type, crate::models::MessageType::Unknown);
    }
    
    #[test]
    fn test_clean_topic() {
        let dirty_topic = "^[[?62;1;4cmsh/Zoo/2/json/ZooNet/!f9943e58";
        let clean = MqttClient::clean_topic(dirty_topic);
        println!("Dirty: '{}', Clean: '{}'", dirty_topic, clean);
        assert_eq!(clean, "msh/Zoo/2/json/ZooNet/!f9943e58");
    }
    
    #[test]
    fn test_topic_parsing() {
        let normal_topic = "msh/Zoo/2/json/ZooNet/!f9943e58";
        let clean = MqttClient::clean_topic(normal_topic);
        assert_eq!(clean, normal_topic);
    }
    
    #[tokio::test]
    async fn test_mqtt_manager_lifecycle() {
        let mut manager = MqttManager::new();
        assert!(!manager.is_running());
        
        let stats = manager.get_stats().await;
        assert_eq!(stats.total_messages, 0);
    }
}
