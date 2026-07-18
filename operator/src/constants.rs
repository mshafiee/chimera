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
    /// Token Program (legacy SPL Token)
    pub const TOKEN: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
    /// Token-2022 Program
    pub const TOKEN_2022: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
}

/// Established, high-liquidity Solana tokens that are pre-vetted as safe.
///
/// These bypass the crude Liq/FDV Ghost-Chain heuristic in `slow_check`. That
/// heuristic (max-pool-liquidity / FDV >= 5%) is impossible for any large-cap
/// token — FDV dwarfs a single pool's liquidity (e.g. BONK: $119K pool / $245M
/// FDV = 0.05%), so it wrongly blocks the safest, highest-volume copy targets.
///
/// Slippage/exit risk for these tokens is instead gated by the executor's
/// Jupiter price-impact check (see `MAX_PRICE_IMPACT_PCT`), which measures the
/// actual trade's market impact rather than a market-cap-vs-pool ratio.
///
/// Only add a token here after confirming deep, multi-pool (and ideally CEX)
/// liquidity and that mint/freeze authority are revoked.
pub mod verified_majors {
    /// BONK
    pub const BONK: &str = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";
    /// dogwifhat (WIF)
    pub const WIF: &str = "EKpQGSJtjMFqKZ9KQanSqYXRcF8fBopzLHYxdM65zcjm";
    /// Jupiter (JUP)
    pub const JUP: &str = "JUPyiwrYJFskUPiHa7hkeR8VUtAeFoSYbKedZNsDvCN";
    /// POPCAT (POP)
    pub const POP: &str = "7GCihgDB8fe6KNjn2MYtkzZcRjQy3t9GHdC8uHYmW2hr";
    /// GRASS
    pub const GRASS: &str = "Grass7B4RdKfBCjTKgSqnXkqjwiG6DuGc62rSdZ9mtm";
    /// Jito (JTO)
    pub const JTO: &str = "jtojtomepa8beP8AuQc6eXt5FriJwfFMwQx2v2f9mCL";

    /// All verified-major mints
    pub const ALL: &[&str] = &[BONK, WIF, JUP, POP, GRASS, JTO];
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
