use rust_decimal::prelude::*;

/// Safely convert SOL (Decimal) to Lamports (u64) using Decimal to avoid precision loss
pub fn sol_to_lamports(sol: Decimal) -> u64 {
    // 1 SOL = 1,000,000,000 Lamports
    let multiplier = Decimal::new(1_000_000_000, 0);
    
    (sol * multiplier).to_u64().unwrap_or(0)
}

/// Safely convert SOL (f64) to Lamports (u64) using Decimal to avoid precision loss
/// This is a convenience function for legacy code that still uses f64
pub fn sol_to_lamports_f64(sol: f64) -> u64 {
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
