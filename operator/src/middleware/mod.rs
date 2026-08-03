//! Middleware for Chimera Operator
//!
//! Provides security middleware for webhook verification and API authentication

mod auth;
mod hmac;
mod rate_limit;

pub use auth::{AuthExtension, AuthState, AuthenticatedUser, Role, bearer_auth, get_auth_user, require_role};
pub use hmac::{HmacState, SIGNATURE_HEADER, TIMESTAMP_HEADER, hmac_verify};
pub use rate_limit::ProxyAwareKeyExtractor;
