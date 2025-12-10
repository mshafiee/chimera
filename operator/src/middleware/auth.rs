//! Bearer token authentication middleware
//!
//! Provides role-based access control for API endpoints.
//!
//! Roles:
//! - `readonly`: View dashboard, positions, trades
//! - `operator`: Promote/demote wallets, view config
//! - `admin`: Full access including config changes, circuit breaker resets
//!
//! Authentication methods:
//! - Bearer token in Authorization header
//! - API keys and admin wallets loaded from config into memory

use axum::{
    extract::{Request, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;


/// User roles for authorization
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// View-only access: dashboard, positions, trades
    Readonly,
    /// Operator access: promote/demote wallets, view config
    Operator,
    /// Full admin access: config changes, circuit breaker resets
    Admin,
}

impl Role {
    /// Check if this role has at least the required permission level
    pub fn has_permission(&self, required: Role) -> bool {
        *self >= required
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::Readonly => write!(f, "readonly"),
            Role::Operator => write!(f, "operator"),
            Role::Admin => write!(f, "admin"),
        }
    }
}

impl std::str::FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "readonly" => Ok(Role::Readonly),
            "operator" => Ok(Role::Operator),
            "admin" => Ok(Role::Admin),
            _ => Err(format!("Unknown role: {}", s)),
        }
    }
}

/// Authenticated user information
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    /// The API key or wallet address used to authenticate
    pub identifier: String,
    /// The user's role
    pub role: Role,
}

/// Authentication state
#[derive(Clone)]
pub struct AuthState {
    /// In-memory cache of API keys to roles
    api_keys: Arc<RwLock<HashMap<String, Role>>>,
    /// In-memory cache of admin wallets to roles
    admin_wallets: Arc<RwLock<HashMap<String, Role>>>,
    /// Whether to allow unauthenticated readonly access
    pub allow_anonymous_readonly: bool,
}

impl AuthState {
    /// Create a new auth state
    pub fn new() -> Self {
        Self {
            api_keys: Arc::new(RwLock::new(HashMap::new())),
            admin_wallets: Arc::new(RwLock::new(HashMap::new())),
            allow_anonymous_readonly: false,
        }
    }

    /// Create auth state with pre-configured API keys and admin wallets
    pub fn with_auth_config(
        api_keys: HashMap<String, Role>,
        admin_wallets: HashMap<String, Role>,
    ) -> Self {
        Self {
            api_keys: Arc::new(RwLock::new(api_keys)),
            admin_wallets: Arc::new(RwLock::new(admin_wallets)),
            allow_anonymous_readonly: false,
        }
    }

    /// Add an API key at runtime
    pub async fn add_api_key(&self, key: String, role: Role) {
        let mut keys = self.api_keys.write().await;
        keys.insert(key, role);
    }

    /// Remove an API key at runtime
    pub async fn remove_api_key(&self, key: &str) {
        let mut keys = self.api_keys.write().await;
        keys.remove(key);
    }

    /// Check API key in memory cache
    async fn check_api_key(&self, key: &str) -> Option<Role> {
        let keys = self.api_keys.read().await;
        keys.get(key).copied()
    }

    /// Check wallet address in memory cache
    async fn check_admin_wallet(&self, address: &str) -> Option<Role> {
        let wallets = self.admin_wallets.read().await;
        wallets.get(address).copied()
    }

    /// Authenticate a token (tries API key first, then admin_wallets)
    pub async fn authenticate(&self, token: &str) -> Option<AuthenticatedUser> {
        // First check in-memory API keys
        if let Some(role) = self.check_api_key(token).await {
            return Some(AuthenticatedUser {
                identifier: token.to_string(),
                role,
            });
        }

        // Then check in-memory admin wallets (token could be a wallet address)
        if let Some(role) = self.check_admin_wallet(token).await {
            return Some(AuthenticatedUser {
                identifier: token.to_string(),
                role,
            });
        }

        None
    }
}

/// Extension to store authenticated user in request
#[derive(Clone)]
pub struct AuthExtension(pub AuthenticatedUser);

/// Bearer token authentication middleware
///
/// Extracts Bearer token from Authorization header and validates against
/// configured API keys and admin wallets loaded from config.
///
/// On success, adds AuthExtension to request for downstream handlers.
pub async fn bearer_auth(
    State(state): State<Arc<AuthState>>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Response {
    // Extract Authorization header
    let auth_header = match headers.get(AUTHORIZATION) {
        Some(header) => match header.to_str() {
            Ok(s) => s,
            Err(_) => {
                return auth_error(StatusCode::BAD_REQUEST, "Invalid Authorization header encoding");
            }
        },
        None => {
            // No auth header - check if anonymous readonly is allowed
            if state.allow_anonymous_readonly {
                let anon_user = AuthenticatedUser {
                    identifier: "anonymous".to_string(),
                    role: Role::Readonly,
                };
                request.extensions_mut().insert(AuthExtension(anon_user));
                return next.run(request).await;
            }
            return auth_error(StatusCode::UNAUTHORIZED, "Missing Authorization header");
        }
    };

    // Parse Bearer token
    let token = if auth_header.starts_with("Bearer ") {
        &auth_header[7..]
    } else {
        return auth_error(
            StatusCode::BAD_REQUEST,
            "Authorization header must use Bearer scheme",
        );
    };

    if token.is_empty() {
        return auth_error(StatusCode::BAD_REQUEST, "Bearer token is empty");
    }

    // Authenticate
    match state.authenticate(token).await {
        Some(user) => {
            tracing::debug!(
                identifier = %user.identifier,
                role = %user.role,
                "User authenticated"
            );
            request.extensions_mut().insert(AuthExtension(user));
            next.run(request).await
        }
        None => {
            tracing::warn!(
                token_prefix = %&token[..token.len().min(8)],
                "Authentication failed - invalid token"
            );
            auth_error(StatusCode::UNAUTHORIZED, "Invalid or expired token")
        }
    }
}

/// Middleware that requires a specific minimum role
///
/// Use this after bearer_auth to enforce role requirements.
/// Example: require_role(Role::Admin) for admin-only endpoints.
pub fn require_role(required: Role) -> impl Fn(Request, Next) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>> + Clone {
    move |request: Request, next: Next| {
        let required = required;
        Box::pin(async move {
            // Get authenticated user from extensions
            let user = match request.extensions().get::<AuthExtension>() {
                Some(AuthExtension(user)) => user.clone(),
                None => {
                    return auth_error(
                        StatusCode::UNAUTHORIZED,
                        "Authentication required",
                    );
                }
            };

            // Check role permission
            if !user.role.has_permission(required) {
                tracing::warn!(
                    identifier = %user.identifier,
                    user_role = %user.role,
                    required_role = %required,
                    "Authorization failed - insufficient permissions"
                );
                return auth_error(
                    StatusCode::FORBIDDEN,
                    &format!("Requires {} role or higher", required),
                );
            }

            next.run(request).await
        })
    }
}

/// Create an authentication error response
fn auth_error(status: StatusCode, message: &str) -> Response {
    let body = json!({
        "status": "rejected",
        "reason": if status == StatusCode::FORBIDDEN { "authorization_failed" } else { "authentication_failed" },
        "details": message
    });

    (status, Json(body)).into_response()
}

/// Helper to extract authenticated user from request extensions
pub fn get_auth_user(request: &Request) -> Option<&AuthenticatedUser> {
    request.extensions().get::<AuthExtension>().map(|ext| &ext.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_ordering() {
        assert!(Role::Admin > Role::Operator);
        assert!(Role::Operator > Role::Readonly);
        assert!(Role::Admin > Role::Readonly);
    }

    #[test]
    fn test_role_has_permission() {
        assert!(Role::Admin.has_permission(Role::Readonly));
        assert!(Role::Admin.has_permission(Role::Operator));
        assert!(Role::Admin.has_permission(Role::Admin));

        assert!(Role::Operator.has_permission(Role::Readonly));
        assert!(Role::Operator.has_permission(Role::Operator));
        assert!(!Role::Operator.has_permission(Role::Admin));

        assert!(Role::Readonly.has_permission(Role::Readonly));
        assert!(!Role::Readonly.has_permission(Role::Operator));
        assert!(!Role::Readonly.has_permission(Role::Admin));
    }

    #[test]
    fn test_role_parse() {
        assert_eq!("admin".parse::<Role>().unwrap(), Role::Admin);
        assert_eq!("ADMIN".parse::<Role>().unwrap(), Role::Admin);
        assert_eq!("operator".parse::<Role>().unwrap(), Role::Operator);
        assert_eq!("readonly".parse::<Role>().unwrap(), Role::Readonly);
        assert!("invalid".parse::<Role>().is_err());
    }

    #[test]
    fn test_role_display() {
        assert_eq!(Role::Admin.to_string(), "admin");
        assert_eq!(Role::Operator.to_string(), "operator");
        assert_eq!(Role::Readonly.to_string(), "readonly");
    }
}
