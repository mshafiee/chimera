//! Middleware for Chimera Operator
//!
//! Provides security middleware for webhook verification and API authentication

mod auth;
mod hmac;
mod rate_limit;

pub use auth::{
    bearer_auth, get_auth_user, require_role, AuthExtension, AuthState, AuthenticatedUser, Role,
};
pub use hmac::{hmac_verify, HmacState, SIGNATURE_HEADER, TIMESTAMP_HEADER};
pub use rate_limit::ProxyAwareKeyExtractor;
