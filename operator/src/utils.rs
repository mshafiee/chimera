pub fn sol_to_lamports(sol: f64) -> u64 {
    // Use a small epsilon to handle floating point imprecision
    ((sol * 1_000_000_000.0) + 0.000001) as u64
}
