use rust_decimal::prelude::*;

/// Safely convert SOL (f64) to Lamports (u64) using Decimal to avoid precision loss
pub fn sol_to_lamports(sol: f64) -> u64 {
    // Convert float to Decimal first to handle precision safely
    // 1 SOL = 1,000,000,000 Lamports
    let sol_decimal = Decimal::from_f64_retain(sol).unwrap_or(Decimal::ZERO);
    let multiplier = Decimal::new(1_000_000_000, 0);
    
    (sol_decimal * multiplier).to_u64().unwrap_or(0)
}

/// Safely convert Lamports (u64) to SOL (f64) for display/DB
pub fn lamports_to_sol(lamports: u64) -> f64 {
    let lamports_dec = Decimal::from(lamports);
    let divisor = Decimal::new(1_000_000_000, 0);
    
    (lamports_dec / divisor).to_f64().unwrap_or(0.0)
}
