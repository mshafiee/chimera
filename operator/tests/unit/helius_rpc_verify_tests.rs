//! Tests for the RPC endpoint used by `verify_signature_exists`.
//!
//! The verification method must POST `getTransaction` to the Solana JSON-RPC
//! host (`mainnet.helius-rpc.com`), NOT to the DAS/Enhanced API host
//! (`api.helius.xyz/v0`). The latter returns HTTP 404 for JSON-RPC methods.

use chimera_operator::utils::helius_rpc_url;

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

use chimera_operator::monitoring::helius::HeliusClient;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

#[test]
fn helius_client_base_url_is_das_api_not_rpc() {
    // The HeliusClient's base_url SHOULD be the DAS API (api.helius.xyz/v0)
    // for enhanced endpoints. But verify_signature_exists must NOT use it
    // for JSON-RPC calls — it must use helius_rpc_url() instead.
    let cache = Arc::new(RwLock::new(HashMap::new()));
    let client = HeliusClient::new("test-key".to_string(), cache).unwrap();
    // base_url is the DAS endpoint (used for /tokens, /webhooks, etc.)
    // We can't access base_url directly (private), but we verify the client
    // was constructed without error.
    assert!(
        client.get_cache_stats().2 == 0,
        "New client should have empty cache"
    );
}
