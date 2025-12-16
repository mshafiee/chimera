//! Unit tests for Kelly Criterion position sizing

#[cfg(test)]
mod tests {
    #[test]
    fn test_kelly_calculation() {
        // Example: 60% win rate, avg win = 0.1 SOL, avg loss = 0.05 SOL
        // kelly = (0.6 * 0.1 - 0.4 * 0.05) / 0.1
        // kelly = (0.06 - 0.02) / 0.1 = 0.4
        // Conservative (25%) = 0.1 = 10% of capital

        // This would be tested with actual database in integration tests
    }
}






