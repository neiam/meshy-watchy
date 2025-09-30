use crate::models::{
    MeshMessage, NodeInfo, MeshWatchyError, 
    PositionResponse, PositionResponseDisplay, NetworkOverview, MessageType, RawMqttMessage,
    PositionPayload, NodeInfoPayload, TelemetryPayload, RecentMessage, TelemetryData, TelemetryDisplay,
    TextMessagePayload, TextMessage
};
use chrono::{DateTime, Utc};
use sqlx::{Sqlite, SqlitePool, Row};
use std::collections::HashMap;

pub struct Database {
    pub(crate) pool: SqlitePool,
}

impl Database {
    /// Create a new database connection
    pub async fn new(database_url: &str) -> Result<Self, MeshWatchyError> {
        tracing::info!("Attempting to connect to database: {}", database_url);
        
        // Ensure parent directory exists if it's a file path
        if let Some(path) = database_url.strip_prefix("sqlite:") {
            let db_path = std::path::Path::new(path);
            
            // Only create directories if the path contains directory separators
            if let Some(parent) = db_path.parent() {
                if !parent.as_os_str().is_empty() {
                    tracing::debug!("Creating database directory: {:?}", parent);
                    std::fs::create_dir_all(parent).map_err(|e| {
                        MeshWatchyError::Config(format!("Failed to create database directory '{}': {}", parent.display(), e))
                    })?;
                }
            }
            
            // Check if we can write to the directory containing the database
            let current_dir = std::env::current_dir().map_err(|e| {
                MeshWatchyError::Config(format!("Failed to get current directory: {}", e))
            })?;
            
            tracing::debug!("Current working directory: {:?}", current_dir);
            tracing::debug!("Database path: {:?}", db_path);
            
            // Try to create a test file to check permissions
            let test_file_path = if db_path.is_absolute() {
                db_path.with_extension("test")
            } else {
                current_dir.join(path).with_extension("test")
            };
            
            if let Err(e) = std::fs::write(&test_file_path, "test") {
                tracing::error!("Cannot write to database directory: {}", e);
                return Err(MeshWatchyError::Config(format!(
                    "Cannot write to database directory '{}': {}. Check permissions or use a different database path.",
                    test_file_path.parent().unwrap_or(&current_dir).display(),
                    e
                )));
            } else {
                // Clean up test file
                let _ = std::fs::remove_file(test_file_path);
            }
            
            tracing::debug!("Database file will be created at: {:?}", 
                if db_path.is_absolute() { db_path.to_path_buf() } else { current_dir.join(path) });
        }
        
        // Connect to database (SQLite will create the file if it doesn't exist)
        let pool = SqlitePool::connect(database_url).await.map_err(|e| {
            tracing::error!("Failed to connect to database '{}': {}", database_url, e);
            e
        })?;
        
        let db = Database { pool };
        db.initialize_schema().await?;
        
        tracing::info!("Database connection established successfully: {}", database_url);
        Ok(db)
    }
    
    /// Initialize database schema
    pub async fn initialize_schema(&self) -> Result<(), MeshWatchyError> {
        // Create mesh_messages table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS mesh_messages (
                id INTEGER PRIMARY KEY NOT NULL,
                timestamp INTEGER NOT NULL,
                message_type TEXT NOT NULL,
                sender_id TEXT NOT NULL,
                from_node INTEGER NOT NULL,
                channel INTEGER NOT NULL,
                rssi INTEGER,
                snr REAL,
                hop_start INTEGER,
                hops_away INTEGER,
                mqtt_topic TEXT NOT NULL,
                payload TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            "#,
        )
        .execute(&self.pool)
        .await?;
        
        // Create position_data table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS position_data (
                message_id INTEGER NOT NULL REFERENCES mesh_messages(id) ON DELETE CASCADE,
                latitude REAL NOT NULL,
                longitude REAL NOT NULL,
                altitude INTEGER,
                ground_speed INTEGER,
                ground_track INTEGER,
                pdop INTEGER,
                satellites_visible INTEGER,
                gps_time INTEGER,
                PRIMARY KEY (message_id)
            );
            "#,
        )
        .execute(&self.pool)
        .await?;
        
        // Create node_info table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS node_info (
                node_id TEXT PRIMARY KEY NOT NULL,
                hardware_type INTEGER NOT NULL,
                long_name TEXT NOT NULL,
                short_name TEXT NOT NULL,
                role INTEGER NOT NULL,
                last_seen INTEGER NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            "#,
        )
        .execute(&self.pool)
        .await?;
        
        // Create telemetry_data table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS telemetry_data (
                message_id INTEGER NOT NULL REFERENCES mesh_messages(id) ON DELETE CASCADE,
                battery_level INTEGER,
                voltage REAL,
                air_util_tx REAL,
                channel_utilization REAL,
                uptime_seconds INTEGER,
                PRIMARY KEY (message_id)
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Create text_messages table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS text_messages (
                message_id INTEGER NOT NULL REFERENCES mesh_messages(id) ON DELETE CASCADE,
                text TEXT NOT NULL,
                PRIMARY KEY (message_id)
            );
            "#,
        )
        .execute(&self.pool)
        .await?;
        
        // Create indexes for performance
        self.create_indexes().await?;
        
        tracing::info!("Database schema initialized successfully");
        Ok(())
    }
    
    /// Create database indexes for performance
    async fn create_indexes(&self) -> Result<(), MeshWatchyError> {
        let indexes = vec![
            "CREATE INDEX IF NOT EXISTS idx_messages_timestamp ON mesh_messages(timestamp);",
            "CREATE INDEX IF NOT EXISTS idx_messages_type ON mesh_messages(message_type);",
            "CREATE INDEX IF NOT EXISTS idx_messages_from_node ON mesh_messages(from_node);",
            "CREATE INDEX IF NOT EXISTS idx_position_location ON position_data(latitude, longitude);",
            "CREATE INDEX IF NOT EXISTS idx_position_gps_time ON position_data(gps_time);",
            "CREATE INDEX IF NOT EXISTS idx_node_last_seen ON node_info(last_seen);",
            "CREATE INDEX IF NOT EXISTS idx_telemetry_battery ON telemetry_data(battery_level);",
        ];
        
        for index_sql in indexes {
            sqlx::query(index_sql).execute(&self.pool).await?;
        }
        
        Ok(())
    }
    
    /// Insert a mesh message and its associated payload data
    pub async fn insert_message(&self, raw_message: &RawMqttMessage, mqtt_topic: &str) -> Result<(), MeshWatchyError> {
        let mut tx = self.pool.begin().await?;
        
        // Insert the main message
        let message = MeshMessage {
            id: raw_message.id,
            timestamp: raw_message.timestamp,
            message_type: raw_message.message_type.to_string(),
            sender_id: raw_message.sender.clone(),
            from_node: raw_message.from,
            channel: raw_message.channel,
            rssi: raw_message.rssi,
            snr: raw_message.snr,
            hop_start: raw_message.hop_start,
            hops_away: raw_message.hops_away,
            mqtt_topic: mqtt_topic.to_string(),
            payload: raw_message.payload.to_string(),
        };
        
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO mesh_messages 
            (id, timestamp, message_type, sender_id, from_node, channel, rssi, snr, hop_start, hops_away, mqtt_topic, payload)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&message.id)
        .bind(&message.timestamp)
        .bind(&message.message_type)
        .bind(&message.sender_id)
        .bind(&message.from_node)
        .bind(&message.channel)
        .bind(&message.rssi)
        .bind(&message.snr)
        .bind(&message.hop_start)
        .bind(&message.hops_away)
        .bind(&message.mqtt_topic)
        .bind(&message.payload)
        .execute(&mut *tx)
        .await?;
        
        // Insert payload-specific data based on message type
        match &raw_message.message_type {
            MessageType::Position => {
                if let Ok(position) = serde_json::from_value::<PositionPayload>(raw_message.payload.clone()) {
                    self.insert_position_data(&mut tx, message.id, &position).await?;
                }
            }
            MessageType::Nodeinfo => {
                if let Ok(node_info) = serde_json::from_value::<NodeInfoPayload>(raw_message.payload.clone()) {
                    self.insert_node_info(&mut tx, &node_info, raw_message.timestamp).await?;
                }
            }
            MessageType::Telemetry => {
                if let Ok(telemetry) = serde_json::from_value::<TelemetryPayload>(raw_message.payload.clone()) {
                    self.insert_telemetry_data(&mut tx, message.id, &telemetry).await?;
                }
            }
            MessageType::Text => {
                if let Ok(text_payload) = serde_json::from_value::<TextMessagePayload>(raw_message.payload.clone()) {
                    self.insert_text_message(&mut tx, message.id, &text_payload).await?;
                }
            }
            MessageType::Unknown => {
                tracing::warn!("Unknown message type for message ID: {}", message.id);
            }
        }
        
        tx.commit().await?;
        Ok(())
    }
    
    /// Insert position data
    async fn insert_position_data(
        &self,
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        message_id: i64,
        position: &PositionPayload,
    ) -> Result<(), MeshWatchyError> {
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO position_data
            (message_id, latitude, longitude, altitude, ground_speed, ground_track, pdop, satellites_visible, gps_time)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(message_id)
        .bind(position.latitude())
        .bind(position.longitude())
        .bind(position.altitude)
        .bind(position.ground_speed)
        .bind(position.ground_track)
        .bind(position.pdop)
        .bind(position.sats_in_view)
        .bind(position.time)
        .execute(&mut **tx)
        .await?;
        
        Ok(())
    }
    
    /// Insert or update node information
    async fn insert_node_info(
        &self,
        tx: &mut sqlx::Transaction<'_, Sqlite>,
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
        .execute(&mut **tx)
        .await?;
        
        tracing::debug!("Inserted/updated node info for {}: {} ({})", 
            node_info.id, node_info.longname, node_info.shortname);
        
        Ok(())
    }
    
    /// Insert telemetry data
    async fn insert_telemetry_data(
        &self,
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        message_id: i64,
        telemetry: &TelemetryPayload,
    ) -> Result<(), MeshWatchyError> {
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO telemetry_data
            (message_id, battery_level, voltage, air_util_tx, channel_utilization, uptime_seconds)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(message_id)
        .bind(telemetry.battery_level)
        .bind(telemetry.voltage)
        .bind(telemetry.air_util_tx)
        .bind(telemetry.channel_utilization)
        .bind(telemetry.uptime_seconds)
        .execute(&mut **tx)
        .await?;
        
        Ok(())
    }

    /// Insert text message data
    async fn insert_text_message(
        &self,
        tx: &mut sqlx::Transaction<'_, Sqlite>,
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
        .execute(&mut **tx)
        .await?;
        
        Ok(())
    }
    
    /// Get recent positions for all nodes
    pub async fn get_recent_positions(&self, limit: i32) -> Result<Vec<PositionResponse>, MeshWatchyError> {
        let positions = sqlx::query_as::<_, (i64, i64, f64, f64, Option<i32>, Option<i32>, Option<i32>, Option<f32>)>(
            r#"
            SELECT 
                mm.from_node as node_id,
                mm.timestamp,
                pd.latitude,
                pd.longitude,
                pd.altitude,
                td.battery_level,
                mm.rssi,
                mm.snr
            FROM mesh_messages mm
            JOIN position_data pd ON mm.id = pd.message_id
            LEFT JOIN (
                SELECT 
                    t1.message_id,
                    t1.battery_level
                FROM telemetry_data t1
                JOIN mesh_messages m1 ON t1.message_id = m1.id
                WHERE m1.timestamp = (
                    SELECT MAX(m2.timestamp)
                    FROM mesh_messages m2
                    JOIN telemetry_data t2 ON m2.id = t2.message_id
                    WHERE m2.from_node = m1.from_node
                )
            ) td ON mm.from_node IN (
                SELECT m3.from_node 
                FROM mesh_messages m3 
                WHERE m3.id = td.message_id
            )
            ORDER BY mm.timestamp DESC
            LIMIT ?
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        
        let mut result = Vec::new();
        for (node_id, timestamp, latitude, longitude, altitude, battery_level, rssi, snr) in positions {
            result.push(PositionResponse {
                node_id: format!("!{:08x}", node_id as u32), // Convert to node ID format
                timestamp: DateTime::<Utc>::from_timestamp(timestamp, 0).unwrap_or_default(),
                latitude,
                longitude,
                altitude,
                battery_level,
                rssi,
                snr,
            });
        }
        
        Ok(result)
    }
    
    /// Get recent positions with enhanced node information for map display
    pub async fn get_recent_positions_with_info(&self, limit: i32) -> Result<Vec<PositionResponseDisplay>, MeshWatchyError> {
        tracing::debug!("Getting recent positions with info, limit: {}", limit);
        
        let positions = sqlx::query_as::<_, (i64, i64, f64, f64, Option<i32>, Option<i32>, Option<i32>, Option<f32>, Option<String>, Option<String>, Option<i32>)>(
            r#"
            SELECT 
                mm.from_node as node_id,
                mm.timestamp,
                pd.latitude,
                pd.longitude,
                pd.altitude,
                td.battery_level,
                mm.rssi,
                mm.snr,
                ni.long_name,
                ni.short_name,
                COALESCE(ni.hardware_type, 0) as hardware_type
            FROM mesh_messages mm
            JOIN position_data pd ON mm.id = pd.message_id
            LEFT JOIN node_info ni ON printf('!%08x', mm.from_node) = ni.node_id
            LEFT JOIN (
                SELECT 
                    printf('!%08x', mm.from_node) as node_id,
                    td.battery_level
                FROM mesh_messages mm
                JOIN telemetry_data td ON mm.id = td.message_id
                WHERE mm.id IN (
                    SELECT MAX(mm2.id) 
                    FROM mesh_messages mm2 
                    JOIN telemetry_data td2 ON mm2.id = td2.message_id 
                    WHERE mm2.from_node = mm.from_node 
                    AND td2.battery_level IS NOT NULL
                )
            ) td ON printf('!%08x', mm.from_node) = td.node_id
            ORDER BY mm.timestamp DESC
            LIMIT ?
            "#
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!("Database query failed in get_recent_positions_with_info: {}", e);
            e
        })?;
        
        let mut result = Vec::new();
        
        tracing::debug!("Database query returned {} rows", positions.len());
        for (i, (node_id, timestamp, latitude, longitude, altitude, battery_level, rssi, snr, long_name, short_name, hardware_type)) in positions.iter().enumerate() {
            tracing::debug!("Processing position {}: node_id={}, hardware_type={:?}", i, node_id, hardware_type);
            
            let hardware_type = hardware_type.unwrap_or(0);
            let hardware_model = Self::hardware_type_to_model(hardware_type);
            
            result.push(PositionResponseDisplay {
                node_id: format!("!{:08x}", *node_id as u32), // Convert to node ID format
                timestamp: DateTime::<Utc>::from_timestamp(*timestamp, 0).unwrap_or_default(),
                latitude: *latitude,
                longitude: *longitude,
                altitude: *altitude,
                battery_level: *battery_level,
                rssi: *rssi,
                snr: *snr,
                long_name: long_name.clone(),
                short_name: short_name.clone(),
                hardware_type,
                hardware_model,
            });
        }
        
        tracing::debug!("Successfully created {} PositionResponseDisplay objects", result.len());
        Ok(result)
    }
    
    /// Get recent positions with optional node filtering
    pub async fn get_recent_positions_filtered(&self, limit: i32, node_ids: Option<Vec<i64>>) -> Result<Vec<PositionResponse>, MeshWatchyError> {
        let mut query_string = r#"
            SELECT 
                mm.from_node as node_id,
                mm.timestamp,
                pd.latitude,
                pd.longitude,
                pd.altitude,
                td.battery_level,
                mm.rssi,
                mm.snr
            FROM mesh_messages mm
            INNER JOIN position_data pd ON mm.id = pd.message_id
            LEFT JOIN telemetry_data td ON mm.id = td.message_id
                AND td.message_id = (
                    SELECT mm2.id 
                    FROM mesh_messages mm2
                    WHERE mm2.from_node = mm.from_node AND mm2.timestamp <= mm.timestamp ORDER BY mm2.timestamp DESC LIMIT 1
                )
        "#.to_string();
        
        // Add WHERE clause for node filtering if provided
        if let Some(ref nodes) = node_ids {
            if !nodes.is_empty() {
                let placeholders: Vec<String> = nodes.iter().map(|_| "?".to_string()).collect();
                query_string.push_str(&format!(" WHERE mm.from_node IN ({})", placeholders.join(",")));
            }
        }
        
        query_string.push_str(" ORDER BY mm.timestamp DESC LIMIT ?");
        
        // Build the query dynamically
        let mut query = sqlx::query_as::<_, (i64, i64, f64, f64, Option<i32>, Option<i32>, Option<i32>, Option<f32>)>(&query_string);
        
        // Bind node IDs if provided
        if let Some(nodes) = node_ids {
            for node_id in nodes {
                query = query.bind(node_id);
            }
        }
        
        // Bind limit
        query = query.bind(limit);
        
        let positions = query.fetch_all(&self.pool).await.map_err(|e| {
            tracing::error!("Database query failed in get_recent_positions_filtered: {}", e);
            e
        })?;
        
        let mut result = Vec::new();
        for (node_id, timestamp, latitude, longitude, altitude, battery_level, rssi, snr) in positions {
            result.push(PositionResponse {
                node_id: format!("!{:08x}", node_id as u32), // Convert to node ID format
                timestamp: DateTime::<Utc>::from_timestamp(timestamp, 0).unwrap_or_default(),
                latitude,
                longitude,
                altitude,
                battery_level,
                rssi,
                snr,
            });
        }
        
        Ok(result)
    }
    
    /// Get network overview statistics
    pub async fn get_network_overview(&self) -> Result<NetworkOverview, MeshWatchyError> {
        // Get total nodes
        let total_nodes: i32 = sqlx::query_scalar("SELECT COUNT(DISTINCT node_id) FROM node_info")
            .fetch_one(&self.pool)
            .await?;
        
        // Get active nodes (seen in last 24 hours)
        let day_ago = chrono::Utc::now().timestamp() - 86400;
        let active_nodes: i32 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT node_id) FROM node_info WHERE last_seen > ?"
        )
        .bind(day_ago)
        .fetch_one(&self.pool)
        .await?;
        
        // Get messages today
        let messages_today: i32 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM mesh_messages WHERE timestamp > ?"
        )
        .bind(day_ago)
        .fetch_one(&self.pool)
        .await?;
        
        // Get last message timestamp
        let last_message_ts: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(timestamp) FROM mesh_messages"
        )
        .fetch_one(&self.pool)
        .await?;
        
        let last_message = last_message_ts
            .and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0));
        
        Ok(NetworkOverview {
            total_nodes,
            active_nodes,
            messages_today,
            last_message,
        })
    }
    
    /// Get all known nodes with enhanced information for display
    pub async fn get_nodes_display(&self) -> Result<Vec<crate::models::NodeInfoDisplay>, MeshWatchyError> {
        // Get current time for activity check (24 hours)
        let day_ago = chrono::Utc::now().timestamp() - 86400;
        
        let rows = sqlx::query(
            r#"
            SELECT DISTINCT
                ni.node_id,
                ni.hardware_type,
                ni.long_name,
                ni.short_name,
                ni.role,
                ni.last_seen,
                ni.created_at,
                -- Check if node has position data
                CASE WHEN EXISTS(
                    SELECT 1 FROM mesh_messages mm 
                    JOIN position_data pd ON mm.id = pd.message_id
                    WHERE printf('!%08x', mm.from_node) = ni.node_id
                ) THEN 1 ELSE 0 END as has_position,
                -- Get latest battery level from telemetry
                (
                    SELECT td.battery_level 
                    FROM mesh_messages mm 
                    JOIN telemetry_data td ON mm.id = td.message_id
                    WHERE printf('!%08x', mm.from_node) = ni.node_id 
                        AND td.battery_level IS NOT NULL
                    ORDER BY mm.timestamp DESC 
                    LIMIT 1
                ) as battery_level
            FROM node_info ni
            ORDER BY ni.last_seen DESC
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        let mut nodes = Vec::new();
        for row in rows {
            let node_id: String = row.get("node_id");
            let hardware_type: i32 = row.get("hardware_type");
            let long_name: String = row.get("long_name");
            let short_name: String = row.get("short_name");
            let role: i32 = row.get("role");
            let last_seen: i64 = row.get("last_seen");
            let created_at: Option<chrono::DateTime<chrono::Utc>> = row.get("created_at");
            let has_position: i32 = row.get("has_position");
            let battery_level: Option<i32> = row.get("battery_level");

            // Map hardware type to model name
            let hardware_model = match hardware_type {
                3 => Some("T-Echo".to_string()),
                47 => Some("Station G1".to_string()),
                50 => Some("RAK11200".to_string()),
                71 => Some("T-Deck".to_string()),
                _ => Some(format!("Hardware #{}", hardware_type)),
            };

            let node = crate::models::NodeInfoDisplay {
                node_id,
                hardware_type,
                hardware_model,
                long_name: if long_name.is_empty() { None } else { Some(long_name) },
                short_name: if short_name.is_empty() { None } else { Some(short_name) },
                role,
                last_seen,
                first_seen: created_at.map(|dt| dt.timestamp()),
                is_active: last_seen > day_ago,
                has_position: has_position > 0,
                battery_level,
                user_id: None, // Could be derived from node_id if needed
                firmware_version: None, // Not currently stored
            };

            nodes.push(node);
        }

        Ok(nodes)
    }
    
    /// Get all known nodes (basic info for API)
    pub async fn get_nodes(&self) -> Result<Vec<NodeInfo>, MeshWatchyError> {
        let nodes = sqlx::query_as::<_, NodeInfo>(
            "SELECT node_id, hardware_type, long_name, short_name, role, last_seen FROM node_info ORDER BY last_seen DESC"
        )
        .fetch_all(&self.pool)
        .await?;
        
        Ok(nodes)
    }
    
    /// Get message statistics by type
    pub async fn get_message_stats(&self) -> Result<HashMap<String, i32>, MeshWatchyError> {
        let stats = sqlx::query_as::<_, (String, i32)>(
            "SELECT message_type, COUNT(*) as count FROM mesh_messages GROUP BY message_type"
        )
        .fetch_all(&self.pool)
        .await?;
        
        let mut result = HashMap::new();
        for (msg_type, count) in stats {
            result.insert(msg_type, count);
        }
        
        Ok(result)
    }
    
    /// Get recent text messages for display
    pub async fn get_recent_text_messages(&self, limit: i32) -> Result<Vec<TextMessage>, MeshWatchyError> {
        let rows = sqlx::query(
            r#"
            SELECT 
                mm.id as message_id,
                printf('!%08x', mm.from_node) as from_node,
                tm.text,
                mm.timestamp,
                mm.channel,
                mm.rssi,
                mm.snr,
                mm.mqtt_topic
            FROM mesh_messages mm
            JOIN text_messages tm ON mm.id = tm.message_id
            ORDER BY mm.timestamp DESC
            LIMIT ?
            "#
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        
        let mut text_messages = Vec::new();
        for row in rows {
            let message_id: i64 = row.get("message_id");
            let from_node: String = row.get("from_node");
            let text: String = row.get("text");
            let timestamp: i64 = row.get("timestamp");
            let channel: Option<i32> = row.get("channel");
            let rssi: Option<i32> = row.get("rssi");
            let snr: Option<f32> = row.get("snr");
            let mqtt_topic: String = row.get("mqtt_topic");
            
            // Extract gateway from MQTT topic
            let gateway = mqtt_topic
                .split('/')
                .nth(1)
                .map(|s| s.to_string());
            
            text_messages.push(TextMessage {
                message_id,
                from_node,
                to_node: None, // We don't store to_node in current schema
                text,
                timestamp: DateTime::<Utc>::from_timestamp(timestamp, 0).unwrap_or_default(),
                channel,
                rssi,
                snr,
                gateway,
            });
        }
        
        Ok(text_messages)
    }

    /// Get recent messages for display
    pub async fn get_recent_messages(&self, limit: i32) -> Result<Vec<RecentMessage>, MeshWatchyError> {
        let rows = sqlx::query(
            r#"
            SELECT 
                id,
                from_node,
                sender_id,
                message_type,
                timestamp,
                channel,
                mqtt_topic,
                payload
            FROM mesh_messages 
            ORDER BY timestamp DESC 
            LIMIT ?
            "#
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        
        let mut messages = Vec::new();
        for row in rows {
            let id: i64 = row.get("id");
            let _from_node: i64 = row.get("from_node");
            let sender_id: String = row.get("sender_id");
            let message_type: String = row.get("message_type");
            let timestamp: i64 = row.get("timestamp");
            let channel: i32 = row.get("channel");
            let mqtt_topic: String = row.get("mqtt_topic");
            let payload: String = row.get("payload");
            
            // Parse payload as JSON for display
            let raw_json = serde_json::from_str(&payload)
                .unwrap_or(serde_json::json!({"error": "Invalid JSON"}));
            
            // Extract gateway from MQTT topic
            let gateway = mqtt_topic
                .split('/')
                .nth(1)
                .map(|s| s.to_string());
            
            messages.push(RecentMessage {
                id,
                from_node: sender_id.clone(),
                to_node: None, // We don't store to_node in the current schema
                message_type,
                timestamp: DateTime::<Utc>::from_timestamp(timestamp, 0).unwrap_or_default(),
                channel: Some(channel),
                gateway,
                raw_json,
            });
        }
        
        Ok(messages)
    }
    
    /// Get recent telemetry data with time-series battery charts
    pub async fn get_recent_telemetry(&self, limit: i32) -> Result<Vec<TelemetryDisplay>, MeshWatchyError> {
        let rows = sqlx::query(
            r#"
            SELECT 
                printf('!%08x', mm.from_node) as node_id,
                mm.timestamp,
                td.battery_level,
                td.voltage,
                td.air_util_tx,
                td.channel_utilization,
                td.uptime_seconds
            FROM mesh_messages mm
            JOIN telemetry_data td ON mm.id = td.message_id
            ORDER BY mm.timestamp DESC
            LIMIT ?
            "#
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        
        // Group telemetry data by node_id to create time series charts
        let mut node_data: std::collections::HashMap<String, Vec<TelemetryDisplay>> = std::collections::HashMap::new();
        
        for row in rows {
            let node_id: String = row.get("node_id");
            let timestamp: i64 = row.get("timestamp");
            let battery_level: Option<i32> = row.get("battery_level");
            let voltage: Option<f32> = row.get("voltage");
            let air_util_tx: Option<f32> = row.get("air_util_tx");
            let channel_utilization: Option<f32> = row.get("channel_utilization");
            let uptime_seconds: Option<i64> = row.get("uptime_seconds");
            
            let telemetry_entry = TelemetryDisplay {
                node_id: node_id.clone(),
                battery_level,
                battery_chart: String::new(), // Will be filled later
                voltage,
                air_util_tx,
                channel_utilization,
                uptime_seconds,
                timestamp,
            };
            
            node_data.entry(node_id).or_insert_with(Vec::new).push(telemetry_entry);
        }
        let mut telemetry = Vec::new();
        let mut _battery_levels: Vec<Option<i32>> = Vec::new();
        
        for (_node_id, mut node_telemetry) in node_data {
            // Sort by timestamp so we display oldest to newest
            node_telemetry.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
            
            // Extract battery levels in chronological order
            let battery_levels: Vec<Option<i32>> = node_telemetry
                .iter()
                .map(|t| t.battery_level)
                .collect();
            
            // Generate time series chart
            let battery_chart = TelemetryDisplay::battery_levels_to_time_chart(&battery_levels);
            
            // Use the latest telemetry entry as the representative one, but with the time series chart
            if let Some(mut latest) = node_telemetry.into_iter().last() {
                latest.battery_chart = battery_chart;
                telemetry.push(latest);
            }
        }
        
        // Sort by timestamp descending for API response
        telemetry.sort_by_key(|t| std::cmp::Reverse(t.timestamp));
        
        Ok(telemetry)
    }
    
    /// Get telemetry time series for a specific node with battery charts
    pub async fn get_node_telemetry_history(&self, node_id: &str, limit: i32) -> Result<Vec<TelemetryDisplay>, MeshWatchyError> {
        let rows = sqlx::query(
            r#"
            SELECT 
                printf('!%08x', mm.from_node) as node_id,
                mm.timestamp,
                td.battery_level,
                td.voltage,
                td.air_util_tx,
                td.channel_utilization,
                td.uptime_seconds
            FROM mesh_messages mm
            JOIN telemetry_data td ON mm.id = td.message_id
            WHERE printf('!%08x', mm.from_node) = ?
            ORDER BY mm.timestamp ASC
            LIMIT ?
            "#
        )
        .bind(node_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        
        let mut telemetry = Vec::new();
        let mut battery_levels = Vec::new();
        
        for row in rows {
            let node_id: String = row.get("node_id");
            let timestamp: i64 = row.get("timestamp");
            let battery_level: Option<i32> = row.get("battery_level");
            let voltage: Option<f32> = row.get("voltage");
            let air_util_tx: Option<f32> = row.get("air_util_tx");
            let channel_utilization: Option<f32> = row.get("channel_utilization");
            let uptime_seconds: Option<i64> = row.get("uptime_seconds");
            
            battery_levels.push(battery_level);
            
            telemetry.push(TelemetryDisplay {
                node_id,
                battery_level,
                battery_chart: String::new(), // Will be filled after loop
                voltage,
                air_util_tx,
                channel_utilization,
                uptime_seconds,
                timestamp,
            });
        }
        
        // Generate complete battery chart for all nodes combined
        let _complete_battery_chart = TelemetryDisplay::battery_levels_to_time_chart(&battery_levels);
        
        Ok(telemetry)
    }
    
    /// Clean up old messages (keep last N days)
    pub async fn cleanup_old_messages(&self, days_to_keep: i32) -> Result<u64, MeshWatchyError> {
        let cutoff_time = chrono::Utc::now().timestamp() - (days_to_keep as i64 * 86400);
        
        let result = sqlx::query("DELETE FROM mesh_messages WHERE timestamp < ?")
            .bind(cutoff_time)
            .execute(&self.pool)
            .await?;
        
        Ok(result.rows_affected())
    }
    
    /// Hardware type mapping for display
    fn hardware_type_to_model(hardware_type: i32) -> Option<String> {
        match hardware_type {
            3 => Some("T-Echo".to_string()),
            47 => Some("Station G1".to_string()),
            50 => Some("RAK11200".to_string()),
            71 => Some("T-Deck".to_string()),
            _ => Some(format!("Hardware #{}", hardware_type)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{MessageType, RawMqttMessage, PositionPayload, NodeInfoPayload, TelemetryPayload};
    
    #[tokio::test]
    async fn test_database_initialization() {
        let db = Database::new("sqlite::memory:").await.unwrap();
        
        // Test that we can get network overview without errors
        let overview = db.get_network_overview().await.unwrap();
        assert_eq!(overview.total_nodes, 0);
    }
    
    #[tokio::test]
    async fn test_insert_position_message() {
        let db = Database::new("sqlite::memory:").await.unwrap();
        
        let position_payload = PositionPayload {
            pdop: Some(156),
            altitude: Some(26),
            ground_speed: Some(1),
            ground_track: Some(33930000),
            latitude_i: 424041350,
            longitude_i: -711257033,
            precision_bits: Some(32),
            sats_in_view: Some(7),
            time: Some(1758156514),
        };
        
        let raw_message = RawMqttMessage {
            id: 123456,
            timestamp: chrono::Utc::now().timestamp(),
            message_type: MessageType::Position,
            sender: "!f9943e58".to_string(),
            from: 774277968,
            to: 4294967295,
            channel: 0,
            rssi: Some(-73),
            snr: Some(8.0),
            hop_start: Some(3),
            hops_away: Some(0),
            payload: serde_json::to_value(&position_payload).unwrap(),
        };
        
        db.insert_message(&raw_message, "msh/Zoo/2/json/ZooNet/!f9943e58").await.unwrap();
        
        let positions = db.get_recent_positions(10).await.unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].latitude, 42.4041350);
        assert_eq!(positions[0].longitude, -71.1257033);
    }
}
