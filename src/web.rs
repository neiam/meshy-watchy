use crate::database::Database;
use crate::models::{NetworkOverview, PositionResponse, NodeInfo, MeshWatchyError, WebhookConfig, TelemetryDisplay, RecentMessage, TextMessage};
use crate::webhooks::{WebhookManager, WebhookStatus, TestResult};
use crate::config::save_webhook_config;
use axum::{
    body::{Body},
    extract::{Path, Query, State, WebSocketUpgrade, ws::WebSocket},
    http::{header, Request, Response, StatusCode},
    response::{Html, Json, IntoResponse, Response as AxumResponse},
    routing::{get, post, put, delete},
    Router,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::convert::Infallible;
use std::task::{Context, Poll};
use serde::{Deserialize, Serialize};
use minijinja::{Environment, context};
use include_dir::{include_dir, Dir};
use mime_guess::from_path;
use tower::Service;
use tower_http::services::ServeDir;
use tracing::info;
use tokio::sync::broadcast;
use futures::{sink::SinkExt, stream::StreamExt};

// Static assets embedded during build
static ASSETS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets/dist");

/// Custom static file service for embedded assets
#[derive(Clone)]
struct StaticFileService;

impl Service<Request<Body>> for StaticFileService {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let path = req.uri().path().trim_start_matches('/');
        info!("{:?}", ASSETS_DIR.dirs().collect::<Vec<_>>());
        let response = if let Some(file) = ASSETS_DIR.get_file(path) {
            let mime_type = from_path(path).first_or_octet_stream();
            Response::builder()
                .header(header::CONTENT_TYPE, mime_type.as_ref())
                .header(header::CACHE_CONTROL, "public, max-age=31536000") // 1 year cache
                .body(Body::from(file.contents().to_vec()))
                .unwrap()
        } else {
            StatusCode::NOT_FOUND.into_response()
        };

        std::future::ready(Ok(response))
    }
}

/// Application state shared across handlers
#[derive(Clone)]
pub struct AppState {
    pub database: Arc<Database>,
    pub webhook_manager: Arc<WebhookManager>,
    pub env: Environment<'static>,
    pub ws_tx: Arc<broadcast::Sender<WsMessage>>,
}

/// WebSocket message types
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum WsMessage {
    #[serde(rename = "new_message")]
    NewMessage { message: RecentMessage },
    #[serde(rename = "node_update")]
    NodeUpdate { node: NodeInfo },
    #[serde(rename = "network_stats")]
    NetworkStats { overview: NetworkOverview },
}

/// Query parameters for position data
#[derive(Debug, Deserialize)]
pub struct PositionQuery {
    pub limit: Option<i32>,
    pub nodes: Option<String>, // Comma-separated list of node IDs
}

/// Request body for webhook testing
#[derive(Debug, Deserialize)]
pub struct TestWebhookRequest {
    pub webhook: WebhookConfig,
}

/// Create the web application router
pub fn create_app(state: AppState) -> Router {
    let mut app = Router::new()
        // Web pages
        .route("/", get(dashboard_handler))
        .route("/nodes", get(nodes_handler))
        .route("/messages", get(messages_handler))
        .route("/map", get(map_handler))
        .route("/webhooks", get(webhooks_handler))
        
        // API endpoints
        .route("/api/overview", get(api_overview))
        .route("/api/positions", get(api_positions))
        .route("/api/nodes", get(api_nodes))
        .route("/api/messages", get(api_messages))
        .route("/api/stats", get(api_stats))
        .route("/api/telemetry", get(api_telemetry))
        .route("/api/telemetry/{node_id}", get(api_node_telemetry))

        // WebSocket endpoint
        .route("/ws", get(websocket_handler))

        // Webhook management API
        .route("/api/webhooks", get(api_get_webhooks))
        .route("/api/webhooks", post(api_add_webhook))
        .route("/api/webhooks/{name}", put(api_update_webhook))
        .route("/api/webhooks/{name}", delete(api_delete_webhook))
        .route("/api/webhooks/{name}/toggle", post(api_toggle_webhook))
        .route("/api/webhooks/test", post(api_test_webhook))
        .route("/api/webhooks/stats/reset", post(api_reset_webhook_stats))
        .with_state(state);

    // Asset serving - conditional based on build type
    #[cfg(not(debug_assertions))]
    {
        app = app.nest_service("/assets/dist/", StaticFileService);
    }

    #[cfg(debug_assertions)]
    {
        app = app.nest_service("/assets", ServeDir::new("assets/"));
    }
    
    app
}

// Web page handlers

async fn dashboard_handler(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    tracing::debug!("Dashboard handler called");
    
    let overview = state.database.get_network_overview().await
        .map_err(|e| {
            tracing::error!("Failed to get network overview: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    let nodes = state.database.get_nodes_display().await
        .map_err(|e| {
            tracing::error!("Failed to get nodes: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    let recent_positions = state.database.get_recent_positions(10).await
        .map_err(|e| {
            tracing::error!("Failed to get recent positions: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    let recent_messages = state.database.get_recent_messages(20).await
        .map_err(|e| {
            tracing::error!("Failed to get recent messages: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    let webhook_stats = state.webhook_manager.get_webhook_status().await;
    
    // Get message statistics
    let message_stats = state.database.get_message_stats().await
        .map_err(|e| {
            tracing::error!("Failed to get message stats: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    tracing::debug!("Getting template: dashboard.html");
    let template = state.env.get_template("dashboard.html")
        .map_err(|e| {
            tracing::error!("Failed to get dashboard template: {}", e);
            tracing::debug!("Available templates: {:?}", state.env.templates().collect::<Vec<_>>());
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        
    tracing::debug!("Rendering template with context");
    let html = template.render(context! {
        overview => overview,
        nodes => nodes,
        recent_positions => recent_positions,
        recent_messages => recent_messages,
        webhook_stats => webhook_stats,
        message_stats => message_stats,
    }).map_err(|e| {
        tracing::error!("Failed to render dashboard template: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    
    tracing::debug!("Dashboard rendered successfully, {} bytes", html.len());
    Ok(Html(html))
}

async fn nodes_handler(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    let nodes = state.database.get_nodes_display().await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    let template = state.env.get_template("nodes.html")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        
    let html = template.render(context! {
        nodes => nodes,
    }).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    Ok(Html(html))
}

async fn messages_handler(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    tracing::debug!("Messages handler called");
    
    let text_messages = state.database.get_recent_text_messages(50).await
        .map_err(|e| {
            tracing::error!("Failed to get text messages: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    tracing::debug!("Retrieved {} text messages", text_messages.len());
    
    let template = state.env.get_template("messages.html")
        .map_err(|e| {
            tracing::error!("Failed to get messages template: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        
    tracing::debug!("Got messages template successfully");
    
    let html = template.render(context! {
        text_messages => text_messages,
    }).map_err(|e| {
        tracing::error!("Failed to render messages template: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    
    tracing::debug!("Messages rendered successfully, {} bytes", html.len());
    Ok(Html(html))
}

async fn map_handler(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    tracing::debug!("Map handler called");
    
    let positions = state.database.get_recent_positions_with_info(100).await
        .map_err(|e| {
            tracing::error!("Failed to get recent positions with info: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    tracing::debug!("Retrieved {} positions for map", positions.len());
    
    let positions_json = serde_json::to_string(&positions)
        .map_err(|e| {
            tracing::error!("Failed to serialize positions to JSON: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    tracing::debug!("Serialized positions JSON, {} bytes", positions_json.len());
    
    let template = state.env.get_template("map.html")
        .map_err(|e| {
            tracing::error!("Failed to get map template: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    tracing::debug!("Got map template successfully");
        
    let html = template.render(context! {
        positions => positions,
        positions_json => positions_json,
    }).map_err(|e| {
        tracing::error!("Failed to render map template: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    
    tracing::debug!("Map rendered successfully, {} bytes", html.len());
    Ok(Html(html))
}

async fn webhooks_handler(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    let webhooks = state.webhook_manager.get_webhook_status().await;
    
    let total_webhooks = webhooks.len();
    let enabled_webhooks = webhooks.iter().filter(|w| w.enabled).count();
    let total_sent: u64 = webhooks.iter().map(|w| w.total_sent).sum();
    let total_failed: u64 = webhooks.iter().map(|w| w.total_failed).sum();
    
    let template = state.env.get_template("webhooks.html")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        
    let html = template.render(context! {
        webhooks => webhooks,
        total_webhooks => total_webhooks,
        enabled_webhooks => enabled_webhooks,
        total_sent => total_sent,
        total_failed => total_failed,
    }).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    Ok(Html(html))
}

// API handlers

async fn api_overview(State(state): State<AppState>) -> Result<Json<NetworkOverview>, StatusCode> {
    let overview = state.database.get_network_overview().await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    Ok(Json(overview))
}

async fn api_positions(
    State(state): State<AppState>,
    Query(query): Query<PositionQuery>,
) -> Result<Json<Vec<PositionResponse>>, StatusCode> {
    let limit = query.limit.unwrap_or(50).min(1000); // Cap at 1000
    
    // Parse node IDs filter if provided
    let node_ids = if let Some(nodes_str) = &query.nodes {
        if !nodes_str.trim().is_empty() {
            Some(nodes_str
                .split(',')
                .filter_map(|s| s.trim().parse::<i64>().ok())
                .collect::<Vec<i64>>())
        } else {
            None
        }
    } else {
        None
    };
    
    let positions = state.database.get_recent_positions_filtered(limit, node_ids).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    Ok(Json(positions))
}

async fn api_nodes(State(state): State<AppState>) -> Result<Json<Vec<NodeInfo>>, StatusCode> {
    let nodes = state.database.get_nodes().await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    Ok(Json(nodes))
}

async fn api_stats(State(state): State<AppState>) -> Result<Json<HashMap<String, serde_json::Value>>, StatusCode> {
    let message_stats = state.database.get_message_stats().await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    let webhook_stats = state.webhook_manager.get_stats().await;
    
    let mut stats = HashMap::new();
    stats.insert("messages".to_string(), serde_json::to_value(message_stats).unwrap());
    stats.insert("webhooks".to_string(), serde_json::to_value(webhook_stats).unwrap());
    
    Ok(Json(stats))
}

// Messages API handler

async fn api_messages(
    State(state): State<AppState>,
    Query(query): Query<PositionQuery>, // Reuse the PositionQuery for limit parameter
) -> Result<Json<Vec<TextMessage>>, StatusCode> {
    let limit = query.limit.unwrap_or(50).min(1000); // Cap at 1000
    
    let text_messages = state.database.get_recent_text_messages(limit).await
        .map_err(|e| {
            tracing::error!("Failed to get text messages: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    Ok(Json(text_messages))
}

// Telemetry API handlers

async fn api_telemetry(State(state): State<AppState>) -> Result<Json<Vec<TelemetryDisplay>>, StatusCode> {
    let telemetry = state.database.get_recent_telemetry(50).await
        .map_err(|e| {
            tracing::error!("Failed to get recent telemetry: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    Ok(Json(telemetry))
}

async fn api_node_telemetry(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
) -> Result<Json<Vec<TelemetryDisplay>>, StatusCode> {
    let telemetry = state.database.get_node_telemetry_history(&node_id, 100).await
        .map_err(|e| {
            tracing::error!("Failed to get telemetry for node {}: {}", node_id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    Ok(Json(telemetry))
}

// Webhook API handlers

async fn api_get_webhooks(State(state): State<AppState>) -> Result<Json<Vec<WebhookStatus>>, StatusCode> {
    let webhooks = state.webhook_manager.get_webhook_status().await;
    Ok(Json(webhooks))
}

async fn api_add_webhook(
    State(state): State<AppState>,
    Json(webhook): Json<WebhookConfig>,
) -> Result<StatusCode, StatusCode> {
    state.webhook_manager.add_webhook(webhook).await;
    
    // Persist changes to config file
    let webhooks = state.webhook_manager.get_webhooks().await;
    if let Err(e) = save_webhook_config("webhooks.toml", &webhooks).await {
        tracing::error!("Failed to save webhook config: {}", e);
        return Ok(StatusCode::INTERNAL_SERVER_ERROR);
    }
    
    Ok(StatusCode::CREATED)
}

async fn api_update_webhook(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(webhook): Json<WebhookConfig>,
) -> Result<StatusCode, StatusCode> {
    if state.webhook_manager.update_webhook(&name, webhook).await {
        // Persist changes to config file
        let webhooks = state.webhook_manager.get_webhooks().await;
        if let Err(e) = save_webhook_config("webhooks.toml", &webhooks).await {
            tracing::error!("Failed to save webhook config: {}", e);
            return Ok(StatusCode::INTERNAL_SERVER_ERROR);
        }
        Ok(StatusCode::OK)
    } else {
        Ok(StatusCode::NOT_FOUND)
    }
}

async fn api_delete_webhook(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, StatusCode> {
    if state.webhook_manager.remove_webhook(&name).await {
        // Persist changes to config file
        let webhooks = state.webhook_manager.get_webhooks().await;
        if let Err(e) = save_webhook_config("webhooks.toml", &webhooks).await {
            tracing::error!("Failed to save webhook config: {}", e);
            return Ok(StatusCode::INTERNAL_SERVER_ERROR);
        }
        Ok(StatusCode::OK)
    } else {
        Ok(StatusCode::NOT_FOUND)
    }
}

async fn api_toggle_webhook(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let webhooks = state.webhook_manager.get_webhooks().await;
    if let Some(webhook) = webhooks.iter().find(|w| w.name == name) {
        let new_enabled = !webhook.enabled;
        if state.webhook_manager.set_webhook_enabled(&name, new_enabled).await {
            // Persist changes to config file
            let webhooks = state.webhook_manager.get_webhooks().await;
            if let Err(e) = save_webhook_config("webhooks.toml", &webhooks).await {
                tracing::error!("Failed to save webhook config: {}", e);
                return Ok(StatusCode::INTERNAL_SERVER_ERROR);
            }
            Ok(StatusCode::OK)
        } else {
            Ok(StatusCode::NOT_FOUND)
        }
    } else {
        Ok(StatusCode::NOT_FOUND)
    }
}

async fn api_test_webhook(
    State(state): State<AppState>,
    Json(request): Json<TestWebhookRequest>,
) -> Result<Json<TestResult>, StatusCode> {
    let result = state.webhook_manager.test_webhook(&request.webhook).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    Ok(Json(result))
}

async fn api_reset_webhook_stats(State(state): State<AppState>) -> Result<StatusCode, StatusCode> {
    state.webhook_manager.reset_stats().await;
    Ok(StatusCode::OK)
}

// WebSocket handler

async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> AxumResponse {
    ws.on_upgrade(move |socket| handle_websocket(socket, state))
}

async fn handle_websocket(socket: WebSocket, state: AppState) {
    let mut rx = state.ws_tx.subscribe();
    let (mut ws_sender, mut ws_receiver) = socket.split();
    
    tracing::info!("WebSocket connected");
    
    // Send initial data
    if let Ok(overview) = state.database.get_network_overview().await {
        let initial_msg = WsMessage::NetworkStats { overview };
        if let Ok(json) = serde_json::to_string(&initial_msg) {
            let _ = ws_sender.send(axum::extract::ws::Message::Text(json.into())).await;
        }
    }
    
    // Handle incoming and outgoing messages
    loop {
        tokio::select! {
            // Receive from broadcast channel and forward to WebSocket
            msg = rx.recv() => {
                match msg {
                    Ok(ws_msg) => {
                        if let Ok(json) = serde_json::to_string(&ws_msg) {
                            if let Err(_) = ws_sender.send(axum::extract::ws::Message::Text(json.into())).await {
                                tracing::debug!("WebSocket send failed, client disconnected");
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::debug!("WebSocket broadcast channel closed");
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        tracing::warn!("WebSocket broadcast lagged");
                        // Continue without breaking
                    }
                }
            }
            // Handle incoming messages (for now just ping/pong)
            msg = ws_receiver.next() => {
                match msg {
                    Some(Ok(axum::extract::ws::Message::Text(_))) => {
                        // Echo back for now
                    }
                    Some(Ok(axum::extract::ws::Message::Close(_))) => {
                        tracing::debug!("WebSocket closed by client");
                        break;
                    }
                    Some(Err(e)) => {
                        tracing::error!("WebSocket error: {}", e);
                        break;
                    }
                    None => {
                        tracing::debug!("WebSocket stream ended");
                        break;
                    }
                    _ => {
                        // Ignore other message types
                    }
                }
            }
        }
    }
    
    tracing::info!("WebSocket disconnected");
}

/// Broadcast a WebSocket message to all connected clients
pub async fn broadcast_ws_message(tx: &broadcast::Sender<WsMessage>, message: WsMessage) {
    if let Err(e) = tx.send(message) {
        tracing::warn!("Failed to broadcast WebSocket message: {}", e);
    }
}

/// Start the web server
pub async fn start_server(
    bind_address: &str,
    bind_port: u16,
    state: AppState,
) -> Result<(), MeshWatchyError> {
    let app = create_app(state);
    
    let listener = tokio::net::TcpListener::bind(format!("{}:{}", bind_address, bind_port))
        .await
        .map_err(|e| MeshWatchyError::Config(format!("Failed to bind to {}:{}: {}", bind_address, bind_port, e)))?;
    
    tracing::info!("Web server starting on http://{}:{}", bind_address, bind_port);
    
    axum::serve(listener, app)
        .await
        .map_err(|e| MeshWatchyError::Config(format!("Web server error: {}", e)))?;
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum_test::TestServer;
    use crate::database::Database;
    use crate::webhooks::WebhookManager;
    
    async fn create_test_state() -> AppState {
        let database = Arc::new(Database::new("sqlite::memory:").await.unwrap());
        let webhook_manager = Arc::new(WebhookManager::new());
        let (ws_tx, _) = broadcast::channel::<WsMessage>(1000);
        
        AppState {
            database,
            webhook_manager,
            env: Environment::new(),
            ws_tx: Arc::new(ws_tx),
        }
    }
    
    #[tokio::test]
    async fn test_api_overview() {
        let state = create_test_state().await;
        let app = create_app(state);
        let server = TestServer::new(app).unwrap();
        
        let response = server.get("/api/overview").await;
        response.assert_status_ok();
        
        let overview: NetworkOverview = response.json();
        assert_eq!(overview.total_nodes, 0);
    }
    
    #[tokio::test]
    async fn test_api_positions() {
        let state = create_test_state().await;
        let app = create_app(state);
        let server = TestServer::new(app).unwrap();
        
        let response = server.get("/api/positions").await;
        response.assert_status_ok();
        
        let positions: Vec<PositionResponse> = response.json();
        assert!(positions.is_empty());
    }
    
    #[tokio::test]
    async fn test_webhook_api() {
        let state = create_test_state().await;
        let app = create_app(state);
        let server = TestServer::new(app).unwrap();
        
        // Get initial webhooks (should be empty)
        let response = server.get("/api/webhooks").await;
        response.assert_status_ok();
        
        let webhooks: Vec<WebhookStatus> = response.json();
        assert!(webhooks.is_empty());
        
        // Add a webhook
        let webhook = WebhookConfig {
            name: "Test Webhook".to_string(),
            url: "https://webhook.site/test".to_string(),
            enabled: true,
            message_types: vec![],
            headers: None,
            devices: vec![],
        };
        
        let response = server.post("/api/webhooks").json(&webhook).await;
        response.assert_status(StatusCode::CREATED);
        
        // Get webhooks again (should have one)
        let response = server.get("/api/webhooks").await;
        response.assert_status_ok();
        
        let webhooks: Vec<WebhookStatus> = response.json();
        assert_eq!(webhooks.len(), 1);
        assert_eq!(webhooks[0].name, "Test Webhook");
    }
}
