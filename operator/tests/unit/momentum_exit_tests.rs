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

        let detector = MomentumExit::new(pool, price_cache, 30);
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
        // For positions < 16 minutes old (before RSI is available), the base threshold is 5%
        // so a 6% drop SHOULD trigger exit — tighter guard while RSI protection is absent.
        // Once the position is ≥16 min old, the base rises to 8%.
        let (pool, _dir) = setup_test_db().await;
        let price_cache = Arc::new(PriceCache::new());

        let token = "So11111111111111111111111111111111111111112";
        let entry_price = Decimal::from_str("1.0").unwrap();

        // 4% drop on a 2-minute-old position: should NOT trigger (below 5% early threshold)
        let entry_time_new = SystemTime::now() - Duration::from_secs(120);
        let price_4pct = Decimal::from_str("0.96").unwrap();
        price_cache.set_price(token, price_4pct, PriceSource::Jupiter);
        let detector = MomentumExit::new(pool.clone(), price_cache.clone(), 30);
        let action_4pct = detector
            .check_momentum("uuid-drop-4", token, entry_price, entry_time_new)
            .await;
        assert_eq!(
            action_4pct,
            MomentumExitAction::None,
            "4% drop on a new position should not trigger — below 5% early base threshold"
        );

        // 6% drop on a 2-minute-old position: should trigger (above 5% early threshold)
        let price_6pct = Decimal::from_str("0.94").unwrap();
        price_cache.set_price(token, price_6pct, PriceSource::Jupiter);
        let detector2 = MomentumExit::new(pool.clone(), price_cache.clone(), 30);
        let action_6pct = detector2
            .check_momentum("uuid-drop-6", token, entry_price, entry_time_new)
            .await;
        assert_eq!(
            action_6pct,
            MomentumExitAction::Exit,
            "6% drop within 16 min should trigger exit — above 5% early base threshold"
        );

        // 6% drop on a 20-minute-old position: should NOT trigger (base is back to 8%)
        let entry_time_old = SystemTime::now() - Duration::from_secs(1200);
        price_cache.set_price(token, price_6pct, PriceSource::Jupiter);
        let detector3 = MomentumExit::new(pool, price_cache, 30);
        let action_6pct_old = detector3
            .check_momentum("uuid-drop-6-old", token, entry_price, entry_time_old)
            .await;
        assert_eq!(
            action_6pct_old,
            MomentumExitAction::None,
            "6% drop after 20 min should not trigger — below 8% standard base threshold"
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

        let detector = MomentumExit::new(pool, price_cache, 30);
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
