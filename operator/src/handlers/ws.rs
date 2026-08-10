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
                    handle_socket(
                        socket,
                        state,
                        Some("anonymous".to_string()),
                        crate::middleware::Role::Readonly,
                    )
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
                    // Expected event: dashboard clients drop WS connections
                    // routinely (tab close, network blip) — "Connection reset
                    // by peer" is the normal case, not an operator failure.
                    // Only log at ERROR for genuinely unexpected errors so the
                    // error scanner isn't flooded with client drops.
                    let err_str = e.to_string();
                    if err_str.contains("Connection reset")
                        || err_str.contains("closed")
                        || err_str.contains("EOF")
                    {
                        tracing::debug!(error = %e, user = %user_id_for_recv, "WebSocket client disconnected");
                    } else {
                        tracing::error!(error = %e, user = %user_id_for_recv, "WebSocket receive error");
                    }
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

    // ==========================================================================
    // ADDITIONAL COVERAGE
    // ==========================================================================

    fn test_state(anonymous: bool) -> Arc<WsState> {
        Arc::new(WsState::new(
            HashMap::from([
                ("api-key-1".to_string(), crate::middleware::Role::Operator),
                ("api-key-ro".to_string(), crate::middleware::Role::Readonly),
            ]),
            "jwt-secret".to_string(),
            anonymous,
        ))
    }

    #[test]
    fn test_ws_state_broadcast() {
        let state = test_state(false);
        // No receivers: send must not panic or error.
        state.broadcast(WsEvent::Alert(AlertData {
            severity: "info".to_string(),
            component: "test".to_string(),
            message: "hello".to_string(),
        }));

        let mut rx = state.tx.subscribe();
        state.broadcast(WsEvent::Alert(AlertData {
            severity: "info".to_string(),
            component: "test".to_string(),
            message: "hello".to_string(),
        }));
        let received = rx.try_recv().expect("event delivered");
        assert!(matches!(received, WsEvent::Alert(_)));
    }

    #[test]
    fn test_authenticate_api_key() {
        use futures_util::FutureExt;
        let state = test_state(false);
        let user = state
            .authenticate("api-key-1")
            .now_or_never()
            .expect("sync path resolves");
        let user = user.expect("api key authenticates");
        assert_eq!(user.role, crate::middleware::Role::Operator);
        // The identifier must be a hash, never the raw key.
        assert!(user.identifier.starts_with("api_key:"));
        assert!(!user.identifier.contains("api-key-1"));

        let ro = state
            .authenticate("api-key-ro")
            .now_or_never()
            .unwrap()
            .unwrap();
        assert_eq!(ro.role, crate::middleware::Role::Readonly);
    }

    #[test]
    fn test_authenticate_jwt() {
        use futures_util::FutureExt;
        use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
        let state = test_state(false);

        // Valid JWT with an operator role claim.
        let exp = chrono::Utc::now().timestamp() + 3600;
        let claims = serde_json::json!({ "sub": "user-42", "role": "Operator", "exp": exp });
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(b"jwt-secret"),
        )
        .expect("encode");
        let user = state
            .authenticate(&token)
            .now_or_never()
            .unwrap()
            .expect("jwt authenticates");
        assert_eq!(user.identifier, "user-42");
        assert_eq!(user.role, crate::middleware::Role::Operator);

        // Valid JWT whose role claim does not parse → rejected.
        let bad_role = serde_json::json!({ "sub": "u", "role": "NotARole", "exp": exp });
        let token = encode(
            &Header::new(Algorithm::HS256),
            &bad_role,
            &EncodingKey::from_secret(b"jwt-secret"),
        )
        .unwrap();
        assert!(
            state.authenticate(&token).now_or_never().unwrap().is_none(),
            "unparseable role must reject"
        );

        // Wrong secret / garbage → rejected.
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(b"wrong-secret"),
        )
        .unwrap();
        assert!(state.authenticate(&token).now_or_never().unwrap().is_none());

        assert!(state
            .authenticate("garbage")
            .now_or_never()
            .unwrap()
            .is_none());
        assert!(state.authenticate("").now_or_never().unwrap().is_none());
    }

    // ----------------------------------------------------------------------
    // End-to-end WebSocket tests: real axum server + tokio-tungstenite client.
    // ----------------------------------------------------------------------

    /// Spawn the ws router on a random port and return its base URL.
    fn spawn_ws_server(state: Arc<WsState>) -> String {
        use axum::routing::get;
        use axum::Router;
        use tokio::net::TcpListener;
        let app = Router::new()
            .route("/ws", get(ws_handler))
            .with_state(state);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("rt");
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let handle = rt.spawn(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("addr");
            let _ = tx.send(format!("ws://{addr}"));
            axum::serve(listener, app).await.expect("serve");
        });
        let url = rx.recv().expect("url");
        std::mem::forget(rt);
        std::mem::forget(handle);
        url
    }

    async fn ws_connect(
        url: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        use futures_util::StreamExt;
        let (ws, _resp) = tokio_tungstenite::connect_async(url)
            .await
            .expect("websocket handshake");
        ws
    }

    async fn read_text_with_timeout(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> String {
        use futures_util::StreamExt;
        use tokio_tungstenite::tungstenite::Message;
        tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("message within timeout")
            .expect("stream alive")
            .expect("no error")
            .into_text()
            .expect("text message")
    }

    #[tokio::test]
    async fn test_ws_end_to_end_operator_receives_all_events() {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message;

        let state = test_state(false);
        let url = spawn_ws_server(state.clone());
        let mut ws = ws_connect(&format!("{url}/ws?token=api-key-1")).await;
        // Let the handler task subscribe to the broadcast channel.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        state.broadcast(WsEvent::TradeUpdate(TradeUpdateData {
            trade_uuid: "t-1".to_string(),
            status: "OPEN".to_string(),
            token_symbol: Some("SOL".to_string()),
            strategy: "SHIELD".to_string(),
        }));
        let msg = read_text_with_timeout(&mut ws).await;
        assert!(msg.contains("trade_update"), "msg: {msg}");

        state.broadcast(WsEvent::Alert(AlertData {
            severity: "warning".to_string(),
            component: "x".to_string(),
            message: "boom".to_string(),
        }));
        let msg = read_text_with_timeout(&mut ws).await;
        assert!(msg.contains("alert"), "msg: {msg}");

        // Client → server messages hit the receive task.
        ws.send(Message::Text("hello server".into())).await.unwrap();
        ws.send(Message::Ping(vec![5, 6].into())).await.unwrap();
        ws.send(Message::Pong(vec![1, 2, 3].into())).await.unwrap();
        ws.send(Message::Binary(vec![9, 9].into())).await.unwrap();

        // Close: recv task ends, send task aborts, handler completes.
        ws.send(Message::Close(None)).await.unwrap();
        // Give the handler a moment to finish cleanup.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    #[tokio::test]
    async fn test_ws_readonly_receives_only_operational_events() {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message;

        let state = test_state(false);
        let url = spawn_ws_server(state.clone());
        let mut ws = ws_connect(&format!("{url}/ws?token=api-key-ro")).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // A position update must be filtered out for readonly clients.
        state.broadcast(WsEvent::PositionUpdate(PositionUpdateData {
            trade_uuid: "secret".to_string(),
            state: "OPEN".to_string(),
            unrealized_pnl_percent: Some(Decimal::from_str("99").unwrap()),
        }));
        // Health updates are allowed for readonly clients.
        state.broadcast(WsEvent::HealthUpdate(HealthUpdateData {
            status: "ok".to_string(),
            queue_depth: 3,
            trading_allowed: true,
        }));
        let msg = read_text_with_timeout(&mut ws).await;
        assert!(
            msg.contains("health_update"),
            "first received must be health: {msg}"
        );

        // No further message may arrive (the position update was filtered).
        let extra = tokio::time::timeout(std::time::Duration::from_millis(400), ws.next()).await;
        assert!(
            extra.is_err(),
            "readonly client must not receive position data"
        );

        ws.send(Message::Close(None)).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    #[tokio::test]
    async fn test_ws_anonymous_allowed() {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message;

        let state = test_state(true); // anonymous readonly allowed
        let url = spawn_ws_server(state.clone());
        let mut ws = ws_connect(&format!("{url}/ws")).await; // no token
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        state.broadcast(WsEvent::Alert(AlertData {
            severity: "info".to_string(),
            component: "anon".to_string(),
            message: "hi".to_string(),
        }));
        let msg = read_text_with_timeout(&mut ws).await;
        assert!(msg.contains("alert"), "msg: {msg}");

        ws.send(Message::Close(None)).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    #[tokio::test]
    async fn test_ws_rejects_missing_and_invalid_tokens() {
        // Anonymous access not allowed → no-token handshake must fail.
        let state = test_state(false);
        let url = spawn_ws_server(state.clone());
        let result = tokio_tungstenite::connect_async(&url).await;
        assert!(result.is_err(), "no-token connection must be rejected");

        // Invalid token → 401.
        let result = tokio_tungstenite::connect_async(format!("{url}/ws?token=bogus")).await;
        assert!(result.is_err(), "bogus token connection must be rejected");

        // Empty token parameter is treated as missing.
        let result = tokio_tungstenite::connect_async(format!("{url}/ws?token=")).await;
        assert!(result.is_err(), "empty token connection must be rejected");
    }

    #[tokio::test]
    async fn test_ws_send_failure_ends_send_task() {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message;

        let state = test_state(false);
        let url = spawn_ws_server(state.clone());
        let ws = ws_connect(&format!("{url}/ws?token=api-key-1")).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Abruptly drop the socket WITHOUT a Close frame: the server's recv
        // task hits a connection error while the send task fails on the next
        // broadcast → whichever task ends first aborts the other.
        drop(ws);

        state.broadcast(WsEvent::HealthUpdate(HealthUpdateData {
            status: "ok".to_string(),
            queue_depth: 1,
            trading_allowed: true,
        }));
        state.broadcast(WsEvent::HealthUpdate(HealthUpdateData {
            status: "ok".to_string(),
            queue_depth: 2,
            trading_allowed: true,
        }));
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    }

    #[tokio::test]
    async fn test_ws_jwt_authentication() {
        use futures_util::SinkExt;
        use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
        use tokio_tungstenite::tungstenite::Message;

        let exp = chrono::Utc::now().timestamp() + 3600;
        let claims = serde_json::json!({ "sub": "jwt-user", "role": "Operator", "exp": exp });
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(b"jwt-secret"),
        )
        .unwrap();

        let state = test_state(false);
        let url = spawn_ws_server(state.clone());
        let mut ws = ws_connect(&format!("{url}/ws?token={token}")).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        state.broadcast(WsEvent::TradeUpdate(TradeUpdateData {
            trade_uuid: "jwt-trade".to_string(),
            status: "OPEN".to_string(),
            token_symbol: None,
            strategy: "SPEAR".to_string(),
        }));
        let msg = read_text_with_timeout(&mut ws).await;
        assert!(msg.contains("jwt-trade"), "msg: {msg}");

        ws.send(Message::Close(None)).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
}
