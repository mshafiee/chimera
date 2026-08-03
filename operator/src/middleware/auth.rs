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

/// Stable non-secret alias for an API key, safe to write to logs/audit trails
/// (mirrors the hashing used by the WebSocket auth path).
fn token_short_hash(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(&hasher.finalize()[..8])
}

/// Percent-decode a single query value (`%XX` and `+` → space). Splitting on
/// `&` happens before decoding, so an encoded `&` (`%26`) inside a value is
/// preserved and decodes correctly.
fn url_decode_value(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let hex_val = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    };
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                match (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push(h * 16 + l);
                        i += 3;
                    }
                    _ => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Authentication state
#[derive(Clone)]
pub struct AuthState {
    /// In-memory cache of API keys to roles
    api_keys: Arc<RwLock<HashMap<String, Role>>>,
    /// Secret for verifying JWT tokens
    jwt_secret: String,
    /// Whether to allow unauthenticated readonly access
    pub allow_anonymous_readonly: bool,
}

impl AuthState {
    /// Create a new auth state
    pub fn new(jwt_secret: String) -> Self {
        Self {
            api_keys: Arc::new(RwLock::new(HashMap::new())),
            jwt_secret,
            allow_anonymous_readonly: false,
        }
    }

    /// Create auth state with pre-configured API keys
    pub fn with_auth_config(api_keys: HashMap<String, Role>, jwt_secret: String) -> Self {
        Self {
            api_keys: Arc::new(RwLock::new(api_keys)),
            jwt_secret,
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

    /// Authenticate a token (tries API key first, then JWT).
    ///
    /// Raw wallet addresses are NOT accepted as Bearer tokens — they are public
    /// information and would allow any observer to spoof an admin session.
    /// All wallet-based sessions must go through /auth/wallet to obtain a JWT.
    pub async fn authenticate(&self, token: &str) -> Option<AuthenticatedUser> {
        // First check in-memory API keys (high-entropy random strings, not wallet addresses).
        // The identifier must NOT embed the raw key — it is written to logs and
        // audit trails, so only a non-secret hash alias is stored.
        if let Some(role) = self.check_api_key(token).await {
            return Some(AuthenticatedUser {
                identifier: format!("api_key:{}", token_short_hash(token)),
                role,
            });
        }

        // Try to decode as JWT
        use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};

        // Define minimal claims struct for verification
        #[derive(Debug, Deserialize)]
        struct Claims {
            sub: String,
            role: String,
            // exp field is validated automatically by jsonwebtoken
        }

        let validation = Validation::new(Algorithm::HS256);
        match decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.jwt_secret.as_bytes()),
            &validation,
        ) {
            Ok(token_data) => {
                if let Ok(role) = token_data.claims.role.parse::<Role>() {
                    return Some(AuthenticatedUser {
                        identifier: token_data.claims.sub,
                        role,
                    });
                }
            }
            Err(_) => {
                // Not a valid JWT or signature mismatch
            }
        }

        None
    }
}

/// Extension to store authenticated user in request
#[derive(Clone)]
pub struct AuthExtension(pub AuthenticatedUser);

/// Bearer token authentication middleware
///
/// Extracts Bearer token from Authorization header or query parameter and validates against
/// configured API keys and admin wallets loaded from config.
///
/// ⚠️  **SECURITY WARNING: Query Parameter Authentication**
///
/// This middleware supports bearer tokens via URL query parameter (?token=xyz) for WebSocket
/// connections where custom headers cannot be sent during the handshake. This approach has
/// significant security implications:
///
/// **Risks:**
/// - Tokens are logged in web server access logs (Apache, Nginx, HAProxy)
/// - Tokens appear in proxy logs and intermediate hop logs
/// - Tokens are stored in browser history
/// - Tokens may be exposed in Referer headers when navigating to external sites
/// - Logs retention policies may keep tokens for months/years
/// - Log aggregation systems may distribute tokens to multiple systems
///
/// **Impact:**
/// If logs are compromised, leaked, or accidentally exposed, attackers gain valid bearer tokens
/// that can be used to authenticate as the compromised user until the token expires.
///
/// **Mitigation Strategies:**
/// 1. **Prefer header-based auth:** Always use Authorization header when possible
/// 2. **Secure logs:** Ensure access logs are protected, encrypted, and have short retention
/// 3. **Log sanitization:** Configure web servers to redact query parameters from logs
/// 4. **Short-lived tokens:** Use tokens with minimal TTL (minutes, not days)
/// 5. **Monitor logs:** Audit who has access to logs and review access patterns
/// 6. **Alternative for WebSocket:** Consider using Sec-WebSocket-Protocol subprotocol for token transmission
///
/// **Example Log Sanitization (Nginx):**
/// ```nginx
/// server {
///     # Redact token parameter from logs
///     if ($args ~* "(^|&)token=") {
///         set $args_redacted $args;
///         rewrite ^(.*)$ $1? permanent;
///     }
/// }
/// ```
///
/// **Example Log Sanitization (HAProxy):**
/// ```haproxy
/// # Log only the path, not query parameters
/// option httplog
/// log-format ${[capture.req.hdr(0)]}
/// http-request capture-uri base 1000
/// ```
///
/// **Future Improvements:**
/// - Implement Sec-WebSocket-Protocol subprotocol authentication
/// - Use one-time upgrade tokens with very short TTL
/// - Consider cookie-based authentication with HttpOnly, Secure flags
///
/// **Current Trade-off:**
/// WebSocket connections cannot send custom headers during the initial handshake in browser
/// environments. The query parameter approach is a pragmatic compromise but requires
/// stringent log security practices.
///
/// On success, adds AuthExtension to request for downstream handlers.
pub async fn bearer_auth(
    State(state): State<Arc<AuthState>>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Response {
    // Try to extract token from Authorization header first
    let token = if let Some(header) = headers.get(AUTHORIZATION) {
        match header.to_str() {
            Ok(s) => {
                // RFC 7235: the auth scheme is a case-insensitive token
                match s.split_once(' ') {
                    Some((scheme, t)) if scheme.eq_ignore_ascii_case("bearer") => {
                        Some(t.trim().to_string())
                    }
                    Some(_) => {
                        return auth_error(
                            StatusCode::BAD_REQUEST,
                            "Authorization header must use Bearer scheme",
                        )
                    }
                    None => {
                        return auth_error(
                            StatusCode::BAD_REQUEST,
                            "Authorization header must use Bearer scheme",
                        )
                    }
                }
            }
            Err(_) => {
                return auth_error(
                    StatusCode::BAD_REQUEST,
                    "Invalid Authorization header encoding",
                );
            }
        }
    } else {
        // No Authorization header - try query parameter (for WebSocket)
        None
    };

    // Track authentication method for security monitoring
    let mut is_query_auth = false;

    // If no token in header, check query parameters
    let token = if token.is_some() {
        token
    } else {
        // SECURITY WARNING: Extracting token from query parameters for WebSocket authentication
        //
        // This is necessary because browser WebSocket API doesn't support custom headers in
        // the initial handshake. However, this means:
        // - Tokens appear in server access logs
        // - Tokens appear in proxy logs (HAProxy, Nginx, load balancers)
        // - Tokens are stored in browser history
        // - Tokens may leak via Referer headers
        //
        // Mitigation required:
        // 1. Configure log sanitization to redact query parameters
        // 2. Use short-lived tokens (minutes, not days)
        // 3. Restrict log access and implement log retention policies
        // 4. Monitor for log exposure incidents
        //
        // See function documentation for detailed security analysis and examples.
        //
        // NOTE: Never log the URI, query string, or extracted token — the token
        // travels in the query string and logging it would defeat the mitigations
        // above. At most log that query auth was attempted.
        tracing::debug!("Query parameter authentication attempted");

        let query_token = request.uri().query().and_then(|query_str| {
            // Values are percent-decoded; an encoded `&` inside a token is
            // preserved because splitting happens before decoding.
            for pair in query_str.split('&') {
                if let Some((key, value)) = pair.split_once('=') {
                    if key == "token" {
                        return Some(url_decode_value(value));
                    }
                }
            }
            None
        });

        // SECURITY: Track if we're using query parameter auth (less secure)
        is_query_auth = query_token.is_some();

        if query_token.is_none() || query_token.as_ref().is_none_or(|t| t.is_empty()) {
            // No auth in header or query - check if anonymous readonly is allowed
            if state.allow_anonymous_readonly {
                let anon_user = AuthenticatedUser {
                    identifier: "anonymous".to_string(),
                    role: Role::Readonly,
                };
                request.extensions_mut().insert(AuthExtension(anon_user));
                return next.run(request).await;
            }
            return auth_error(StatusCode::UNAUTHORIZED, "Missing authentication token");
        }

        query_token
    };

    let token_str = match token.as_ref() {
        Some(t) if !t.is_empty() => t,
        _ => return auth_error(StatusCode::UNAUTHORIZED, "Missing authentication token"),
    };

    // Authenticate
    match state.authenticate(token_str).await {
        Some(user) => {
            // Log authentication method for security monitoring
            if is_query_auth {
                tracing::warn!(
                    identifier = %user.identifier,
                    role = %user.role,
                    "User authenticated via QUERY PARAMETER (security risk - token may be in logs)"
                );
            } else {
                tracing::debug!(
                    identifier = %user.identifier,
                    role = %user.role,
                    "User authenticated via Authorization header (secure)"
                );
            }

            request.extensions_mut().insert(AuthExtension(user));
            next.run(request).await
        }
        None => {
            tracing::warn!(
                token_hash = %token_short_hash(token_str),
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
pub fn require_role(
    required: Role,
) -> impl Fn(Request, Next) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>
       + Clone {
    move |request: Request, next: Next| {
        let required = required;
        Box::pin(async move {
            // Get authenticated user from extensions
            let user = match request.extensions().get::<AuthExtension>() {
                Some(AuthExtension(user)) => user.clone(),
                None => {
                    return auth_error(StatusCode::UNAUTHORIZED, "Authentication required");
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
    request
        .extensions()
        .get::<AuthExtension>()
        .map(|ext| &ext.0)
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
