use serde_json::Value;
use sqlx::{Row, SqlitePool};
use std::sync::Arc;
use tracing::Level;

mod config;
mod database;
mod models;

use config::load_config;
use database::Database;
use models::{MeshWatchyError, NodeInfoPayload};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing with more verbose logging for replay
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tracing::info!("Starting Node Info Replay Tool");

    // Load configuration
    let config = load_config("config.toml").await?;
    tracing::info!("Configuration loaded successfully");

    // Initialize database
    let database = Arc::new(Database::new(&config.database.url).await?);
    tracing::info!("Database initialized");

    // Create the replay processor
    let replay_processor = NodeInfoReplayProcessor::new(database);

    // Run the replay
    replay_processor.run().await?;

    tracing::info!("Node Info Replay completed successfully");
    Ok(())
}

pub struct NodeInfoReplayProcessor {
    database: Arc<Database>,
}

impl NodeInfoReplayProcessor {
    pub fn new(database: Arc<Database>) -> Self {
        Self { database }
    }

    pub async fn run(&self) -> Result<(), MeshWatchyError> {
        tracing::info!("Starting node info message replay...");

        // Get all node info messages from mesh_messages table
        let node_info_messages = self.get_node_info_messages().await?;

        tracing::info!(
            "Found {} node info messages to process",
            node_info_messages.len()
        );

        let mut processed_count = 0;
        let mut error_count = 0;
        let mut updated_count = 0;

        for message in node_info_messages {
            match self.process_node_info_message(message).await {
                Ok(was_updated) => {
                    processed_count += 1;
                    if was_updated {
                        updated_count += 1;
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
            "Replay completed: {} processed, {} updated, {} errors",
            processed_count,
            updated_count,
            error_count
        );

        Ok(())
    }

    async fn get_node_info_messages(&self) -> Result<Vec<NodeInfoMessage>, MeshWatchyError> {
        let rows = sqlx::query(
            r#"
            SELECT 
                id,
                timestamp,
                sender_id,
                payload
            FROM mesh_messages 
            WHERE message_type = 'NODEINFO' 
            ORDER BY timestamp ASC
            "#,
        )
        .fetch_all(&self.database.pool)
        .await?;

        let mut messages = Vec::new();
        for row in rows {
            messages.push(NodeInfoMessage {
                id: row.get("id"),
                timestamp: row.get("timestamp"),
                sender_id: row.get("sender_id"),
                payload: row.get("payload"),
            });
        }

        Ok(messages)
    }

    async fn process_node_info_message(
        &self,
        message: NodeInfoMessage,
    ) -> Result<bool, MeshWatchyError> {
        // Parse the payload as JSON
        let payload_json: Value =
            serde_json::from_str(&message.payload).map_err(MeshWatchyError::Json)?;

        // Try to deserialize into NodeInfoPayload
        let node_info: NodeInfoPayload =
            serde_json::from_value(payload_json).map_err(MeshWatchyError::Json)?;

        // Check if this node already exists and if this message is newer
        let should_update = self
            .should_update_node_info(&node_info.id, message.timestamp)
            .await?;

        if should_update {
            // Insert or update the node info
            self.upsert_node_info(&node_info, message.timestamp).await?;

            tracing::debug!(
                "Updated node info for {}: {} ({})",
                node_info.id,
                node_info.longname,
                node_info.shortname
            );

            Ok(true)
        } else {
            tracing::debug!("Skipping older node info message for {}", node_info.id);
            Ok(false)
        }
    }

    async fn should_update_node_info(
        &self,
        node_id: &str,
        timestamp: i64,
    ) -> Result<bool, MeshWatchyError> {
        let existing_timestamp: Option<i64> =
            sqlx::query_scalar("SELECT last_seen FROM node_info WHERE node_id = ?")
                .bind(node_id)
                .fetch_optional(&self.database.pool)
                .await?;

        match existing_timestamp {
            Some(existing) => Ok(timestamp > existing),
            None => Ok(true), // New node, should insert
        }
    }

    async fn upsert_node_info(
        &self,
        node_info: &NodeInfoPayload,
        timestamp: i64,
    ) -> Result<(), MeshWatchyError> {
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO node_info
            (node_id, hardware_type, long_name, short_name, role, last_seen, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
            "#,
        )
        .bind(&node_info.id)
        .bind(node_info.hardware)
        .bind(&node_info.longname)
        .bind(&node_info.shortname)
        .bind(node_info.role)
        .bind(timestamp)
        .execute(&self.database.pool)
        .await?;

        Ok(())
    }
}

#[derive(Debug)]
struct NodeInfoMessage {
    #[allow(dead_code)]
    id: i64,
    timestamp: i64,
    #[allow(dead_code)]
    sender_id: String,
    payload: String,
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

        let processor = NodeInfoReplayProcessor::new(db_arc);

        // Test that processor can be created without error
        assert!(processor.database.pool().is_closed() == false);
    }
}
