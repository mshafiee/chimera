//! Unit tests for Kelly Criterion position sizing

#[cfg(test)]
mod tests {
    use chimera_operator::db_abstraction::{
        create_database, Database, DatabaseConfig, InsertTrade,
    };
    use chimera_operator::engine::kelly_sizer::KellySizer;
    use chimera_operator::models::Strategy;
    use rust_decimal::prelude::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn setup_test_db() -> (Arc<dyn Database>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let config = DatabaseConfig::sqlite(temp_dir.path().join("test.db"));
        let db = create_database(&config).await.unwrap();
        db.run_migrations().await.unwrap();
        (db, temp_dir)
    }

    #[tokio::test]
    async fn test_kelly_zero_trade_history() {
        // With no closed trades, calculate_kelly should return an error.
        let (db, _dir) = setup_test_db().await;
        let sizer = KellySizer::new(db);

        let result = sizer
            .calculate_kelly("wallet_with_no_trades", Strategy::Shield, 30)
            .await;
        assert!(result.is_err(), "Expected error for wallet with no trades");
    }

    #[tokio::test]
    async fn test_kelly_positive_edge() {
        // 60% win rate, avg_win = 0.1 SOL, avg_loss = 0.05 SOL
        // full_kelly = (0.6 * 0.1 - 0.4 * 0.05) / 0.1 = 0.4
        // conservative (25%) = 0.1
        let (db, _dir) = setup_test_db().await;

        let wallet = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
        let token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";

        // Insert 12 winning trades (+0.1 SOL each) and 8 losing trades (-0.05 SOL each)
        // = 20 trades total at 60% win rate (minimum required by Kelly sizer).
        for i in 0..12u32 {
            let uuid = format!("win-{}", i);
            db.insert_trade(&InsertTrade {
                trade_uuid: uuid.clone(),
                wallet_address: wallet.to_string(),
                token_address: token.to_string(),
                token_symbol: Some("BONK".to_string()),
                strategy: "SHIELD".to_string(),
                side: "BUY".to_string(),
                amount_sol: Decimal::from_str("0.1").unwrap(),
                status: "CLOSED".to_string(),
            })
            .await
            .unwrap();
            db.update_trade_net_pnl(&uuid, Decimal::from_str("0.1").unwrap())
                .await
                .unwrap();
        }
        for i in 0..8u32 {
            let uuid = format!("loss-{}", i);
            db.insert_trade(&InsertTrade {
                trade_uuid: uuid.clone(),
                wallet_address: wallet.to_string(),
                token_address: token.to_string(),
                token_symbol: Some("BONK".to_string()),
                strategy: "SHIELD".to_string(),
                side: "BUY".to_string(),
                amount_sol: Decimal::from_str("0.05").unwrap(),
                status: "CLOSED".to_string(),
            })
            .await
            .unwrap();
            db.update_trade_net_pnl(&uuid, Decimal::from_str("-0.05").unwrap())
                .await
                .unwrap();
        }

        let sizer = KellySizer::new(db);
        let result = sizer
            .calculate_kelly(wallet, Strategy::Shield, 30)
            .await
            .unwrap();

        assert!(
            result.full_kelly > Decimal::ZERO,
            "full_kelly should be positive with positive edge"
        );
        assert!(
            result.conservative_kelly > Decimal::ZERO,
            "conservative_kelly should be positive"
        );
        assert!(
            result.conservative_kelly <= result.full_kelly,
            "conservative should be <= full kelly"
        );
        assert!(
            result.win_rate > Decimal::ZERO && result.win_rate <= Decimal::ONE,
            "win_rate should be 0-1"
        );
    }

    #[tokio::test]
    async fn test_kelly_negative_edge() {
        // More losses than wins → kelly fraction should be 0 (never go negative)
        let (db, _dir) = setup_test_db().await;

        let wallet = "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890AA";
        let token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";

        // 6 wins of 0.05 and 14 losses of 0.1 → negative edge (30% win rate, 20 trades total)
        for i in 0..6u32 {
            let uuid = format!("neg-win-{}", i);
            db.insert_trade(&InsertTrade {
                trade_uuid: uuid.clone(),
                wallet_address: wallet.to_string(),
                token_address: token.to_string(),
                token_symbol: Some("BONK".to_string()),
                strategy: "SHIELD".to_string(),
                side: "BUY".to_string(),
                amount_sol: Decimal::from_str("0.05").unwrap(),
                status: "CLOSED".to_string(),
            })
            .await
            .unwrap();
            db.update_trade_net_pnl(&uuid, Decimal::from_str("0.05").unwrap())
                .await
                .unwrap();
        }
        for i in 0..14u32 {
            let uuid = format!("neg-loss-{}", i);
            db.insert_trade(&InsertTrade {
                trade_uuid: uuid.clone(),
                wallet_address: wallet.to_string(),
                token_address: token.to_string(),
                token_symbol: Some("BONK".to_string()),
                strategy: "SHIELD".to_string(),
                side: "BUY".to_string(),
                amount_sol: Decimal::from_str("0.1").unwrap(),
                status: "CLOSED".to_string(),
            })
            .await
            .unwrap();
            db.update_trade_net_pnl(&uuid, Decimal::from_str("-0.1").unwrap())
                .await
                .unwrap();
        }

        let sizer = KellySizer::new(db);
        let result = sizer
            .calculate_kelly(wallet, Strategy::Shield, 30)
            .await
            .unwrap();

        // Negative edge: kelly is clamped to zero (implementation uses .max(Decimal::ZERO))
        assert_eq!(
            result.full_kelly,
            Decimal::ZERO,
            "full_kelly should be 0 when edge is negative"
        );
        assert_eq!(
            result.recommended_size_percent,
            Decimal::ZERO,
            "Position size must be 0 with negative edge"
        );
    }

    #[tokio::test]
    async fn test_expected_profit_calculation() {
        // Test expected profit calculation with a positive edge wallet
        // 60% win rate, avg_win = 0.1 (10%), avg_loss = 0.05 (5%)
        // Expected return = (0.6 * 0.1) - (0.4 * 0.05) = 0.06 - 0.02 = 0.04 (4%)
        let (db, _dir) = setup_test_db().await;

        let wallet = "expected_profit_test_wallet";
        let token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";

        // Insert 12 winning trades (+0.1 SOL each) and 8 losing trades (-0.05 SOL each)
        for i in 0..12u32 {
            let uuid = format!("ep-win-{}", i);
            db.insert_trade(&InsertTrade {
                trade_uuid: uuid.clone(),
                wallet_address: wallet.to_string(),
                token_address: token.to_string(),
                token_symbol: Some("TEST".to_string()),
                strategy: "SHIELD".to_string(),
                side: "BUY".to_string(),
                amount_sol: Decimal::from_str("0.1").unwrap(),
                status: "CLOSED".to_string(),
            })
            .await
            .unwrap();
            db.update_trade_net_pnl(&uuid, Decimal::from_str("0.1").unwrap())
                .await
                .unwrap();
        }
        for i in 0..8u32 {
            let uuid = format!("ep-loss-{}", i);
            db.insert_trade(&InsertTrade {
                trade_uuid: uuid.clone(),
                wallet_address: wallet.to_string(),
                token_address: token.to_string(),
                token_symbol: Some("TEST".to_string()),
                strategy: "SHIELD".to_string(),
                side: "BUY".to_string(),
                amount_sol: Decimal::from_str("0.05").unwrap(),
                status: "CLOSED".to_string(),
            })
            .await
            .unwrap();
            db.update_trade_net_pnl(&uuid, Decimal::from_str("-0.05").unwrap())
                .await
                .unwrap();
        }

        let sizer = KellySizer::new(db);
        let kelly = sizer
            .calculate_kelly(wallet, Strategy::Shield, 30)
            .await
            .unwrap();

        // Test expected_return_pct calculation
        // Expected return = (0.6 * 0.1) - (0.4 * 0.05) = 0.04 (4%)
        let expected_return = kelly.expected_return_pct();
        assert!(
            expected_return > Decimal::ZERO,
            "Expected return should be positive for profitable wallet"
        );

        // Expected return should be close to 4% (with some tolerance for rounding)
        let expected_approx = Decimal::from_str("0.04").unwrap();
        let tolerance = Decimal::from_str("0.005").unwrap(); // 0.5% tolerance
        assert!(
            (expected_return - expected_approx).abs() < tolerance,
            "Expected return should be approximately 4%, got {}",
            expected_return
        );

        // Test expected_profit_sol calculation
        // For 1.0 SOL position: expected profit = 1.0 * 0.04 = 0.04 SOL
        let position_size = Decimal::from_str("1.0").unwrap();
        let expected_profit = kelly.expected_profit_sol(position_size);

        let profit_approx = Decimal::from_str("0.04").unwrap();
        assert!(
            (expected_profit - profit_approx).abs() < tolerance,
            "Expected profit should be approximately 0.04 SOL, got {}",
            expected_profit
        );

        // Test with different position sizes
        // For 0.5 SOL position: expected profit = 0.5 * 0.04 = 0.02 SOL
        let small_position = Decimal::from_str("0.5").unwrap();
        let small_profit = kelly.expected_profit_sol(small_position);
        let small_profit_approx = Decimal::from_str("0.02").unwrap();
        assert!(
            (small_profit - small_profit_approx).abs() < tolerance,
            "Expected profit for 0.5 SOL should be approximately 0.02 SOL, got {}",
            small_profit
        );

        // For 2.0 SOL position: expected profit = 2.0 * 0.04 = 0.08 SOL
        let large_position = Decimal::from_str("2.0").unwrap();
        let large_profit = kelly.expected_profit_sol(large_position);
        let large_profit_approx = Decimal::from_str("0.08").unwrap();
        assert!(
            (large_profit - large_profit_approx).abs() < tolerance,
            "Expected profit for 2.0 SOL should be approximately 0.08 SOL, got {}",
            large_profit
        );
    }

    #[tokio::test]
    async fn test_expected_profit_negative_edge() {
        // Test expected profit calculation with a negative edge wallet
        // Expected return should be negative
        let (db, _dir) = setup_test_db().await;

        let wallet = "negative_edge_wallet";
        let token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";

        // 6 wins of 0.05 and 14 losses of 0.1 → negative edge (30% win rate)
        for i in 0..6u32 {
            let uuid = format!("neg-ep-win-{}", i);
            db.insert_trade(&InsertTrade {
                trade_uuid: uuid.clone(),
                wallet_address: wallet.to_string(),
                token_address: token.to_string(),
                token_symbol: Some("TEST".to_string()),
                strategy: "SHIELD".to_string(),
                side: "BUY".to_string(),
                amount_sol: Decimal::from_str("0.05").unwrap(),
                status: "CLOSED".to_string(),
            })
            .await
            .unwrap();
            db.update_trade_net_pnl(&uuid, Decimal::from_str("0.05").unwrap())
                .await
                .unwrap();
        }
        for i in 0..14u32 {
            let uuid = format!("neg-ep-loss-{}", i);
            db.insert_trade(&InsertTrade {
                trade_uuid: uuid.clone(),
                wallet_address: wallet.to_string(),
                token_address: token.to_string(),
                token_symbol: Some("TEST".to_string()),
                strategy: "SHIELD".to_string(),
                side: "BUY".to_string(),
                amount_sol: Decimal::from_str("0.1").unwrap(),
                status: "CLOSED".to_string(),
            })
            .await
            .unwrap();
            db.update_trade_net_pnl(&uuid, Decimal::from_str("-0.1").unwrap())
                .await
                .unwrap();
        }

        let sizer = KellySizer::new(db);
        let kelly = sizer
            .calculate_kelly(wallet, Strategy::Shield, 30)
            .await
            .unwrap();

        // Expected return should be negative
        let expected_return = kelly.expected_return_pct();
        assert!(
            expected_return < Decimal::ZERO,
            "Expected return should be negative for losing wallet"
        );

        // Expected profit should also be negative
        let position_size = Decimal::from_str("1.0").unwrap();
        let expected_profit = kelly.expected_profit_sol(position_size);
        assert!(
            expected_profit < Decimal::ZERO,
            "Expected profit should be negative for losing wallet"
        );
    }
}
