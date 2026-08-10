//! Shared mock HTTP servers for integration/unit tests.
//!
//! Provides:
//! - [`MockServer`]: generic axum router on an ephemeral port.
//! - [`json_rpc_mock`]: JSON-RPC mock (Solana RPC / Helius RPC targets).
//! - [`DuneMock`]: REST mock of the Dune Analytics API v1.
//! - [`HeliusApiMock`]: REST mock of the Helius enhanced API (transactions,
//!   webhooks).
//! - [`JupiterQuoteMock`]: REST mock of the Jupiter quote API.
//!
//! Test files include this module via `#[path = "../common/mock_rpc.rs"]`.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::routing::{any, get, post};
use axum::Router;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

/// Spawn `router` on an ephemeral port and return the base URL.
pub async fn spawn_router(router: Router) -> (String, MockServer) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("local addr");
    let url = format!("http://{addr}");
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("mock server");
    });
    (url, MockServer { handle })
}

/// Owns a spawned mock server; aborts it on drop.
pub struct MockServer {
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// JSON-RPC dispatch handler: `(method, params) -> result`.
/// `None` yields an error response (code -32004). A `Some` value shaped as
/// `{"__rpc_error__": {"code": ..., "message": ...}}` yields a JSON-RPC error
/// response with that code/message (the way a real node reports e.g.
/// `-32004: Transaction not found`).
pub type RpcHandler = Arc<dyn Fn(&str, Value) -> Option<Value> + Send + Sync>;

/// Wrap a closure as an [`RpcHandler`].
pub fn rpc_handler<F>(f: F) -> RpcHandler
where
    F: Fn(&str, Value) -> Option<Value> + Send + Sync + 'static,
{
    Arc::new(f)
}

/// Marker for a JSON-RPC error response (see [`RpcHandler`]).
pub fn rpc_error(code: i64, message: &str) -> Value {
    json!({"__rpc_error__": {"code": code, "message": message}})
}

/// Spawn a JSON-RPC mock server.
pub async fn json_rpc_mock(handler: RpcHandler) -> (String, MockServer) {
    async fn dispatch(State(handler): State<RpcHandler>, body: String) -> (StatusCode, String) {
        let parsed: Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32600, "message": "bad request"}})
                        .to_string(),
                )
            }
        };
        let id = parsed.get("id").cloned().unwrap_or(Value::Null);
        let method = parsed
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        let params = parsed.get("params").cloned().unwrap_or(Value::Null);
        match handler(&method, params) {
            Some(result) => {
                if let Some(err) = result.get("__rpc_error__") {
                    (
                        StatusCode::OK,
                        json!({"jsonrpc": "2.0", "id": id, "error": err}).to_string(),
                    )
                } else {
                    (
                        StatusCode::OK,
                        json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
                    )
                }
            }
            None => (
                StatusCode::OK,
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32004, "message": "mock: no handler"}})
                    .to_string(),
            ),
        }
    }
    let router = Router::new().route("/", any(dispatch)).with_state(handler);
    spawn_router(router).await
}

/// Shared state for the Dune REST mock.
#[derive(Default)]
pub struct DuneMockState {
    /// Every request path observed by the mock (debug aid).
    pub requests: Vec<String>,
    /// Raw bodies of observed POST requests.
    pub request_bodies: Vec<String>,
    /// `QUERY_STATE_COMPLETED`, `QUERY_STATE_FAILED`, or anything else (pending).
    pub status: String,
    pub error_message: String,
    pub csv: String,
    pub json_rows: Vec<Value>,
    /// Number of status polls that report PENDING before COMPLETED.
    pub pending_polls: usize,
    pub executed_query_ids: Vec<u64>,
    /// When set, `/results/csv` returns 500 (tests the CSV-fetch failure path).
    pub fail_csv: bool,
    /// When set to a query id, only that query's CSV fetch returns 500.
    pub fail_csv_query_id: Option<u64>,
    /// When set, executing this query id returns 500 (tests the execute
    /// failure path for a specific promote query).
    pub fail_execute_query_id: Option<u64>,
    /// When set, `/results` returns 500 (tests the JSON fallback failure).
    pub fail_results: bool,
}

/// REST mock of `api.dune.com/api/v1`.
pub struct DuneMock {
    pub url: String,
    pub state: Arc<Mutex<DuneMockState>>,
    _server: MockServer,
}

impl DuneMock {
    pub async fn spawn() -> Self {
        let state = Arc::new(Mutex::new(DuneMockState {
            status: "QUERY_STATE_COMPLETED".to_string(),
            ..Default::default()
        }));

        async fn execute(
            State(state): State<Arc<Mutex<DuneMockState>>>,
            Path(id): Path<u64>,
        ) -> (StatusCode, String) {
            let fail = {
                let mut st = state.lock().await;
                st.executed_query_ids.push(id);
                st.fail_execute_query_id == Some(id)
            };
            if fail {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({"error": "query execution failed"}).to_string(),
                )
            } else {
                (
                    StatusCode::OK,
                    json!({"execution_id": format!("exec-{id}")}).to_string(),
                )
            }
        }

        async fn status(
            State(state): State<Arc<Mutex<DuneMockState>>>,
            Path(id): Path<String>,
        ) -> (StatusCode, String) {
            let mut st = state.lock().await;
            let status = if st.pending_polls > 0 {
                st.pending_polls -= 1;
                "QUERY_STATE_PENDING".to_string()
            } else {
                st.status.clone()
            };
            let body = if status == "QUERY_STATE_FAILED" {
                json!({"state": status, "error": {"message": st.error_message}})
            } else {
                json!({"state": status})
            };
            let _ = id;
            (StatusCode::OK, body.to_string())
        }

        async fn csv(
            State(state): State<Arc<Mutex<DuneMockState>>>,
            Path(id): Path<String>,
        ) -> (StatusCode, String) {
            let st = state.lock().await;
            let fail = st.fail_csv
                || st
                    .fail_csv_query_id
                    .map(|qid| id == format!("exec-{qid}"))
                    .unwrap_or(false);
            if fail {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "csv exploded".to_string(),
                )
            } else {
                (StatusCode::OK, st.csv.clone())
            }
        }

        async fn results(
            State(state): State<Arc<Mutex<DuneMockState>>>,
            Path(id): Path<String>,
        ) -> (StatusCode, String) {
            let _ = id;
            let st = state.lock().await;
            if st.fail_results {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "results exploded".to_string(),
                )
            } else {
                (
                    StatusCode::OK,
                    json!({"result": {"rows": st.json_rows}}).to_string(),
                )
            }
        }

        async fn record(
            State(state): State<Arc<Mutex<DuneMockState>>>,
            req: Request<Body>,
            next: axum::middleware::Next,
        ) -> Response {
            let method = req.method().clone();
            let uri = req.uri().clone();
            let mut st = state.lock().await;
            st.requests.push(format!("{method} {uri}"));
            drop(st);
            next.run(req).await
        }

        let router = Router::new()
            .route("/api/v1/query/:id/execute", post(execute))
            .route("/api/v1/execution/:id/status", get(status))
            .route("/api/v1/execution/:id/results/csv", get(csv))
            .route("/api/v1/execution/:id/results", get(results))
            .with_state(state.clone())
            .layer(axum::middleware::from_fn_with_state(state.clone(), record));

        let (url, _server) = spawn_router(router).await;
        Self {
            url,
            state,
            _server,
        }
    }
}

/// Shared state for the Helius enhanced API mock.
#[derive(Default)]
pub struct HeliusApiState {
    /// Transactions returned by `/addresses/{wallet}/transactions`.
    pub transactions: Vec<Value>,
    /// Empty array returned when `transactions` is empty and this is true.
    pub next_webhook_id: usize,
    /// When set, `/addresses/{wallet}/transactions` returns 500.
    pub fail_transactions: bool,
}

/// REST mock of the Helius enhanced API (transactions + webhooks).
pub struct HeliusApiMock {
    pub url: String,
    pub state: Arc<Mutex<HeliusApiState>>,
    _server: MockServer,
}

impl HeliusApiMock {
    pub async fn spawn() -> Self {
        let state = Arc::new(Mutex::new(HeliusApiState {
            next_webhook_id: 1,
            ..Default::default()
        }));

        async fn wallet_transactions(
            State(state): State<Arc<Mutex<HeliusApiState>>>,
            Path(wallet): Path<String>,
        ) -> (StatusCode, String) {
            let _ = wallet;
            let st = state.lock().await;
            if st.fail_transactions {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "transactions exploded".to_string(),
                )
            } else {
                (
                    StatusCode::OK,
                    serde_json::to_string(&st.transactions).unwrap(),
                )
            }
        }

        async fn register_webhook(
            State(state): State<Arc<Mutex<HeliusApiState>>>,
        ) -> (StatusCode, String) {
            let mut st = state.lock().await;
            st.next_webhook_id += 1;
            (
                StatusCode::OK,
                json!({"webhookID": format!("mock-webhook-{}", st.next_webhook_id - 1)})
                    .to_string(),
            )
        }

        async fn get_webhook(
            State(state): State<Arc<Mutex<HeliusApiState>>>,
            Path(id): Path<String>,
        ) -> (StatusCode, String) {
            let _ = state;
            (
                StatusCode::OK,
                json!({"webhookID": id, "webhookURL": "https://example.invalid/webhook", "accountAddresses": []}).to_string(),
            )
        }

        async fn update_webhook(
            State(_state): State<Arc<Mutex<HeliusApiState>>>,
            Path(_id): Path<String>,
        ) -> StatusCode {
            StatusCode::OK
        }

        async fn delete_webhook(
            State(_state): State<Arc<Mutex<HeliusApiState>>>,
            Path(_id): Path<String>,
        ) -> StatusCode {
            StatusCode::OK
        }

        let router = Router::new()
            .route("/addresses/:wallet/transactions", get(wallet_transactions))
            .route("/webhooks", post(register_webhook))
            .route(
                "/webhooks/:id",
                get(get_webhook)
                    .patch(update_webhook)
                    .delete(delete_webhook),
            )
            .with_state(state.clone());

        let (url, _server) = spawn_router(router).await;
        Self {
            url,
            state,
            _server,
        }
    }
}

/// Shared state for the Jupiter quote API mock.
#[derive(Default)]
pub struct JupiterQuoteState {
    pub quote: Value,
    pub fail: bool,
}

/// REST mock of the Jupiter `/quote` API.
pub struct JupiterQuoteMock {
    pub url: String,
    pub state: Arc<Mutex<JupiterQuoteState>>,
    _server: MockServer,
}

impl JupiterQuoteMock {
    pub async fn spawn() -> Self {
        let state = Arc::new(Mutex::new(JupiterQuoteState {
            quote: json!({
                "inputMint": "So11111111111111111111111111111111111111112",
                "inAmount": "1000000000",
                "outAmount": "500000000",
                "slippageBps": 1000,
            }),
            fail: false,
        }));

        async fn quote(
            State(state): State<Arc<Mutex<JupiterQuoteState>>>,
            request: Request<Body>,
        ) -> (StatusCode, String) {
            let _ = request;
            let st = state.lock().await;
            if st.fail {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({"error": "mock failure"}).to_string(),
                )
            } else {
                (StatusCode::OK, st.quote.to_string())
            }
        }

        let router = Router::new()
            .route("/quote", get(quote))
            .with_state(state.clone());
        let (url, _server) = spawn_router(router).await;
        Self {
            url,
            state,
            _server,
        }
    }
}

/// Build a Solana base64 account payload for `getAccountInfo` results.
pub fn base64_account(data: Vec<u8>, owner: &str) -> Value {
    use base64::Engine;
    json!({
        "context": {"slot": 1, "apiVersion": "1.18.1"},
        "value": {
            "data": [base64::engine::general_purpose::STANDARD.encode(&data), "base64"],
            "executable": false,
            "lamports": 2039280,
            "owner": owner,
            "rentEpoch": 0,
        }
    })
}

/// Extract the first signature from a base64-encoded LEGACY transaction wire
/// format (bytes[0] = signature count, bytes[1..65] = first signature). The
/// Solana RPC client rejects `sendTransaction` responses whose signature does
/// not match the submitted transaction, so a mock must return the real one.
pub fn legacy_tx_signature_from_params(params: &Value) -> Option<String> {
    use base64::Engine;
    let wire = params.get(0)?.as_str()?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(wire)
        .ok()?;
    if bytes.len() < 65 || bytes[0] != 1 {
        return None;
    }
    Some(bs58::encode(&bytes[1..65]).into_string())
}
