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

/// Pin the JSON-RPC URL format: must target mainnet.helius-rpc.com with API key.
#[test]
fn helius_rpc_url_targets_mainnet_rpc_host_not_das_api() {
    let url = helius_rpc_url("test-key-123");
    assert!(
        url.starts_with("https://mainnet.helius-rpc.com"),
        "RPC URL must target mainnet.helius-rpc.com for JSON-RPC methods, got: {url}"
    );
    assert!(
        !url.contains("api.helius.xyz"),
        "RPC URL must NOT use the DAS API host (api.helius.xyz), got: {url}"
    );
    assert!(
        url.contains("api-key=test-key-123"),
        "RPC URL must include the API key, got: {url}"
    );
}

/// The RPC and DAS helpers must produce different hosts. If they ever converge
/// (e.g. someone overrides both env vars to the same value, or changes the
/// defaults), `verify_signature_exists` would silently start hitting the DAS
/// endpoint and every webhook would be rejected with HTTP 404.
#[test]
fn rpc_url_and_das_base_url_target_different_hosts() {
    let rpc_url = helius_rpc_url("key");
    let das_base = helius_api_base_url();
    assert!(
        !rpc_url.starts_with(&das_base),
        "RPC URL ({rpc_url}) must NOT share the DAS base host ({das_base}); \
         verify_signature_exists requires a different host than DAS endpoints"
    );
}
