//! Error types for Chimera Operator

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

/// Application-level errors
#[derive(Error, Debug)]
pub enum AppError {
    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(#[from] config::ConfigError),

    /// Database error
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    /// Validation error
    #[error("Validation error: {0}")]
    Validation(String),

    /// Authentication error
    #[error("Authentication failed: {0}")]
    Auth(String),

    /// Authorization error (authenticated but insufficient permissions)
    #[error("Authorization failed: {0}")]
    Forbidden(String),

    /// Not found error
    #[error("Not found: {0}")]
    NotFound(String),

    /// Signal processing error
    #[error("Signal error: {0}")]
    Signal(String),

    /// RPC/Solana error
    #[error("RPC error: {0}")]
    Rpc(String),

    /// Queue error
    #[error("Queue error: {0}")]
    Queue(String),

    /// Circuit breaker triggered
    #[error("Circuit breaker triggered: {0}")]
    CircuitBreaker(String),

    /// Duplicate signal (idempotency check)
    #[error("Duplicate signal: {0}")]
    Duplicate(String),

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Error response structure for API
#[derive(Debug, serde::Serialize)]
pub struct ErrorResponse {
    pub status: &'static str,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status_code, error_response) = match &self {
            AppError::Config(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse {
                    status: "error",
                    reason: "configuration_error".to_string(),
                    details: Some(e.to_string()),
                },
            ),
            AppError::Database(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse {
                    status: "error",
                    reason: "database_error".to_string(),
                    details: Some(e.to_string()),
                },
            ),
            AppError::Validation(msg) => (
                StatusCode::BAD_REQUEST,
                ErrorResponse {
                    status: "rejected",
                    reason: "validation_failed".to_string(),
                    details: Some(msg.clone()),
                },
            ),
            AppError::Auth(msg) => (
                StatusCode::UNAUTHORIZED,
                ErrorResponse {
                    status: "rejected",
                    reason: "authentication_failed".to_string(),
                    details: Some(msg.clone()),
                },
            ),
            AppError::Forbidden(msg) => (
                StatusCode::FORBIDDEN,
                ErrorResponse {
                    status: "rejected",
                    reason: "authorization_failed".to_string(),
                    details: Some(msg.clone()),
                },
            ),
            AppError::NotFound(msg) => (
                StatusCode::NOT_FOUND,
                ErrorResponse {
                    status: "rejected",
                    reason: "not_found".to_string(),
                    details: Some(msg.clone()),
                },
            ),
            AppError::Signal(msg) => (
                StatusCode::BAD_REQUEST,
                ErrorResponse {
                    status: "rejected",
                    reason: "invalid_signal".to_string(),
                    details: Some(msg.clone()),
                },
            ),
            AppError::Rpc(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorResponse {
                    status: "error",
                    reason: "rpc_error".to_string(),
                    details: Some(msg.clone()),
                },
            ),
            AppError::Queue(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorResponse {
                    status: "rejected",
                    reason: "queue_full".to_string(),
                    details: Some(msg.clone()),
                },
            ),
            AppError::CircuitBreaker(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorResponse {
                    status: "rejected",
                    reason: "circuit_breaker_triggered".to_string(),
                    details: Some(msg.clone()),
                },
            ),
            AppError::Duplicate(trade_uuid) => (
                StatusCode::CONFLICT,
                ErrorResponse {
                    status: "rejected",
                    reason: "duplicate_signal".to_string(),
                    details: Some(format!("Trade UUID already exists: {}", trade_uuid)),
                },
            ),
            AppError::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse {
                    status: "error",
                    reason: "internal_error".to_string(),
                    details: Some(msg.clone()),
                },
            ),
        };

        // Log the error
        tracing::error!(
            error_type = %self,
            status_code = %status_code,
            "Request error"
        );

        (status_code, Json(json!(error_response))).into_response()
    }
}

/// Result type alias for convenience
pub type AppResult<T> = Result<T, AppError>;
