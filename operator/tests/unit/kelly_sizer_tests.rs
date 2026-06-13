//! Unit tests for Kelly Criterion position sizing

#[cfg(test)]
mod tests {
    use chimera_operator::config::DatabaseConfig;
    use chimera_operator::db;
    use chimera_operator::engine::kelly_sizer::KellySizer;
    use rust_decimal::prelude::*;
    use tempfile::TempDir;

    async fn setup_test_db() -> (db::DbPool, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let pool = db::init_pool(&DatabaseConfig {
            path: db_path,
            max_connections: 5,
        })
        .await
        .unwrap();
        db::run_migrations(&pool).await.unwrap();
        (pool, temp_dir)
    }

    #[tokio::test]
    async fn test_kelly_zero_trade_history() {
        // With no closed trades, calculate_kelly should return an error.
        let (pool, _dir) = setup_test_db().await;
        let sizer = KellySizer::new(pool);

        let result = sizer.calculate_kelly("wallet_with_no_trades", 30).await;
        assert!(result.is_err(), "Expected error for wallet with no trades");
    }

    #[tokio::test]
    async fn test_kelly_positive_edge() {
        // 60% win rate, avg_win = 0.1 SOL, avg_loss = 0.05 SOL
        // full_kelly = (0.6 * 0.1 - 0.4 * 0.05) / 0.1 = 0.4
        // conservative (25%) = 0.1
        let (pool, _dir) = setup_test_db().await;

        let wallet = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
        let token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";

        // Insert 6 winning trades (+0.1 SOL each) and 4 losing trades (-0.05 SOL each)
        for i in 0..6u32 {
            let uuid = format!("win-{}", i);
            db::insert_trade(
                &pool,
                &uuid,
                wallet,
                token,
                Some("BONK"),
                "SHIELD",
                "BUY",
                Decimal::from_str("0.1").unwrap(),
                "CLOSED",
            )
            .await
            .unwrap();
            db::update_trade_net_pnl(&pool, &uuid, Decimal::from_str("0.1").unwrap())
                .await
                .unwrap();
        }
        for i in 0..4u32 {
            let uuid = format!("loss-{}", i);
            db::insert_trade(
                &pool,
                &uuid,
                wallet,
                token,
                Some("BONK"),
                "SHIELD",
                "BUY",
                Decimal::from_str("0.05").unwrap(),
                "CLOSED",
            )
            .await
            .unwrap();
            db::update_trade_net_pnl(&pool, &uuid, Decimal::from_str("-0.05").unwrap())
                .await
                .unwrap();
        }

        let sizer = KellySizer::new(pool);
        let result = sizer.calculate_kelly(wallet, 30).await.unwrap();

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
        let (pool, _dir) = setup_test_db().await;

        let wallet = "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890AA";
        let token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";

        // 3 wins of 0.05 and 7 losses of 0.1 → negative edge
        for i in 0..3u32 {
            let uuid = format!("neg-win-{}", i);
            db::insert_trade(
                &pool,
                &uuid,
                wallet,
                token,
                Some("BONK"),
                "SHIELD",
                "BUY",
                Decimal::from_str("0.05").unwrap(),
                "CLOSED",
            )
            .await
            .unwrap();
            db::update_trade_net_pnl(&pool, &uuid, Decimal::from_str("0.05").unwrap())
                .await
                .unwrap();
        }
        for i in 0..7u32 {
            let uuid = format!("neg-loss-{}", i);
            db::insert_trade(
                &pool,
                &uuid,
                wallet,
                token,
                Some("BONK"),
                "SHIELD",
                "BUY",
                Decimal::from_str("0.1").unwrap(),
                "CLOSED",
            )
            .await
            .unwrap();
            db::update_trade_net_pnl(&pool, &uuid, Decimal::from_str("-0.1").unwrap())
                .await
                .unwrap();
        }

        let sizer = KellySizer::new(pool);
        let result = sizer.calculate_kelly(wallet, 30).await.unwrap();

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
}
