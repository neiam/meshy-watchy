use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// Core message types supported by Meshtastic protocol
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum MessageType {
    Position,
    Nodeinfo,
    Telemetry,
    Text,
    #[serde(other)]
    Unknown,
}

impl std::fmt::Display for MessageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageType::Position => write!(f, "POSITION"),
            MessageType::Nodeinfo => write!(f, "NODEINFO"),
            MessageType::Telemetry => write!(f, "TELEMETRY"),
            MessageType::Text => write!(f, "TEXT"),
            MessageType::Unknown => write!(f, "UNKNOWN"),
        }
    }
}

/// Core mesh message structure from MQTT
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct MeshMessage {
    pub id: i64,
    pub timestamp: i64,
    pub message_type: String,
    pub sender_id: String,
    pub from_node: i64,
    pub channel: i32,
    pub rssi: Option<i32>,
    pub snr: Option<f32>,
    pub hop_start: Option<i32>,
    pub hops_away: Option<i32>,
    pub mqtt_topic: String,
    pub payload: String, // JSON string
}

/// Raw MQTT message structure for parsing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMqttMessage {
    pub id: i64,
    pub timestamp: i64,
    #[serde(rename = "type")]
    pub message_type: MessageType,
    pub sender: String,
    pub from: i64,
    pub to: i64,
    pub channel: i32,
    pub rssi: Option<i32>,
    pub snr: Option<f32>,
    pub hop_start: Option<i32>,
    pub hops_away: Option<i32>,
    pub payload: serde_json::Value,
}

/// Position data payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionPayload {
    #[serde(rename = "PDOP")]
    pub pdop: Option<i32>,
    pub altitude: Option<i32>,
    pub ground_speed: Option<i32>,
    pub ground_track: Option<i32>,
    pub latitude_i: i32,
    pub longitude_i: i32,
    pub precision_bits: Option<i32>,
    pub sats_in_view: Option<i32>,
    pub time: Option<i64>,
}

impl PositionPayload {
    /// Convert integer latitude to decimal degrees
    pub fn latitude(&self) -> f64 {
        self.latitude_i as f64 / 10_000_000.0
    }

    /// Convert integer longitude to decimal degrees
    pub fn longitude(&self) -> f64 {
        self.longitude_i as f64 / 10_000_000.0
    }

    /// Convert ground track to decimal degrees (if present)
    pub fn ground_track_degrees(&self) -> Option<f64> {
        self.ground_track.map(|track| track as f64 / 1000.0)
    }
}

/// Node information payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfoPayload {
    pub hardware: i32,
    pub id: String,
    pub longname: String,
    pub role: i32,
    pub shortname: String,
}

/// Text message data payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextMessagePayload {
    pub text: String,
}

/// Text message data for display
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct TextMessage {
    pub message_id: i64,
    pub from_node: String,
    pub to_node: Option<String>,
    pub text: String,
    pub timestamp: DateTime<Utc>,
    pub channel: Option<i32>,
    pub rssi: Option<i32>,
    pub snr: Option<f32>,
    pub gateway: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryPayload {
    pub air_util_tx: Option<f32>,
    pub battery_level: Option<i32>,
    pub channel_utilization: Option<f32>,
    pub uptime_seconds: Option<i64>,
    pub voltage: Option<f32>,
}

/// Database record for position data
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PositionData {
    pub message_id: i64,
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: Option<i32>,
    pub ground_speed: Option<i32>,
    pub ground_track: Option<i32>,
    pub pdop: Option<i32>,
    pub satellites_visible: Option<i32>,
    pub gps_time: Option<i64>,
}

/// Database record for node information
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct NodeInfo {
    pub node_id: String,
    pub hardware_type: i32,
    pub long_name: String,
    pub short_name: String,
    pub role: i32,
    pub last_seen: i64,
}

/// Enhanced node information for web display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfoDisplay {
    pub node_id: String,
    pub hardware_type: i32,
    pub hardware_model: Option<String>,
    pub long_name: Option<String>,
    pub short_name: Option<String>,
    pub role: i32,
    pub last_seen: i64,
    pub first_seen: Option<i64>,
    pub is_active: bool,
    pub has_position: bool,
    pub battery_level: Option<i32>,
    pub user_id: Option<String>,
    pub firmware_version: Option<String>,
}

/// Database record for telemetry data
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct TelemetryData {
    pub message_id: i64,
    pub battery_level: Option<i32>,
    pub voltage: Option<f32>,
    pub air_util_tx: Option<f32>,
    pub channel_utilization: Option<f32>,
    pub uptime_seconds: Option<i64>,
}

/// Telemetry data with display formatting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryDisplay {
    pub node_id: String,
    pub battery_level: Option<i32>,
    pub battery_chart: String,
    pub voltage: Option<f32>,
    pub air_util_tx: Option<f32>,
    pub channel_utilization: Option<f32>,
    pub uptime_seconds: Option<i64>,
    pub timestamp: i64,
}

impl TelemetryDisplay {
    /// Convert battery levels over time to Unicode block characters showing time series
    /// Each character represents a battery reading at a point in time
    /// Battery levels are mapped to block heights: 0-100% -> ▁▂▃▄▅▆▇█
    pub fn battery_levels_to_time_chart(battery_levels: &[Option<i32>]) -> String {
        if battery_levels.is_empty() {
            return "────────────────────".to_string(); // No data placeholder
        }
        
        battery_levels.iter().map(|level| {
            match level {
                None => '─', // No data
                Some(level) => {
                    let level = (*level).min(101).max(0); // Clamp to valid range
                    
                    if level > 100 {
                        '🔌' // Plugged in / charging - but this might break the chart visually
                    } else {
                        // Map 0-100% to block characters (8 levels)
                        match level {
                            0..=12 => '▁',      // 0-12%: Lower one eighth block
                            13..=25 => '▂',     // 13-25%: Lower one quarter block  
                            26..=37 => '▃',     // 26-37%: Lower three eighths block
                            38..=50 => '▄',     // 38-50%: Lower half block
                            51..=62 => '▅',     // 51-62%: Lower five eighths block
                            63..=75 => '▆',     // 63-75%: Lower three quarters block
                            76..=87 => '▇',     // 76-87%: Lower seven eighths block
                            88..=100 => '█',    // 88-100%: Full block
                            _ => '?',           // Should not happen due to clamping
                        }
                    }
                }
            }
        }).collect()
    }
    
    /// Convert a single battery level to a block character (for backwards compatibility)
    pub fn battery_level_to_chart(battery_level: Option<i32>) -> String {
        Self::battery_levels_to_time_chart(&[battery_level])
    }
}

/// Webhook configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub message_types: Vec<MessageType>,
    pub headers: Option<std::collections::HashMap<String, String>>,
    /// Optional list of device IDs to filter on. If empty, all devices trigger webhook.
    /// Device IDs should be in hex format (e.g., "2e268b50")
    #[serde(default)]
    pub devices: Vec<String>,
}

/// Webhook event for sending
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEvent {
    pub event_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub message_type: MessageType,
    pub node_id: String,
    pub data: serde_json::Value,
}

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub mqtt: MqttConfig,
    pub database: DatabaseConfig,
    pub web: WebConfig,
    #[serde(default)]
    pub webhooks: Vec<WebhookConfig>,
}

/// MQTT configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MqttConfig {
    pub broker_host: String,
    pub broker_port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub topics: Vec<String>,
}

/// Database configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

/// Web server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    pub bind_address: String,
    pub bind_port: u16,
    pub static_dir: String,
    pub templates_dir: String,
}

/// API response for position data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionResponse {
    pub node_id: String,
    pub timestamp: DateTime<Utc>,
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: Option<i32>,
    pub battery_level: Option<i32>,
    pub rssi: Option<i32>,
    pub snr: Option<f32>,
}

/// Enhanced position response for map display with node info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionResponseDisplay {
    pub node_id: String,
    pub timestamp: DateTime<Utc>,
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: Option<i32>,
    pub battery_level: Option<i32>,
    pub rssi: Option<i32>,
    pub snr: Option<f32>,
    pub long_name: Option<String>,
    pub short_name: Option<String>,
    pub hardware_type: i32,
    pub hardware_model: Option<String>,
}

/// API response for network overview
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkOverview {
    pub total_nodes: i32,
    pub active_nodes: i32,
    pub messages_today: i32,
    pub last_message: Option<DateTime<Utc>>,
}

/// Recent message data for display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentMessage {
    pub id: i64,
    pub from_node: String,
    pub to_node: Option<String>,
    pub message_type: String,
    pub timestamp: DateTime<Utc>,
    pub channel: Option<i32>,
    pub gateway: Option<String>,
    pub raw_json: serde_json::Value,
}

/// Error types for the application
#[derive(Debug, thiserror::Error)]
pub enum MeshWatchyError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    
    #[error("MQTT error: {0}")]
    Mqtt(#[from] rumqttc::ClientError),
    
    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),
    
    #[error("Configuration error: {0}")]
    Config(String),
    
    #[error("Webhook error: {0}")]
    Webhook(#[from] reqwest::Error),
}

pub type Result<T> = std::result::Result<T, MeshWatchyError>;
