//! Unit tests for momentum exit module

#[cfg(test)]
mod tests {
    use chimera_operator::config::DatabaseConfig;
    use chimera_operator::db;
    use chimera_operator::engine::momentum_exit::{MomentumExit, MomentumExitAction};
    use chimera_operator::price_cache::{PriceCache, PriceSource};
    use rust_decimal::prelude::*;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};
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

    #[test]
    fn test_momentum_exit_action() {
        assert_eq!(MomentumExitAction::None, MomentumExitAction::None);
        assert_ne!(MomentumExitAction::None, MomentumExitAction::Exit);
    }

    #[tokio::test]
    async fn test_no_exit_when_price_stable() {
        // Price unchanged from entry: no momentum exit triggered.
        let (pool, _dir) = setup_test_db().await;
        let price_cache = Arc::new(PriceCache::new());

        let token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";
        let entry_price = Decimal::from_str("1.0").unwrap();

        // Current price same as entry price
        price_cache.set_price(token, entry_price, PriceSource::Jupiter);

        let detector = MomentumExit::new(pool, price_cache);
        let action = detector
            .check_momentum("uuid-stable", token, entry_price, SystemTime::now())
            .await;

        assert_eq!(
            action,
            MomentumExitAction::None,
            "Stable price should not trigger exit"
        );
    }

    #[tokio::test]
    async fn test_exit_when_price_drops_six_percent() {
        // Base threshold is now 8% — a 6% drop should NOT trigger exit (normal intraday noise).
        // A 9% drop within 5 minutes should trigger exit.
        let (pool, _dir) = setup_test_db().await;
        let price_cache = Arc::new(PriceCache::new());

        let token = "So11111111111111111111111111111111111111112";
        let entry_price = Decimal::from_str("1.0").unwrap();

        // Entry time set to 2 minutes ago (within the 5-minute window)
        let entry_time = SystemTime::now() - Duration::from_secs(120);

        // 6% drop: should NOT trigger (below 8% base threshold)
        let price_6pct = Decimal::from_str("0.94").unwrap();
        price_cache.set_price(token, price_6pct, PriceSource::Jupiter);
        let detector = MomentumExit::new(pool.clone(), price_cache.clone());
        let action_6pct = detector
            .check_momentum("uuid-drop-6", token, entry_price, entry_time)
            .await;
        assert_eq!(
            action_6pct,
            MomentumExitAction::None,
            "6% drop should not trigger exit — below new 8% base threshold"
        );

        // 9% drop: should trigger exit (above 8% base threshold)
        let price_9pct = Decimal::from_str("0.91").unwrap();
        price_cache.set_price(token, price_9pct, PriceSource::Jupiter);
        let detector2 = MomentumExit::new(pool, price_cache);
        let action_9pct = detector2
            .check_momentum("uuid-drop-9", token, entry_price, entry_time)
            .await;
        assert_eq!(
            action_9pct,
            MomentumExitAction::Exit,
            "9% price drop within 5 min should trigger exit"
        );
    }

    #[tokio::test]
    async fn test_no_exit_when_no_price_data() {
        // If price cache has no data for the token, check_momentum should return None.
        let (pool, _dir) = setup_test_db().await;
        let price_cache = Arc::new(PriceCache::new());

        // Do NOT set any price for this token
        let token = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
        let entry_price = Decimal::from_str("1.0").unwrap();

        let detector = MomentumExit::new(pool, price_cache);
        let action = detector
            .check_momentum("uuid-noprice", token, entry_price, SystemTime::now())
            .await;

        assert_eq!(
            action,
            MomentumExitAction::None,
            "Missing price data should not trigger exit"
        );
    }
}
