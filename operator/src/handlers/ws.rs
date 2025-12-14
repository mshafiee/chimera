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
        State,
    },
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::broadcast;

/// WebSocket state for managing connections
pub struct WsState {
    /// Broadcast channel for sending updates to all clients
    pub tx: broadcast::Sender<WsEvent>,
}

impl WsState {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(100);
        Self { tx }
    }

    /// Broadcast an event to all connected clients
    pub fn broadcast(&self, event: WsEvent) {
        // Ignore send errors (no receivers)
        let _ = self.tx.send(event);
    }
}

impl Default for WsState {
    fn default() -> Self {
        Self::new()
    }
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
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "serialize_decimal_option")]
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

/// WebSocket upgrade handler
///
/// GET /ws
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<WsState>>,
) -> Response {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

/// Handle individual WebSocket connection
async fn handle_socket(socket: WebSocket, state: Arc<WsState>) {
    let (mut sender, mut receiver) = socket.split();

    // Subscribe to broadcast channel
    let mut rx = state.tx.subscribe();

    // Task to send events to client
    let send_task = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            let msg = match serde_json::to_string(&event) {
                Ok(json) => Message::Text(json.into()),
                Err(e) => {
                    tracing::error!(error = %e, "Failed to serialize WebSocket event");
                    continue;
                }
            };

            if sender.send(msg).await.is_err() {
                // Client disconnected
                break;
            }
        }
    });

    // Task to receive messages from client (mainly for ping/pong)
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                Message::Ping(data) => {
                    tracing::debug!("Received ping");
                    // Pong is automatically sent by axum
                    let _ = data;
                }
                Message::Close(_) => {
                    tracing::debug!("Client requested close");
                    break;
                }
                _ => {}
            }
        }
    });

    // Wait for either task to finish
    tokio::select! {
        _ = send_task => {
            tracing::debug!("Send task finished");
        }
        _ = recv_task => {
            tracing::debug!("Receive task finished");
        }
    }

    tracing::debug!("WebSocket connection closed");
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
