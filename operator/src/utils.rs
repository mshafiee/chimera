use rust_decimal::prelude::*;

use crate::error::AppError;

/// Helius API base URL from env var with fallback
pub fn helius_api_base_url() -> String {
    std::env::var("HELIUS_API_BASE_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "https://api.helius.xyz/v0".into())
}

/// Helius **Solana RPC** endpoint (with the API key), used for JSON-RPC bundle
/// methods (`sendBundle`, `getBundleStatuses`). Per Helius docs these live at
/// the RPC host (`mainnet.helius-rpc.com`), NOT at `api.helius.xyz/v0`.
/// Overridable via `HELIUS_RPC_BASE_URL`.
pub fn helius_rpc_url(api_key: &str) -> String {
    let base = std::env::var("HELIUS_RPC_BASE_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "https://mainnet.helius-rpc.com".into());
    format!("{}?api-key={}", base, api_key)
}

/// Safely convert SOL (Decimal) to Lamports (u64) using Decimal to avoid precision loss
pub fn sol_to_lamports(sol: Decimal) -> Result<u64, AppError> {
    // 1 SOL = 1,000,000,000 Lamports
    let multiplier = Decimal::new(1_000_000_000, 0);
    let result = sol * multiplier;

    result.to_u64().ok_or_else(|| {
        AppError::InvalidInput(format!(
            "Decimal conversion overflow: {} SOL exceeds u64 max",
            sol
        ))
    })
}

/// Safely convert SOL (f64) to Lamports (u64) using Decimal to avoid precision loss
/// This is a convenience function for legacy code that still uses f64
pub fn sol_to_lamports_f64(sol: f64) -> Result<u64, AppError> {
    // Convert float to Decimal first to handle precision safely
    let sol_decimal = Decimal::from_f64_retain(sol).unwrap_or(Decimal::ZERO);
    sol_to_lamports(sol_decimal)
}

/// Safely convert Lamports (u64) to SOL (f64) for display/DB
pub fn lamports_to_sol(lamports: u64) -> f64 {
    let lamports_dec = Decimal::from(lamports);
    let divisor = Decimal::new(1_000_000_000, 0);

    (lamports_dec / divisor).to_f64().unwrap_or(0.0)
}
