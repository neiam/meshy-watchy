use crate::models::{
    AppConfig, DatabaseConfig, MeshWatchyError, MessageType, MqttConfig, WebConfig, WebhookConfig,
};
use std::collections::HashMap;
use std::fs;

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mqtt: MqttConfig::default(),
            database: DatabaseConfig::default(),
            web: WebConfig::default(),
            webhooks: Vec::new(),
        }
    }
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            broker_host: "public.meshtastic.org".to_string(),
            broker_port: 1883,
            username: None,
            password: None,
            topics: vec!["msh/US/+/json/+/+".to_string()],
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "sqlite:mesh_watchy.db".to_string(),
            max_connections: 10,
        }
    }
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1".to_string(),
            bind_port: 3000,
            static_dir: "static".to_string(),
            templates_dir: "templates".to_string(),
        }
    }
}

/// Load configuration from TOML file
pub async fn load_config(path: &str) -> Result<AppConfig, MeshWatchyError> {
    if std::path::Path::new(path).exists() {
        let config_str = fs::read_to_string(path)
            .map_err(|e| MeshWatchyError::Config(format!("Failed to read config file: {}", e)))?;

        let mut config: AppConfig = toml::from_str(&config_str)
            .map_err(|e| MeshWatchyError::Config(format!("Failed to parse config: {}", e)))?;

        // Load webhooks from separate file if it exists
        if std::path::Path::new("webhooks.toml").exists() {
            let webhook_config = load_webhook_config("webhooks.toml").await?;
            config.webhooks = webhook_config;
        }

        Ok(config)
    } else {
        // Create default config file
        let default_config = AppConfig::default();
        let config_str = toml::to_string_pretty(&default_config).map_err(|e| {
            MeshWatchyError::Config(format!("Failed to serialize default config: {}", e))
        })?;

        fs::write(path, config_str).map_err(|e| {
            MeshWatchyError::Config(format!("Failed to write default config: {}", e))
        })?;

        tracing::info!("Created default configuration file at {}", path);
        Ok(default_config)
    }
}

/// Load webhook configuration from separate TOML file
pub async fn load_webhook_config(path: &str) -> Result<Vec<WebhookConfig>, MeshWatchyError> {
    if std::path::Path::new(path).exists() {
        let webhook_str = fs::read_to_string(path).map_err(|e| {
            MeshWatchyError::Config(format!("Failed to read webhook config: {}", e))
        })?;

        #[derive(serde::Deserialize)]
        struct WebhookFile {
            webhook: Vec<WebhookConfig>,
        }

        let webhook_file: WebhookFile = toml::from_str(&webhook_str).map_err(|e| {
            MeshWatchyError::Config(format!("Failed to parse webhook config: {}", e))
        })?;

        Ok(webhook_file.webhook)
    } else {
        // Create example webhook config
        let example_webhooks = create_example_webhook_config();
        save_webhook_config(path, &example_webhooks).await?;
        Ok(example_webhooks)
    }
}

/// Save webhook configuration to file
pub async fn save_webhook_config(
    path: &str,
    webhooks: &[WebhookConfig],
) -> Result<(), MeshWatchyError> {
    #[derive(serde::Serialize)]
    struct WebhookFile<'a> {
        webhook: &'a [WebhookConfig],
    }

    let webhook_file = WebhookFile { webhook: webhooks };
    let webhook_str = toml::to_string_pretty(&webhook_file).map_err(|e| {
        MeshWatchyError::Config(format!("Failed to serialize webhook config: {}", e))
    })?;

    fs::write(path, webhook_str)
        .map_err(|e| MeshWatchyError::Config(format!("Failed to write webhook config: {}", e)))?;

    Ok(())
}

/// Create example webhook configuration
fn create_example_webhook_config() -> Vec<WebhookConfig> {
    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert(
        "Authorization".to_string(),
        "Bearer YOUR_TOKEN_HERE".to_string(),
    );

    vec![
        WebhookConfig {
            name: "Position Alerts".to_string(),
            url: "https://your-webhook-url.com/position".to_string(),
            enabled: false,
            message_types: vec![MessageType::Position],
            headers: Some(headers.clone()),
            devices: vec![],
        },
        WebhookConfig {
            name: "Low Battery Alert".to_string(),
            url: "https://your-webhook-url.com/battery".to_string(),
            enabled: false,
            message_types: vec![MessageType::Telemetry],
            headers: Some(headers),
            devices: vec![],
        },
        WebhookConfig {
            name: "All Messages".to_string(),
            url: "https://webhook.site/unique-id".to_string(),
            enabled: false,
            message_types: vec![
                MessageType::Position,
                MessageType::Nodeinfo,
                MessageType::Telemetry,
            ],
            headers: None,
            devices: vec![],
        },
    ]
}

/// Validate configuration
pub fn validate_config(config: &AppConfig) -> Result<(), MeshWatchyError> {
    // Validate MQTT config
    if config.mqtt.broker_host.is_empty() {
        return Err(MeshWatchyError::Config(
            "MQTT broker host cannot be empty".to_string(),
        ));
    }

    if config.mqtt.broker_port == 0 {
        return Err(MeshWatchyError::Config(
            "MQTT broker port must be valid".to_string(),
        ));
    }

    if config.mqtt.topics.is_empty() {
        return Err(MeshWatchyError::Config(
            "At least one MQTT topic must be specified".to_string(),
        ));
    }

    // Validate database config
    if config.database.url.is_empty() {
        return Err(MeshWatchyError::Config(
            "Database URL cannot be empty".to_string(),
        ));
    }

    if config.database.max_connections == 0 {
        return Err(MeshWatchyError::Config(
            "Max connections must be greater than 0".to_string(),
        ));
    }

    // Validate web config
    if config.web.bind_port == 0 {
        return Err(MeshWatchyError::Config(
            "Web server port must be valid".to_string(),
        ));
    }

    // Validate webhook configs
    for webhook in &config.webhooks {
        if webhook.enabled {
            if webhook.url.is_empty() {
                return Err(MeshWatchyError::Config(format!(
                    "Webhook '{}' URL cannot be empty",
                    webhook.name
                )));
            }

            if !webhook.url.starts_with("http://") && !webhook.url.starts_with("https://") {
                return Err(MeshWatchyError::Config(format!(
                    "Webhook '{}' URL must start with http:// or https://",
                    webhook.name
                )));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_default_config_creation() {
        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("test_config.toml");

        let config = load_config(config_path.to_str().unwrap()).await.unwrap();
        assert_eq!(config.mqtt.broker_host, "public.meshtastic.org");
        assert_eq!(config.web.bind_port, 3000);
    }

    #[test]
    fn test_config_validation() {
        let mut config = AppConfig::default();
        assert!(validate_config(&config).is_ok());

        config.mqtt.broker_host = String::new();
        assert!(validate_config(&config).is_err());
    }
}
