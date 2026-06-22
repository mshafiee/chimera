//! Unit tests for Kelly Criterion position sizing

#[cfg(test)]
mod tests {
    use chimera_operator::db_abstraction::{
        create_database, Database, DatabaseConfig, DbPool, InsertTrade,
    };
    use chimera_operator::engine::kelly_sizer::KellySizer;
    use chimera_operator::models::Strategy;
    use rust_decimal::prelude::*;
    use sqlx::Pool;
    use sqlx::Sqlite;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn sqlite_pool(db: &Arc<dyn Database>) -> Pool<Sqlite> {
        match db.pool() {
            DbPool::SQLite(pool) => pool,
            _ => panic!("test requires SQLite backend"),
        }
    }

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
}
