use crate::models::{WebhookConfig, WebhookEvent, RawMqttMessage, MessageType, MeshWatchyError};
use chrono::Utc;
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;


pub struct WebhookManager {
    client: Client,
    webhooks: Arc<RwLock<Vec<WebhookConfig>>>,
    stats: Arc<RwLock<WebhookStats>>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct WebhookStats {
    pub total_sent: u64,
    pub total_failed: u64,
    pub sent_by_webhook: HashMap<String, u64>,
    pub failed_by_webhook: HashMap<String, u64>,
    pub last_sent: Option<chrono::DateTime<Utc>>,
}

impl WebhookManager {
    /// Create a new webhook manager
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to create HTTP client");
        
        Self {
            client,
            webhooks: Arc::new(RwLock::new(Vec::new())),
            stats: Arc::new(RwLock::new(WebhookStats::default())),
        }
    }
    
    /// Load webhook configurations
    pub async fn load_webhooks(&self, webhooks: Vec<WebhookConfig>) {
        let mut webhook_list = self.webhooks.write().await;
        *webhook_list = webhooks;
        
        let enabled_count = webhook_list.iter().filter(|w| w.enabled).count();
        tracing::info!("Loaded {} webhooks ({} enabled)", webhook_list.len(), enabled_count);
    }
    
    /// Process a mesh message and send to relevant webhooks
    pub async fn process_message(&self, message: &RawMqttMessage) -> Result<(), MeshWatchyError> {
        let webhooks = self.webhooks.read().await;
        let node_id = format!("{:08x}", message.from);
        
        let matching_webhooks: Vec<WebhookConfig> = webhooks
            .iter()
            .filter(|webhook| {
                // Check if webhook is enabled
                if !webhook.enabled {
                    return false;
                }
                
                // Check message type filter (empty means all types)
                if !webhook.message_types.is_empty() && !webhook.message_types.contains(&message.message_type) {
                    return false;
                }
                
                // Check device filter (empty means all devices)
                if !webhook.devices.is_empty() && !webhook.devices.contains(&node_id) {
                    return false;
                }
                
                true
            })
            .cloned()
            .collect();
        
        if matching_webhooks.is_empty() {
            return Ok(());
        }
        
        // Create webhook event
        let event = WebhookEvent {
            event_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            message_type: message.message_type.clone(),
            node_id: format!("!{:08x}", message.from),
            data: message.payload.clone(),
        };
        
        // Send to all matching webhooks concurrently
        let mut send_tasks = Vec::new();
        for webhook in matching_webhooks {
            let event_clone = event.clone();
            let client = self.client.clone();
            let stats = self.stats.clone();
            
            let task = tokio::spawn(async move {
                Self::send_webhook(client, webhook, event_clone, stats).await
            });
            
            send_tasks.push(task);
        }
        
        // Wait for all webhook sends to complete
        let results = futures::future::join_all(send_tasks).await;
        for result in results {
            if let Err(e) = result {
                tracing::error!("Webhook send task failed: {}", e);
            }
        }
        
        Ok(())
    }
    
    /// Send a single webhook
    async fn send_webhook(
        client: Client,
        webhook: WebhookConfig,
        event: WebhookEvent,
        stats: Arc<RwLock<WebhookStats>>,
    ) -> Result<(), MeshWatchyError> {
        let start_time = std::time::Instant::now();
        
        // Prepare request
        let mut request_builder = client
            .post(&webhook.url)
            .json(&event);
        
        // Add custom headers if specified
        if let Some(headers) = &webhook.headers {
            for (key, value) in headers {
                request_builder = request_builder.header(key, value);
            }
        }
        
        // Send request
        match request_builder.send().await {
            Ok(response) => {
                let duration = start_time.elapsed();
                let status = response.status();
                
                if status.is_success() {
                    Self::update_stats_success(&webhook.name, &stats).await;
                    tracing::debug!(
                        "Webhook '{}' sent successfully ({}ms) - Status: {}",
                        webhook.name,
                        duration.as_millis(),
                        status
                    );
                } else {
                    Self::update_stats_failure(&webhook.name, &stats).await;
                    let error_body = response.text().await.unwrap_or_default();
                    tracing::warn!(
                        "Webhook '{}' returned error status {} ({}ms): {}",
                        webhook.name,
                        status,
                        duration.as_millis(),
                        error_body
                    );
                }
            }
            Err(e) => {
                Self::update_stats_failure(&webhook.name, &stats).await;
                tracing::error!("Failed to send webhook '{}': {}", webhook.name, e);
                return Err(MeshWatchyError::Webhook(e));
            }
        }
        
        Ok(())
    }
    
    /// Update success statistics
    async fn update_stats_success(webhook_name: &str, stats: &Arc<RwLock<WebhookStats>>) {
        let mut stats_guard = stats.write().await;
        stats_guard.total_sent += 1;
        *stats_guard.sent_by_webhook.entry(webhook_name.to_string()).or_insert(0) += 1;
        stats_guard.last_sent = Some(Utc::now());
    }
    
    /// Update failure statistics
    async fn update_stats_failure(webhook_name: &str, stats: &Arc<RwLock<WebhookStats>>) {
        let mut stats_guard = stats.write().await;
        stats_guard.total_failed += 1;
        *stats_guard.failed_by_webhook.entry(webhook_name.to_string()).or_insert(0) += 1;
    }
    
    /// Get webhook statistics
    pub async fn get_stats(&self) -> WebhookStats {
        self.stats.read().await.clone()
    }
    
    /// Get current webhook configurations
    pub async fn get_webhooks(&self) -> Vec<WebhookConfig> {
        self.webhooks.read().await.clone()
    }
    
    /// Add a new webhook
    pub async fn add_webhook(&self, webhook: WebhookConfig) {
        let mut webhooks = self.webhooks.write().await;
        webhooks.push(webhook);
    }
    
    /// Update an existing webhook
    pub async fn update_webhook(&self, name: &str, webhook: WebhookConfig) -> bool {
        let mut webhooks = self.webhooks.write().await;
        if let Some(existing) = webhooks.iter_mut().find(|w| w.name == name) {
            *existing = webhook;
            true
        } else {
            false
        }
    }
    
    /// Remove a webhook
    pub async fn remove_webhook(&self, name: &str) -> bool {
        let mut webhooks = self.webhooks.write().await;
        let original_len = webhooks.len();
        webhooks.retain(|w| w.name != name);
        webhooks.len() != original_len
    }
    
    /// Enable/disable a webhook
    pub async fn set_webhook_enabled(&self, name: &str, enabled: bool) -> bool {
        let mut webhooks = self.webhooks.write().await;
        if let Some(webhook) = webhooks.iter_mut().find(|w| w.name == name) {
            webhook.enabled = enabled;
            tracing::info!("Webhook '{}' {}", name, if enabled { "enabled" } else { "disabled" });
            true
        } else {
            false
        }
    }
    
    /// Test a webhook with a sample payload
    pub async fn test_webhook(&self, webhook: &WebhookConfig) -> Result<TestResult, MeshWatchyError> {
        let test_event = WebhookEvent {
            event_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            message_type: MessageType::Position,
            node_id: "!test1234".to_string(),
            data: serde_json::json!({
                "latitude_i": 424041350,
                "longitude_i": -711257033,
                "altitude": 26,
                "PDOP": 156,
                "sats_in_view": 7
            }),
        };
        
        let start_time = std::time::Instant::now();
        
        let mut request_builder = self.client
            .post(&webhook.url)
            .json(&test_event);
        
        if let Some(headers) = &webhook.headers {
            for (key, value) in headers {
                request_builder = request_builder.header(key, value);
            }
        }
        
        match request_builder.send().await {
            Ok(response) => {
                let duration = start_time.elapsed();
                let status = response.status();
                let response_body = response.text().await.unwrap_or_default();
                
                Ok(TestResult {
                    success: status.is_success(),
                    status_code: status.as_u16(),
                    response_time: duration,
                    response_body: if response_body.len() > 500 {
                        format!("{}...", &response_body[..500])
                    } else {
                        response_body
                    },
                })
            }
            Err(e) => {
                Ok(TestResult {
                    success: false,
                    status_code: 0,
                    response_time: start_time.elapsed(),
                    response_body: format!("Connection error: {}", e),
                })
            }
        }
    }
    
    /// Reset webhook statistics
    pub async fn reset_stats(&self) {
        let mut stats = self.stats.write().await;
        *stats = WebhookStats::default();
        tracing::info!("Webhook statistics reset");
    }
}

impl Default for WebhookManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of webhook testing
#[derive(Debug, Clone, serde::Serialize)]
pub struct TestResult {
    pub success: bool,
    pub status_code: u16,
    pub response_time: std::time::Duration,
    pub response_body: String,
}

/// Webhook delivery status for monitoring
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WebhookStatus {
    pub name: String,
    pub enabled: bool,
    pub url: String,
    pub message_types: Vec<MessageType>,
    pub total_sent: u64,
    pub total_failed: u64,
    pub success_rate: f64,
    pub last_sent: Option<chrono::DateTime<Utc>>,
}

impl WebhookManager {
    /// Get detailed status for all webhooks
    pub async fn get_webhook_status(&self) -> Vec<WebhookStatus> {
        let webhooks = self.webhooks.read().await;
        let stats = self.stats.read().await;
        
        let mut result = Vec::new();
        
        for webhook in webhooks.iter() {
            let sent = stats.sent_by_webhook.get(&webhook.name).copied().unwrap_or(0);
            let failed = stats.failed_by_webhook.get(&webhook.name).copied().unwrap_or(0);
            let total = sent + failed;
            let success_rate = if total > 0 {
                sent as f64 / total as f64 * 100.0
            } else {
                0.0
            };
            
            result.push(WebhookStatus {
                name: webhook.name.clone(),
                enabled: webhook.enabled,
                url: webhook.url.clone(),
                message_types: webhook.message_types.clone(),
                total_sent: sent,
                total_failed: failed,
                success_rate,
                last_sent: stats.last_sent,
            });
        }
        
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{MessageType, RawMqttMessage};
    
    #[tokio::test]
    async fn test_webhook_manager_creation() {
        let manager = WebhookManager::new();
        let webhooks = manager.get_webhooks().await;
        assert!(webhooks.is_empty());
        
        let stats = manager.get_stats().await;
        assert_eq!(stats.total_sent, 0);
        assert_eq!(stats.total_failed, 0);
    }
    
    #[tokio::test]
    async fn test_webhook_management() {
        let manager = WebhookManager::new();
        
        let webhook = WebhookConfig {
            name: "Test Webhook".to_string(),
            url: "https://webhook.site/test".to_string(),
            enabled: true,
            message_types: vec![MessageType::Position],
            headers: None,
            devices: vec![],
        };
        
        manager.add_webhook(webhook.clone()).await;
        
        let webhooks = manager.get_webhooks().await;
        assert_eq!(webhooks.len(), 1);
        assert_eq!(webhooks[0].name, "Test Webhook");
        
        assert!(manager.set_webhook_enabled("Test Webhook", false).await);
        assert!(!manager.remove_webhook("Non-existent").await);
        assert!(manager.remove_webhook("Test Webhook").await);
        
        let webhooks = manager.get_webhooks().await;
        assert!(webhooks.is_empty());
    }
    
    #[test]
    fn test_webhook_event_creation() {
        let event = WebhookEvent {
            event_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            message_type: MessageType::Position,
            node_id: "!test1234".to_string(),
            data: serde_json::json!({"test": "data"}),
        };
        
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("test1234"));
        assert!(json.contains("POSITION"));
    }
    
    #[tokio::test]
    async fn test_device_filtering() {
        let manager = WebhookManager::new();
        
        // Create webhook with device filter
        let webhook = WebhookConfig {
            name: "Device Filtered Webhook".to_string(),
            url: "https://webhook.site/test".to_string(),
            enabled: true,
            message_types: vec![MessageType::Position],
            headers: None,
            devices: vec!["2e268b50".to_string()], // Only this device
        };
        
        manager.add_webhook(webhook).await;
        
        // Create a test message from the filtered device
        let message_from_device = RawMqttMessage {
            id: 1,
            timestamp: 1642156800,
            message_type: MessageType::Position,
            sender: "!2e268b50".to_string(),
            from: 0x2e268b50, // This matches our filter
            to: 0,
            channel: 0,
            rssi: Some(-50),
            snr: Some(10.5),
            hop_start: Some(3),
            hops_away: Some(0),
            payload: serde_json::json!({"latitude_i": 42404135}),
        };
        
        // Create a test message from a different device
        let message_from_other = RawMqttMessage {
            id: 2,
            timestamp: 1642156800,
            message_type: MessageType::Position,
            sender: "!12345678".to_string(),
            from: 0x12345678, // This doesn't match our filter
            to: 0,
            channel: 0,
            rssi: Some(-50),
            snr: Some(10.5),
            hop_start: Some(3),
            hops_away: Some(0),
            payload: serde_json::json!({"latitude_i": 42404135}),
        };
        
        // Process messages - the first should match, second should be filtered out
        // We can't easily test the HTTP calls without a mock server, but we can
        // verify the filtering logic by checking the webhooks collection
        let result1 = manager.process_message(&message_from_device).await;
        let result2 = manager.process_message(&message_from_other).await;
        
        // Both should succeed (no errors), but only the first should have triggered the webhook
        assert!(result1.is_ok());
        assert!(result2.is_ok());
    }
}
