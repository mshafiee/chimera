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
    use axum::{
        body::Body,
        http::{Method, Request as HttpRequest, Uri},
        middleware,
        routing::get,
        Router,
    };
    use jsonwebtoken::{encode, EncodingKey, Header};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tower::ServiceExt;

    // ==========================================================================
    // token_short_hash
    // ==========================================================================

    #[test]
    fn test_token_short_hash_stable_and_non_secret() {
        let h1 = token_short_hash("some-api-key");
        let h2 = token_short_hash("some-api-key");
        assert_eq!(h1, h2, "hash must be deterministic");
        assert_eq!(h1.len(), 16, "8 bytes -> 16 hex chars");
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
        // Different tokens hash differently (collision resistance, 64-bit space)
        assert_ne!(token_short_hash("some-api-key"), token_short_hash("other-key"));
        // Empty token is handled without panicking
        assert_eq!(token_short_hash("").len(), 16);
    }

    // ==========================================================================
    // url_decode_value
    // ==========================================================================

    #[test]
    fn test_url_decode_value() {
        assert_eq!(url_decode_value("abc"), "abc");
        assert_eq!(url_decode_value("a+b+c"), "a b c");
        assert_eq!(url_decode_value("hello%20world"), "hello world");
        assert_eq!(url_decode_value("%26"), "&", "encoded & preserved");
        assert_eq!(url_decode_value("%41%42%43"), "ABC");
        assert_eq!(url_decode_value("%61%62%63"), "abc");
        // Invalid percent sequences pass through unchanged
        assert_eq!(url_decode_value("100%"), "100%");
        assert_eq!(url_decode_value("%zz"), "%zz");
        assert_eq!(url_decode_value("%4"), "%4");
        assert_eq!(url_decode_value("%4G"), "%4G");
        // Trailing single hex digit at the end of the string
        assert_eq!(url_decode_value("x%2"), "x%2");
        // Mixed valid/invalid
        assert_eq!(url_decode_value("a%2Gb"), "a%2Gb");
        // Non-UTF8 decoded bytes fall back to the raw string
        assert_eq!(url_decode_value("%FF%FE"), "%FF%FE");
        // Empty string
        assert_eq!(url_decode_value(""), "");
    }

    // ==========================================================================
    // AuthState key management + authenticate
    // ==========================================================================

    fn make_jwt(sub: &str, role: &str, secret: &str, exp_offset_secs: i64) -> String {
        let exp = (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + exp_offset_secs) as usize;
        let claims = serde_json::json!({
            "sub": sub,
            "role": role,
            "exp": exp,
            "iat": exp - 3600,
        });
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn test_auth_state_key_management() {
        let state = AuthState::new("jwt-secret".to_string());
        assert!(!state.allow_anonymous_readonly);
        assert!(state.authenticate("key1").await.is_none());

        state.add_api_key("key1".to_string(), Role::Admin).await;
        let user = state.authenticate("key1").await.expect("key must authenticate");
        assert_eq!(user.role, Role::Admin);
        assert!(user.identifier.starts_with("api_key:"));
        assert_ne!(user.identifier, "api_key:key1", "raw key must never be the identifier");

        state.remove_api_key("key1").await;
        assert!(state.authenticate("key1").await.is_none());
    }

    #[tokio::test]
    async fn test_auth_state_with_auth_config() {
        let mut keys = HashMap::new();
        keys.insert("read-key".to_string(), Role::Readonly);
        keys.insert("op-key".to_string(), Role::Operator);
        let state = AuthState::with_auth_config(keys, "jwt-secret".to_string());

        assert_eq!(state.authenticate("read-key").await.unwrap().role, Role::Readonly);
        assert_eq!(state.authenticate("op-key").await.unwrap().role, Role::Operator);
        assert!(state.authenticate("unknown").await.is_none());
    }

    #[tokio::test]
    async fn test_authenticate_api_key_takes_precedence_over_jwt() {
        let state = AuthState::new("jwt-secret".to_string());
        // A key that looks like a JWT-ish string is still matched as an API key first
        state.add_api_key("eyJhbGciOiJIUzI1NiJ9.token".to_string(), Role::Readonly).await;
        let user = state.authenticate("eyJhbGciOiJIUzI1NiJ9.token").await.unwrap();
        assert_eq!(user.role, Role::Readonly);
        assert!(user.identifier.starts_with("api_key:"));
    }

    #[tokio::test]
    async fn test_authenticate_valid_jwt() {
        let secret = "super-secret-jwt-key";
        let state = AuthState::new(secret.to_string());
        let token = make_jwt("wallet123", "admin", secret, 3600);
        let user = state.authenticate(&token).await.expect("valid JWT must authenticate");
        assert_eq!(user.identifier, "wallet123");
        assert_eq!(user.role, Role::Admin);
    }

    #[tokio::test]
    async fn test_authenticate_jwt_with_invalid_role() {
        let secret = "super-secret-jwt-key";
        let state = AuthState::new(secret.to_string());
        let token = make_jwt("wallet123", "superuser", secret, 3600);
        // Signature is valid but the role doesn't parse -> overall None
        assert!(state.authenticate(&token).await.is_none());
    }

    #[tokio::test]
    async fn test_authenticate_jwt_wrong_secret() {
        let state = AuthState::new("correct-secret".to_string());
        let token = make_jwt("wallet123", "admin", "wrong-secret", 3600);
        assert!(state.authenticate(&token).await.is_none());
    }

    #[tokio::test]
    async fn test_authenticate_jwt_expired() {
        let secret = "super-secret-jwt-key";
        let state = AuthState::new(secret.to_string());
        let token = make_jwt("wallet123", "admin", secret, -7200);
        assert!(state.authenticate(&token).await.is_none());
    }

    #[tokio::test]
    async fn test_authenticate_garbage_token() {
        let state = AuthState::new("secret".to_string());
        assert!(state.authenticate("not-a-token").await.is_none());
        assert!(state.authenticate("").await.is_none());
    }

    // ==========================================================================
    // bearer_auth middleware (via Router + oneshot)
    // ==========================================================================

    async fn noop_handler(req: HttpRequest<Body>) -> Response {
        // Echo the auth extension into the response so tests can assert the
        // middleware attached the expected identity.
        let mut resp = Response::new(Body::from("ok"));
        if let Some(user) = req.extensions().get::<AuthExtension>() {
            resp.extensions_mut().insert(user.clone());
        }
        resp
    }

    fn auth_router(state: AuthState) -> Router {
        Router::new()
            .route("/protected", get(noop_handler))
            .route_layer(middleware::from_fn_with_state(Arc::new(state), bearer_auth))
    }

    async fn send(router: Router, mut req: HttpRequest<Body>) -> Response {
        req.headers_mut().insert("content-type", "application/json".parse().unwrap());
        router.oneshot(req).await.unwrap()
    }

    fn get_req(path: &str) -> HttpRequest<Body> {
        let uri = if path.is_empty() { "/protected" } else { path };
        HttpRequest::builder()
            .method(Method::GET)
            .uri(Uri::try_from(uri).unwrap())
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn test_bearer_auth_valid_header() {
        let state = AuthState::with_auth_config(
            HashMap::from([("secret-key".to_string(), Role::Operator)]),
            "jwt-secret".to_string(),
        );
        let router = auth_router(state);
        let mut req = get_req("/protected");
        req.headers_mut().insert(
            AUTHORIZATION,
            "Bearer secret-key".parse().unwrap(),
        );
        let resp = send(router, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let user = resp.extensions().get::<AuthExtension>().expect("auth extension present");
        assert_eq!(user.0.role, Role::Operator);
    }

    #[tokio::test]
    async fn test_bearer_auth_bearer_case_insensitive_and_trimmed() {
        let state = AuthState::with_auth_config(
            HashMap::from([("secret-key".to_string(), Role::Readonly)]),
            "jwt-secret".to_string(),
        );
        let router = auth_router(state);
        let mut req = get_req("/protected");
        req.headers_mut().insert(AUTHORIZATION, "bEaReR  secret-key  ".parse().unwrap());
        let resp = send(router, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_bearer_auth_wrong_scheme_rejected() {
        let state = AuthState::with_auth_config(
            HashMap::from([("secret-key".to_string(), Role::Readonly)]),
            "jwt-secret".to_string(),
        );
        let router = auth_router(state);
        let mut req = get_req("/protected");
        req.headers_mut().insert(AUTHORIZATION, "Basic dXNlcjpwYXNz".parse().unwrap());
        let resp = send(router, req).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_bearer_auth_no_space_rejected() {
        let state = AuthState::with_auth_config(HashMap::new(), "jwt-secret".to_string());
        let router = auth_router(state);
        let mut req = get_req("/protected");
        req.headers_mut().insert(AUTHORIZATION, "Bearer".parse().unwrap());
        let resp = send(router, req).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_bearer_auth_missing_token_unauthorized() {
        let state = AuthState::with_auth_config(HashMap::new(), "jwt-secret".to_string());
        let router = auth_router(state);
        let resp = send(router, get_req("/protected")).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_bearer_auth_invalid_token_unauthorized() {
        let state = AuthState::with_auth_config(HashMap::new(), "jwt-secret".to_string());
        let router = auth_router(state);
        let mut req = get_req("/protected");
        req.headers_mut().insert(AUTHORIZATION, "Bearer wrong-key".parse().unwrap());
        let resp = send(router, req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_bearer_auth_anonymous_readonly_allowed() {
        let mut state = AuthState::new("jwt-secret".to_string());
        state.allow_anonymous_readonly = true;
        let router = auth_router(state);
        let resp = send(router, get_req("/protected")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let user = resp.extensions().get::<AuthExtension>().expect("anon extension");
        assert_eq!(user.0.identifier, "anonymous");
        assert_eq!(user.0.role, Role::Readonly);
    }

    #[tokio::test]
    async fn test_bearer_auth_anonymous_readonly_disabled() {
        let state = AuthState::new("jwt-secret".to_string());
        let router = auth_router(state);
        let resp = send(router, get_req("/protected")).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_bearer_auth_query_param_token() {
        let state = AuthState::with_auth_config(
            HashMap::from([("query-key".to_string(), Role::Admin)]),
            "jwt-secret".to_string(),
        );
        let router = auth_router(state);
        let mut req = get_req("/protected?token=query-key");
        req.headers_mut().insert("content-type", "application/json".parse().unwrap());
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let user = resp.extensions().get::<AuthExtension>().expect("auth extension present");
        assert_eq!(user.0.role, Role::Admin);
    }

    #[tokio::test]
    async fn test_bearer_auth_query_param_percent_encoded_token() {
        let state = AuthState::with_auth_config(
            HashMap::from([("a&b c".to_string(), Role::Readonly)]),
            "jwt-secret".to_string(),
        );
        let router = auth_router(state);
        let req = get_req("/protected?token=a%26b%20c");
        let resp = send(router, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_bearer_auth_query_param_empty_token() {
        let state = AuthState::new("jwt-secret".to_string());
        let router = auth_router(state);
        let req = get_req("/protected?token=");
        let resp = send(router, req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_bearer_auth_query_param_wrong_key() {
        let state = AuthState::new("jwt-secret".to_string());
        let router = auth_router(state);
        let req = get_req("/protected?token=nope");
        let resp = send(router, req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_bearer_auth_query_param_ignores_other_params() {
        let state = AuthState::with_auth_config(
            HashMap::from([("real-key".to_string(), Role::Operator)]),
            "jwt-secret".to_string(),
        );
        let router = auth_router(state);
        let req = get_req("/protected?other=1&token=real-key");
        let resp = send(router, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_bearer_auth_header_precedes_query() {
        // Header token wins even when a different query token is present
        let state = AuthState::with_auth_config(
            HashMap::from([
                ("header-key".to_string(), Role::Operator),
                ("query-key".to_string(), Role::Readonly),
            ]),
            "jwt-secret".to_string(),
        );
        let router = auth_router(state);
        let mut req = get_req("/protected?token=query-key");
        req.headers_mut().insert(AUTHORIZATION, "Bearer header-key".parse().unwrap());
        let resp = send(router, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let user = resp.extensions().get::<AuthExtension>().expect("auth extension present");
        assert_eq!(user.0.identifier.starts_with("api_key:"), true);
    }

    #[tokio::test]
    async fn test_bearer_auth_jwt_via_header() {
        let secret = "super-secret-jwt-key";
        let state = AuthState::new(secret.to_string());
        let token = make_jwt("wallet-xyz", "operator", secret, 3600);
        let router = auth_router(state);
        let mut req = get_req("/protected");
        req.headers_mut().insert(AUTHORIZATION, format!("Bearer {}", token).parse().unwrap());
        let resp = send(router, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let user = resp.extensions().get::<AuthExtension>().expect("auth extension present");
        assert_eq!(user.0.identifier, "wallet-xyz");
    }

    // ==========================================================================
    // require_role
    // ==========================================================================

    fn role_router(state: AuthState, required: Role) -> Router {
        Router::new()
            .route("/protected", get(noop_handler))
            .layer(middleware::from_fn(require_role(required)))
            .route_layer(middleware::from_fn_with_state(Arc::new(state), bearer_auth))
    }

    #[tokio::test]
    async fn test_require_role_sufficient() {
        let state = AuthState::with_auth_config(
            HashMap::from([("admin-key".to_string(), Role::Admin)]),
            "jwt-secret".to_string(),
        );
        let router = role_router(state, Role::Operator);
        let mut req = get_req("/protected");
        req.headers_mut().insert(AUTHORIZATION, "Bearer admin-key".parse().unwrap());
        let resp = send(router, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_require_role_insufficient() {
        let state = AuthState::with_auth_config(
            HashMap::from([("read-key".to_string(), Role::Readonly)]),
            "jwt-secret".to_string(),
        );
        let router = role_router(state, Role::Admin);
        let mut req = get_req("/protected");
        req.headers_mut().insert(AUTHORIZATION, "Bearer read-key".parse().unwrap());
        let resp = send(router, req).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_require_role_no_user() {
        let state = AuthState::new("jwt-secret".to_string());
        let router = role_router(state, Role::Readonly);
        let mut req = get_req("/protected");
        // auth layer passes through an anonymous readonly user when allowed,
        // so bypass it entirely: build the router without bearer_auth.
        let router_no_auth = Router::new()
            .route("/protected", get(noop_handler))
            .layer(middleware::from_fn(require_role(Role::Readonly)));
        req.headers_mut().insert("content-type", "application/json".parse().unwrap());
        let resp = router_no_auth.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ==========================================================================
    // auth_error / get_auth_user
    // ==========================================================================

    #[tokio::test]
    async fn test_auth_error_bodies() {
        let forbidden = auth_error(StatusCode::FORBIDDEN, "Requires admin role or higher");
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(forbidden.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["reason"], "authorization_failed");

        let unauthorized = auth_error(StatusCode::UNAUTHORIZED, "Missing authentication token");
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(unauthorized.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["reason"], "authentication_failed");
        assert_eq!(json["details"], "Missing authentication token");
    }

    #[tokio::test]
    async fn test_get_auth_user() {
        let user = AuthenticatedUser {
            identifier: "w".to_string(),
            role: Role::Admin,
        };
        let mut req = get_req("/protected");
        req.extensions_mut().insert(AuthExtension(user.clone()));
        let got = get_auth_user(&req).expect("user present");
        assert_eq!(got.identifier, "w");
        assert_eq!(got.role, Role::Admin);

        let req2 = get_req("/protected");
        assert!(get_auth_user(&req2).is_none());
    }

    #[test]
    fn test_auth_extension_holds_user() {
        let user = AuthenticatedUser {
            identifier: "id".to_string(),
            role: Role::Operator,
        };
        let ext = AuthExtension(user);
        assert_eq!(ext.0.identifier, "id");
    }
}
