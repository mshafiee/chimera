//! WebSocket handler for real-time updates
//!
//! Provides real-time updates to connected clients:
//! - Position updates
//! - Health status changes
//! - Trade notifications
//! - Alerts

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State, Query,
    },
    http::StatusCode,
    response::{Response, IntoResponse},
};
use futures_util::{SinkExt, StreamExt};
use rust_decimal::prelude::*;
use rust_decimal::Decimal;
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

/// WebSocket state for managing connections
pub struct WsState {
    /// Broadcast channel for sending updates to all clients
    pub tx: broadcast::Sender<WsEvent>,
    /// API keys for authentication (key -> role)
    pub api_keys: HashMap<String, crate::middleware::Role>,
    /// JWT secret for token validation
    pub jwt_secret: String,
    /// Whether to allow anonymous readonly access
    pub allow_anonymous_readonly: bool,
}

impl WsState {
    pub fn new(api_keys: HashMap<String, crate::middleware::Role>, jwt_secret: String, allow_anonymous_readonly: bool) -> Self {
        let (tx, _) = broadcast::channel(100);
        Self { tx, api_keys, jwt_secret, allow_anonymous_readonly }
    }

    /// Broadcast an event to all connected clients
    pub fn broadcast(&self, event: WsEvent) {
        // Ignore send errors (no receivers)
        let _ = self.tx.send(event);
    }

    /// Authenticate a token (either API key or JWT)
    pub async fn authenticate(&self, token: &str) -> Option<crate::middleware::AuthenticatedUser> {
        // Try API key first
        if let Some(role) = self.api_keys.get(token) {
            return Some(crate::middleware::AuthenticatedUser {
                identifier: format!("api_key:{}", token),
                role: *role,
            });
        }

        // Try JWT - decode inline
        use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};

        #[derive(Debug, serde::Deserialize)]
        struct JwtClaims {
            sub: String,
            role: String,
        }

        let validation = Validation::new(Algorithm::HS256);
        match decode::<JwtClaims>(
            token,
            &DecodingKey::from_secret(self.jwt_secret.as_bytes()),
            &validation,
        ) {
            Ok(token_data) => {
                if let Ok(role) = token_data.claims.role.parse::<crate::middleware::Role>() {
                    return Some(crate::middleware::AuthenticatedUser {
                        identifier: token_data.claims.sub,
                        role,
                    });
                }
            }
            Err(_) => {
                // Not a valid JWT
            }
        }

        None
    }
}

#[derive(Debug, Deserialize)]
pub struct WsQueryParams {
    pub token: Option<String>,
}

/// Events that can be sent over WebSocket
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum WsEvent {
    /// Position state changed
    #[serde(rename = "position_update")]
    PositionUpdate(PositionUpdateData),

    /// Health status changed
    #[serde(rename = "health_update")]
    HealthUpdate(HealthUpdateData),

    /// New trade executed
    #[serde(rename = "trade_update")]
    TradeUpdate(TradeUpdateData),

    /// Alert notification
    #[serde(rename = "alert")]
    Alert(AlertData),
}

#[derive(Clone, Debug, Serialize)]
pub struct PositionUpdateData {
    pub trade_uuid: String,
    pub state: String,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_decimal_option"
    )]
    pub unrealized_pnl_percent: Option<Decimal>,
}

fn serialize_decimal_option<S>(value: &Option<Decimal>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(decimal) => serializer.serialize_f64(decimal.to_f64().unwrap_or(0.0)),
        None => serializer.serialize_none(),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct HealthUpdateData {
    pub status: String,
    pub queue_depth: usize,
    pub trading_allowed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct TradeUpdateData {
    pub trade_uuid: String,
    pub status: String,
    pub token_symbol: Option<String>,
    pub strategy: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AlertData {
    pub severity: String, // "critical", "warning", "info"
    pub component: String,
    pub message: String,
}

/// WebSocket upgrade handler with authentication
///
/// GET /ws?token=<api_key_or_jwt>
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<WsState>>,
    Query(params): Query<WsQueryParams>,
) -> Response {
    tracing::info!("WebSocket upgrade request received");

    // Authenticate from query parameter (WebSocket can't send custom headers in browser)
    let token = match params.token {
        Some(t) if !t.is_empty() => t,
        _ => {
            // No token provided - check if anonymous readonly is allowed
            if state.allow_anonymous_readonly {
                tracing::info!("WebSocket connection allowed (anonymous readonly)");
                let response = ws.on_upgrade(|socket| handle_socket(socket, state, Some("anonymous".to_string())));
                tracing::info!("WebSocket upgrade successful (anonymous)");
                return response;
            }
            tracing::warn!("WebSocket connection rejected: no token provided");
            // Return a 401 Unauthorized response instead of upgrading
            return StatusCode::UNAUTHORIZED.into_response();
        }
    };

    tracing::info!(token_prefix = %&token[..token.len().min(8)], "WebSocket connection attempt");

    // Validate token asynchronously
    match state.authenticate(&token).await {
        Some(user) => {
            tracing::info!(identifier = %user.identifier, role = %user.role, "WebSocket connection authenticated");
            let response = ws.on_upgrade(move |socket| handle_socket(socket, state, Some(user.identifier)));
            tracing::info!("WebSocket upgrade successful for user: {}", user.identifier);
            response
        }
        None => {
            tracing::warn!(token_prefix = %&token[..token.len().min(8)], "WebSocket connection rejected: invalid token");
            // Return a 401 Unauthorized response instead of upgrading and closing
            StatusCode::UNAUTHORIZED.into_response()
        }
    }
}

/// Handle individual WebSocket connection
async fn handle_socket(socket: WebSocket, state: Arc<WsState>, user_identifier: Option<String>) {
    // If no identifier, close the connection immediately
    let user_id = match user_identifier {
        Some(id) => id,
        None => {
            tracing::warn!("WebSocket closed: no valid authentication");
            let _ = socket.close().await;
            return;
        }
    };

    tracing::info!(user = %user_id, "WebSocket connection established, starting message handler");

    let (mut sender, mut receiver) = socket.split();

    // Subscribe to broadcast channel
    let mut rx = state.tx.subscribe();

    tracing::debug!(user = %user_id, "WebSocket subscribed to broadcast channel");

    // Task to send events to client
    let send_task = tokio::spawn(async move {
        let mut event_count = 0;
        while let Ok(event) = rx.recv().await {
            event_count += 1;
            let msg = match serde_json::to_string(&event) {
                Ok(json) => {
                    tracing::debug!(user = %user_id, event_count, "Sending WebSocket event");
                    Message::Text(json)
                },
                Err(e) => {
                    tracing::error!(error = %e, user = %user_id, "Failed to serialize WebSocket event");
                    continue;
                }
            };

            if sender.send(msg).await.is_err() {
                // Client disconnected
                tracing::info!(user = %user_id, events_sent = event_count, "WebSocket client disconnected");
                break;
            }
        }
        tracing::debug!(user = %user_id, events_sent = event_count, "WebSocket send task completed");
    });

    // Task to receive messages from client (mainly for ping/pong)
    let recv_task = tokio::spawn(async move {
        let mut msg_count = 0;
        while let Some(result) = receiver.next().await {
            match result {
                Ok(msg) => {
                    msg_count += 1;
                    match msg {
                        Message::Ping(data) => {
                            tracing::debug!(user = %user_id, msg_count, "Received ping, pong will be automatic");
                            let _ = data;
                        }
                        Message::Close(frame) => {
                            tracing::info!(user = %user_id, msg_count, close_reason = ?frame.as_ref().map(|f| &f.reason), "Client requested close");
                            break;
                        }
                        Message::Pong(_) => {
                            tracing::debug!(user = %user_id, msg_count, "Received pong");
                        }
                        Message::Text(text) => {
                            tracing::debug!(user = %user_id, msg_count, text_len = text.len(), "Received text message");
                        }
                        Message::Binary(data) => {
                            tracing::debug!(user = %user_id, msg_count, data_len = data.len(), "Received binary message");
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, user = %user_id, "WebSocket receive error");
                    break;
                }
            }
        }
        tracing::debug!(user = %user_id, messages_received = msg_count, "WebSocket receive task completed");
    });

    // Wait for either task to finish
    tokio::select! {
        _ = send_task => {
            tracing::info!(user = %user_id, "WebSocket send task finished first");
        }
        _ = recv_task => {
            tracing::info!(user = %user_id, "WebSocket receive task finished first");
        }
    }

    tracing::info!(user = %user_id, "WebSocket connection closed and cleanup completed");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_event_serialization() {
        let event = WsEvent::PositionUpdate(PositionUpdateData {
            trade_uuid: "test-uuid".to_string(),
            state: "ACTIVE".to_string(),
            unrealized_pnl_percent: Some(Decimal::from_str("10.5").unwrap()),
        });

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("position_update"));
        assert!(json.contains("test-uuid"));
    }

    #[test]
    fn test_alert_serialization() {
        let event = WsEvent::Alert(AlertData {
            severity: "critical".to_string(),
            component: "RPC".to_string(),
            message: "Helius connection failed".to_string(),
        });

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("alert"));
        assert!(json.contains("critical"));
    }
}
