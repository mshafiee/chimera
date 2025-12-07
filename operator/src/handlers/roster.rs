//! Roster management endpoints

use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

use crate::db::DbPool;
use crate::error::AppError;
use crate::roster;

/// Roster merge request
#[derive(Debug, Deserialize)]
pub struct MergeRequest {
    /// Optional custom path to roster file (defaults to roster_new.db)
    #[serde(default)]
    pub roster_path: Option<String>,
}

/// Roster merge response
#[derive(Debug, Serialize)]
pub struct MergeResponse {
    /// Whether merge was successful
    pub success: bool,
    /// Number of wallets merged
    pub wallets_merged: u32,
    /// Number of wallets that were replaced
    pub wallets_removed: u32,
    /// Any warnings during merge
    pub warnings: Vec<String>,
    /// Error message if failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// State for roster endpoints
pub struct RosterState {
    /// Database pool
    pub db: DbPool,
    /// Default roster path
    pub default_roster_path: PathBuf,
}

/// Trigger roster merge
///
/// POST /api/v1/roster/merge
///
/// Requires admin authentication (to be implemented via middleware).
/// Merges wallets from roster_new.db into the main database.
pub async fn roster_merge(
    State(state): State<Arc<RosterState>>,
    Json(request): Json<MergeRequest>,
) -> Result<(StatusCode, Json<MergeResponse>), AppError> {
    let roster_path = request
        .roster_path
        .map(PathBuf::from)
        .unwrap_or_else(|| state.default_roster_path.clone());

    tracing::info!(
        roster_path = %roster_path.display(),
        "Manual roster merge triggered"
    );

    match roster::merge_roster(&state.db, &roster_path).await {
        Ok(result) => {
            tracing::info!(
                wallets_merged = result.wallets_merged,
                wallets_removed = result.wallets_removed,
                "Manual roster merge completed"
            );

            Ok((
                StatusCode::OK,
                Json(MergeResponse {
                    success: true,
                    wallets_merged: result.wallets_merged,
                    wallets_removed: result.wallets_removed,
                    warnings: result.warnings,
                    error: None,
                }),
            ))
        }
        Err(e) => {
            tracing::error!(error = %e, "Manual roster merge failed");

            Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(MergeResponse {
                    success: false,
                    wallets_merged: 0,
                    wallets_removed: 0,
                    warnings: vec![],
                    error: Some(e.to_string()),
                }),
            ))
        }
    }
}

/// Validate roster file
///
/// GET /api/v1/roster/validate
///
/// Checks if the roster_new.db file exists and passes integrity check.
#[derive(Debug, Serialize)]
pub struct ValidateResponse {
    /// Whether roster file is valid
    pub valid: bool,
    /// Whether file exists
    pub exists: bool,
    /// File path checked
    pub path: String,
    /// Error message if invalid
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub async fn roster_validate(
    State(state): State<Arc<RosterState>>,
) -> Result<(StatusCode, Json<ValidateResponse>), AppError> {
    let roster_path = &state.default_roster_path;
    let exists = roster_path.exists();

    if !exists {
        return Ok((
            StatusCode::OK,
            Json(ValidateResponse {
                valid: false,
                exists: false,
                path: roster_path.display().to_string(),
                error: Some("Roster file does not exist".to_string()),
            }),
        ));
    }

    match roster::validate_roster(&state.db, roster_path).await {
        Ok(valid) => Ok((
            StatusCode::OK,
            Json(ValidateResponse {
                valid,
                exists: true,
                path: roster_path.display().to_string(),
                error: if valid {
                    None
                } else {
                    Some("Integrity check failed".to_string())
                },
            }),
        )),
        Err(e) => Ok((
            StatusCode::OK,
            Json(ValidateResponse {
                valid: false,
                exists: true,
                path: roster_path.display().to_string(),
                error: Some(e.to_string()),
            }),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_response_serialization() {
        let response = MergeResponse {
            success: true,
            wallets_merged: 10,
            wallets_removed: 5,
            warnings: vec!["test warning".to_string()],
            error: None,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(!json.contains("error")); // Should be skipped when None
    }
}
