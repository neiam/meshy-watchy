use std::sync::Arc;
use tracing::Level;

mod config;
mod database;
mod models;
mod mqtt;
mod web;
mod webhooks;

use config::{load_config, validate_config};
use database::Database;
use mqtt::MqttManager;
use tokio::sync::broadcast;
use web::{start_server, AppState};
use webhooks::WebhookManager;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tracing::info!("Starting Mesh Watchy application");

    // Load configuration
    let config = load_config("config.toml").await?;
    validate_config(&config)?;

    tracing::info!("Configuration loaded successfully");

    // Initialize database
    let database = Arc::new(Database::new(&config.database.url).await?);
    tracing::info!("Database initialized");

    // Initialize webhook manager
    let webhook_manager = Arc::new(WebhookManager::new());
    webhook_manager.load_webhooks(config.webhooks.clone()).await;
    tracing::info!(
        "Webhook manager initialized with {} webhooks",
        config.webhooks.len()
    );

    // Create WebSocket broadcast channel early so we can pass it to MQTT manager
    let (ws_tx, _) = broadcast::channel::<web::WsMessage>(1000);

    // Initialize MQTT manager
    let mut mqtt_manager = MqttManager::new();
    mqtt_manager
        .start(
            config.mqtt.clone(),
            database.clone(),
            webhook_manager.clone(),
            Some(Arc::new(ws_tx.clone())),
        )
        .await?;
    tracing::info!("MQTT client started");

    // Create web application state
    let mut env = minijinja::Environment::new();

    // Add custom date formatting filter for strftime compatibility
    env.add_filter(
        "strftime",
        |value: &minijinja::Value, format: String| -> Result<String, minijinja::Error> {
            if let Some(dt) = value.as_str() {
                // Try to parse the datetime string
                if let Ok(parsed_dt) = chrono::DateTime::parse_from_rfc3339(dt) {
                    Ok(parsed_dt.format(&format).to_string())
                } else if let Ok(utc_dt) = dt.parse::<chrono::DateTime<chrono::Utc>>() {
                    Ok(utc_dt.format(&format).to_string())
                } else {
                    Err(minijinja::Error::new(
                        minijinja::ErrorKind::InvalidOperation,
                        "Could not parse datetime",
                    ))
                }
            } else {
                Err(minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    "Value is not a datetime string",
                ))
            }
        },
    );

    // Add custom relative time filter for "n minutes ago" formatting
    env.add_filter(
        "timeago",
        |value: &minijinja::Value| -> Result<String, minijinja::Error> {
            let timestamp = if let Some(ts) = value.as_i64() {
                ts
            } else if let Some(ts_str) = value.as_str() {
                ts_str.parse::<i64>().map_err(|_| {
                    minijinja::Error::new(
                        minijinja::ErrorKind::InvalidOperation,
                        "Could not parse timestamp",
                    )
                })?
            } else {
                return Err(minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    format!("Value is not a timestamp, got: {:?}", value.kind()),
                ));
            };

            let now = chrono::Utc::now().timestamp();
            let diff = now - timestamp;

            if diff < 0 {
                return Ok("in the future".to_string());
            }

            let (amount, unit) = if diff < 60 {
                (diff, if diff == 1 { "second" } else { "seconds" })
            } else if diff < 3600 {
                let minutes = diff / 60;
                (minutes, if minutes == 1 { "minute" } else { "minutes" })
            } else if diff < 86400 {
                let hours = diff / 3600;
                (hours, if hours == 1 { "hour" } else { "hours" })
            } else if diff < 2592000 {
                // 30 days
                let days = diff / 86400;
                (days, if days == 1 { "day" } else { "days" })
            } else if diff < 31536000 {
                // 365 days
                let months = diff / 2592000;
                (months, if months == 1 { "month" } else { "months" })
            } else {
                let years = diff / 31536000;
                (years, if years == 1 { "year" } else { "years" })
            };

            Ok(format!("{} {} ago", amount, unit))
        },
    );

    minijinja_embed::load_templates!(&mut env);
    tracing::info!(
        "Template environment initialized with {} templates",
        env.templates().count()
    );

    let app_state = AppState {
        database,
        webhook_manager,
        env,
        ws_tx: Arc::new(ws_tx),
    };

    // Start web server
    tracing::info!(
        "Starting web server on {}:{}",
        config.web.bind_address,
        config.web.bind_port
    );

    // Handle graceful shutdown
    let shutdown_signal = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");

        tracing::info!("Shutdown signal received, cleaning up...");

        // Stop MQTT client
        if let Err(e) = mqtt_manager.stop().await {
            tracing::error!("Error stopping MQTT client: {}", e);
        }

        tracing::info!("Mesh Watchy shutdown complete");
    };

    // Run server with graceful shutdown
    tokio::select! {
        result = start_server(&config.web.bind_address, config.web.bind_port, app_state) => {
            if let Err(e) = result {
                tracing::error!("Web server error: {}", e);
                return Err(e.into());
            }
        }
        _ = shutdown_signal => {
            tracing::info!("Graceful shutdown completed");
        }
    }

    Ok(())
}
