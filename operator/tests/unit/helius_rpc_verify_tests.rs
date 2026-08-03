//! Tests for the RPC endpoint used by `verify_signature_exists`.
//!
//! The verification method must POST `getTransaction` to the Solana JSON-RPC
//! host (`mainnet.helius-rpc.com`), NOT to the DAS/Enhanced API host
//! (`api.helius.xyz/v0`). The latter returns HTTP 404 for JSON-RPC methods.
//!
//! `verify_signature_exists` uses `helius_rpc_url()` for the request URL. The
//! rest of `HeliusClient` uses `self.base_url` (from `helius_api_base_url()`)
//! for DAS endpoints. These two helpers must never resolve to the same host.

use chimera_operator::utils::{helius_api_base_url, helius_rpc_url};
use url::Url;

/// Pin the JSON-RPC URL format: must target mainnet.helius-rpc.com with API key.
#[test]
fn helius_rpc_url_targets_mainnet_rpc_host_not_das_api() {
    let url = Url::parse(&helius_rpc_url("test-key-123")).expect("valid URL");
    assert_eq!(url.scheme(), "https", "RPC URL must use https, got: {url}");
    assert_eq!(
        url.host_str(),
        Some("mainnet.helius-rpc.com"),
        "RPC URL must target mainnet.helius-rpc.com for JSON-RPC methods, got: {url}"
    );
    assert_ne!(
        url.host_str(),
        Some("api.helius.xyz"),
        "RPC URL must NOT use the DAS API host (api.helius.xyz), got: {url}"
    );
    assert!(
        url.query().unwrap_or_default().contains("api-key=test-key-123"),
        "RPC URL must include the API key, got: {url}"
    );
}

/// The RPC and DAS helpers must produce different hosts. If they ever converge
/// (e.g. someone overrides both env vars to the same value, or changes the
/// defaults), `verify_signature_exists` would silently start hitting the DAS
/// endpoint and every webhook would be rejected with HTTP 404.
#[test]
fn rpc_url_and_das_base_url_target_different_hosts() {
    let rpc = Url::parse(&helius_rpc_url("key")).expect("valid RPC URL");
    let das = Url::parse(&helius_api_base_url()).expect("valid DAS base URL");
    assert_ne!(
        rpc.host_str(),
        das.host_str(),
        "RPC URL ({rpc}) must NOT share the DAS base host ({das}); \
         verify_signature_exists requires a different host than DAS endpoints"
    );
}
