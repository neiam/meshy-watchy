use serde_json::Value;
use sqlx::{Row, SqlitePool};
use std::sync::Arc;
use tracing::Level;

mod config;
mod database;
mod models;

use config::load_config;
use database::Database;
use models::{MeshWatchyError, TextMessagePayload};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing with more verbose logging for replay
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tracing::info!("Starting Text Messages Replay Tool");

    // Load configuration
    let config = load_config("config.toml").await?;
    tracing::info!("Configuration loaded successfully");

    // Initialize database
    let database = Arc::new(Database::new(&config.database.url).await?);
    tracing::info!("Database initialized");

    // Create the replay processor
    let replay_processor = TextMessagesReplayProcessor::new(database);

    // Run the replay
    replay_processor.run().await?;

    tracing::info!("Text Messages Replay completed successfully");
    Ok(())
}

pub struct TextMessagesReplayProcessor {
    database: Arc<Database>,
}

impl TextMessagesReplayProcessor {
    pub fn new(database: Arc<Database>) -> Self {
        Self { database }
    }

    pub async fn run(&self) -> Result<(), MeshWatchyError> {
        tracing::info!("Starting text messages replay...");

        // Get all text messages from mesh_messages table
        let text_messages = self.get_text_messages().await?;

        tracing::info!("Found {} text messages to process", text_messages.len());

        let mut processed_count = 0;
        let mut error_count = 0;
        let mut inserted_count = 0;

        for message in text_messages {
            match self.process_text_message(message).await {
                Ok(was_inserted) => {
                    processed_count += 1;
                    if was_inserted {
                        inserted_count += 1;
                    }

                    if processed_count % 100 == 0 {
                        tracing::info!("Processed {} messages so far...", processed_count);
                    }
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!("Error processing message: {}", e);
                }
            }
        }

        tracing::info!(
            "Replay completed: {} processed, {} inserted, {} errors",
            processed_count,
            inserted_count,
            error_count
        );

        Ok(())
    }

    async fn get_text_messages(&self) -> Result<Vec<TextMessageRecord>, MeshWatchyError> {
        let rows = sqlx::query(
            r#"
            SELECT 
                id,
                timestamp,
                sender_id,
                from_node,
                payload,
                message_type
            FROM mesh_messages 
            WHERE payload LIKE '%"text"%'
                AND json_extract(payload, '$.text') IS NOT NULL
            ORDER BY timestamp ASC
            "#,
        )
        .fetch_all(&self.database.pool)
        .await?;

        let mut messages = Vec::new();
        for row in rows {
            let message_type: String = row.get("message_type");
            messages.push(TextMessageRecord {
                id: row.get("id"),
                timestamp: row.get("timestamp"),
                sender_id: row.get("sender_id"),
                from_node: row.get("from_node"),
                payload: row.get("payload"),
                message_type,
            });
        }

        Ok(messages)
    }

    async fn process_text_message(
        &self,
        message: TextMessageRecord,
    ) -> Result<bool, MeshWatchyError> {
        // Check if this text message already exists in the text_messages table
        let exists = self.text_message_exists(message.id).await?;

        if exists {
            tracing::debug!("Text message {} already exists, skipping", message.id);
            return Ok(false);
        }

        // Parse the payload as JSON
        let payload_json: Value =
            serde_json::from_str(&message.payload).map_err(|e| MeshWatchyError::Json(e))?;

        // Try to deserialize into TextMessagePayload
        let text_payload: TextMessagePayload =
            serde_json::from_value(payload_json).map_err(|e| MeshWatchyError::Json(e))?;

        // Update message type to TEXT if it's currently UNKNOWN
        if message.message_type == "UNKNOWN" {
            self.update_message_type(message.id).await?;
        }

        // Insert the text message
        self.insert_text_message(message.id, &text_payload).await?;

        tracing::debug!(
            "Inserted text message from {}: \"{}\"",
            message.sender_id,
            text_payload.text.chars().take(50).collect::<String>()
        );

        Ok(true)
    }

    async fn text_message_exists(&self, message_id: i64) -> Result<bool, MeshWatchyError> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM text_messages WHERE message_id = ?")
                .bind(message_id)
                .fetch_one(&self.database.pool)
                .await?;

        Ok(count > 0)
    }

    async fn insert_text_message(
        &self,
        message_id: i64,
        text_payload: &TextMessagePayload,
    ) -> Result<(), MeshWatchyError> {
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO text_messages
            (message_id, text)
            VALUES (?, ?)
            "#,
        )
        .bind(message_id)
        .bind(&text_payload.text)
        .execute(&self.database.pool)
        .await?;

        Ok(())
    }

    async fn update_message_type(&self, message_id: i64) -> Result<(), MeshWatchyError> {
        sqlx::query(
            r#"
            UPDATE mesh_messages 
            SET message_type = 'TEXT' 
            WHERE id = ?
            "#,
        )
        .bind(message_id)
        .execute(&self.database.pool)
        .await?;

        Ok(())
    }
}

#[derive(Debug)]
struct TextMessageRecord {
    id: i64,
    #[allow(dead_code)]
    timestamp: i64,
    sender_id: String,
    #[allow(dead_code)]
    from_node: i64,
    payload: String,
    message_type: String,
}

// Add database access to the Database struct by making pool public for this binary
impl Database {
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_replay_processor() {
        let db = Database::new("sqlite::memory:").await.unwrap();
        let db_arc = Arc::new(db);

        let processor = TextMessagesReplayProcessor::new(db_arc);

        // Test that processor can be created without error
        assert!(processor.database.pool().is_closed() == false);
    }
}
