/// Solana token mint addresses (shared constants)
/// 
/// These constants are used across both Rust (Operator) and Python (Scout)
/// to ensure consistency. When updating these values, ensure they match
/// the corresponding constants in the Scout codebase.
pub mod mints {
    /// Wrapped SOL (native SOL wrapped as SPL token)
    pub const SOL: &str = "So11111111111111111111111111111111111111112";
    /// USDC (Circle USD Coin)
    pub const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    /// USDT (Tether USD)
    pub const USDT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
}

/// Program IDs
pub mod programs {
    /// Jupiter Aggregator Program ID
    pub const JUPITER: &str = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4";
}

// Legacy constants for backward compatibility
/// @deprecated Use `mints::SOL` instead
pub const SOL_MINT: &str = mints::SOL;
/// @deprecated Use `mints::USDC` instead
pub const USDC_MINT: &str = mints::USDC;
/// @deprecated Use `mints::USDT` instead
pub const USDT_MINT: &str = mints::USDT;
/// @deprecated Use `programs::JUPITER` instead
pub const JUPITER_PROGRAM_ID: &str = programs::JUPITER;
