//! Jupiter API credential & request helpers.
//!
//! Jupiter now requires an `x-api-key` header on all endpoints (keyless access
//! being phased out). Rather than threading the key through every constructor
//! (including the dozens of `PriceCache`/`TokenMetadataFetcher` test sites), the
//! key is installed once at startup into a process-global `OnceLock` and attached
//! to every Jupiter request via [`with_api_key`].
//!
//! The key is sourced from config (`jupiter.api_key`, e.g. env
//! `CHIMERA_JUPITER__API_KEY`). In `Live` trade mode a missing key is a
//! hard startup error (see `AppConfig::validate`).

use std::sync::OnceLock;

static API_KEY: OnceLock<Option<String>> = OnceLock::new();

/// Install the Jupiter API key for the process. Called exactly once at startup.
///
/// Subsequent calls are ignored (the first value wins) to mirror how a
/// deployment credential is bound before any worker touches the network.
/// Returns `true` if this call installed the key, `false` if a key was already
/// installed (a conflicting later call is rejected).
pub fn set_api_key(key: Option<String>) -> bool {
    API_KEY.set(key).is_ok()
}

/// Returns the installed Jupiter API key, if any.
pub fn api_key() -> Option<&'static str> {
    API_KEY.get().and_then(|k| k.as_deref())
}

/// Attach the `x-api-key` header (if a key is installed) to a request builder.
///
/// Callers should wrap every request they send to a `*.jup.ag` endpoint:
///
/// ```ignore
/// let resp = jupiter::with_api_key(self.http_client.get(&url)).send().await?;
/// ```
pub fn with_api_key(rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match api_key() {
        Some(k) => rb.header("x-api-key", k),
        None => rb,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The process-global key is installed by [`set_api_key`] and can never be
    /// reset (and tests run in parallel), so exactly ONE test may install the
    /// key and no test may assert on the global state afterwards.
    #[test]
    fn with_api_key_attaches_configured_key() {
        // This is the only test in the crate that installs a key, so the
        // first-wins set must succeed deterministically.
        assert!(
            set_api_key(Some("test-jupiter-key".to_string())),
            "set_api_key must install the key on the first call"
        );
        // A conflicting second installation is rejected (first-wins semantics).
        assert!(
            !set_api_key(Some("different-key".to_string())),
            "a later conflicting set_api_key call must be rejected"
        );

        let client = reqwest::Client::new();
        let rb = with_api_key(client.get("https://api.jup.ag/price/v3?ids=x"));
        let req = rb.build().expect("request must build");
        assert_eq!(
            req.headers().get("x-api-key").and_then(|v| v.to_str().ok()),
            Some("test-jupiter-key")
        );
    }
}
