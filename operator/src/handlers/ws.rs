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
        Query, State,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
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
    pub fn new(
        api_keys: HashMap<String, crate::middleware::Role>,
        jwt_secret: String,
        allow_anonymous_readonly: bool,
    ) -> Self {
        let (tx, _) = broadcast::channel(100);
        Self {
            tx,
            api_keys,
            jwt_secret,
            allow_anonymous_readonly,
        }
    }

    /// Broadcast an event to all connected clients
    pub fn broadcast(&self, event: WsEvent) {
        // Ignore send errors (no receivers)
        let _ = self.tx.send(event);
    }

    /// Authenticate a token (either API key or JWT)
    pub async fn authenticate(&self, token: &str) -> Option<crate::middleware::AuthenticatedUser> {
        // Try API key first. The identifier must NOT embed the raw token —
        // it is written to logs throughout the connection lifecycle.
        if let Some(role) = self.api_keys.get(token) {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(token.as_bytes());
            let digest = hex::encode(&hasher.finalize()[..8]);
            return Some(crate::middleware::AuthenticatedUser {
                identifier: format!("api_key:{}", digest),
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
        // Serialize as a string to preserve exactness — f64 rounding corrupts
        // financial data and a failed conversion must not silently become 0.0.
        Some(decimal) => serializer.serialize_str(&decimal.to_string()),
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
                let response = ws.on_upgrade(move |socket| {
                    handle_socket(socket, state, Some("anonymous".to_string()), crate::middleware::Role::Readonly)
                });
                tracing::info!("WebSocket upgrade successful (anonymous)");
                return response;
            }
            tracing::warn!("WebSocket connection rejected: no token provided");
            // Return a 401 Unauthorized response instead of upgrading
            return StatusCode::UNAUTHORIZED.into_response();
        }
    };

    tracing::info!(token_prefix = %token.chars().take(8).collect::<String>(), "WebSocket connection attempt");

    // Validate token asynchronously
    match state.authenticate(&token).await {
        Some(user) => {
            let identifier = user.identifier.clone();
            let role = user.role;
            tracing::info!(identifier = %identifier, role = %role, "WebSocket connection authenticated");
            let identifier_for_closure = identifier.clone();
            let response = ws.on_upgrade(move |socket| {
                handle_socket(socket, state, Some(identifier_for_closure), role)
            });
            tracing::info!("WebSocket upgrade successful for user: {}", identifier);
            response
        }
        None => {
            tracing::warn!(token_prefix = %token.chars().take(8).collect::<String>(), "WebSocket connection rejected: invalid token");
            // Return a 401 Unauthorized response instead of upgrading and closing
            StatusCode::UNAUTHORIZED.into_response()
        }
    }
}

/// Handle individual WebSocket connection
async fn handle_socket(
    socket: WebSocket,
    state: Arc<WsState>,
    user_identifier: Option<String>,
    role: crate::middleware::Role,
) {
    // If no identifier, close the connection immediately
    let user_id = match user_identifier {
        Some(id) => id,
        None => {
            tracing::warn!("WebSocket closed: no valid authentication");
            let _ = socket.close().await;
            return;
        }
    };

    tracing::info!(user = %user_id, role = %role, "WebSocket connection established, starting message handler");

    let (mut sender, mut receiver) = socket.split();

    // Subscribe to broadcast channel
    let mut rx = state.tx.subscribe();

    let user_id_for_send = user_id.clone();
    let user_id_for_recv = user_id.clone();
    let user_id_cleanup = user_id.clone();
    tracing::debug!(user = %user_id, "WebSocket subscribed to broadcast channel");

    // Task to send events to client
    let mut send_task = tokio::spawn(async move {
        let mut event_count = 0;
        loop {
            let event = match rx.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    // Slow consumer: instead of silently dropping the client,
                    // catch up to the latest event and log the skips.
                    tracing::warn!(
                        user = %user_id_for_send,
                        skipped = skipped,
                        "WebSocket client lagged, skipped events"
                    );
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tracing::debug!(user = %user_id_for_send, "Broadcast channel closed");
                    break;
                }
            };

            // Role-based filtering: readonly (incl. anonymous) clients receive
            // only operational events, never position/trade data. Previously
            // the role was only logged and every client got the full stream.
            if !role.has_permission(crate::middleware::Role::Operator)
                && !matches!(event, WsEvent::HealthUpdate(_) | WsEvent::Alert(_))
            {
                continue;
            }

            event_count += 1;
            let msg = match serde_json::to_string(&event) {
                Ok(json) => {
                    tracing::debug!(user = %user_id_for_send, event_count, "Sending WebSocket event");
                    Message::Text(json)
                }
                Err(e) => {
                    tracing::error!(error = %e, user = %user_id_for_send, "Failed to serialize WebSocket event");
                    continue;
                }
            };

            if sender.send(msg).await.is_err() {
                // Client disconnected
                tracing::info!(user = %user_id_for_send, events_sent = event_count, "WebSocket client disconnected");
                break;
            }
        }
        tracing::debug!(user = %user_id_for_send, events_sent = event_count, "WebSocket send task completed");
    });

    // Task to receive messages from client (mainly for ping/pong)
    let mut recv_task = tokio::spawn(async move {
        let mut msg_count = 0;
        while let Some(result) = receiver.next().await {
            match result {
                Ok(msg) => {
                    msg_count += 1;
                    match msg {
                        Message::Ping(data) => {
                            tracing::debug!(user = %user_id_for_recv, msg_count, "Received ping, pong will be automatic");
                            let _ = data;
                        }
                        Message::Close(frame) => {
                            tracing::info!(user = %user_id_for_recv, msg_count, close_reason = ?frame.as_ref().map(|f| &f.reason), "Client requested close");
                            break;
                        }
                        Message::Pong(_) => {
                            tracing::debug!(user = %user_id_for_recv, msg_count, "Received pong");
                        }
                        Message::Text(text) => {
                            tracing::debug!(user = %user_id_for_recv, msg_count, text_len = text.len(), "Received text message");
                        }
                        Message::Binary(data) => {
                            tracing::debug!(user = %user_id_for_recv, msg_count, data_len = data.len(), "Received binary message");
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, user = %user_id_for_recv, "WebSocket receive error");
                    break;
                }
            }
        }
        tracing::debug!(user = %user_id_for_recv, messages_received = msg_count, "WebSocket receive task completed");
    });

    // Wait for either task to finish, then abort the losing task so both
    // halves of the socket are dropped deterministically (select! alone only
    // detaches the loser).
    tokio::select! {
        _ = &mut send_task => {
            recv_task.abort();
            tracing::info!(user = %user_id_cleanup, "WebSocket send task finished first");
        }
        _ = &mut recv_task => {
            send_task.abort();
            tracing::info!(user = %user_id_cleanup, "WebSocket receive task finished first");
        }
    }

    tracing::info!(user = %user_id_cleanup, "WebSocket connection closed and cleanup completed");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

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
