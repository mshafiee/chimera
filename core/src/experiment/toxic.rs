use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::config::ExperimentConfig;
use crate::error::AppResult;

#[derive(Debug, Clone, Copy)]
pub enum ToxicReason {
    RoiDrop,
    LocalTopSqueeze,
}

#[derive(Debug, Clone)]
pub struct ToxicWallet {
    pub address: String,
    pub selection_roi: f64,
    pub post_promotion_roi: f64,
    pub local_top_entries: u32,
    pub total_entries: u32,
    pub is_toxic: bool,
    pub toxic_reason: Option<ToxicReason>,
    pub detected_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone)]
pub struct ToxicFlowDetector {
    wallets: Arc<RwLock<HashMap<String, ToxicWallet>>>,
    config: ExperimentConfig,
}

impl ToxicFlowDetector {
    pub fn new(config: ExperimentConfig) -> Self {
        Self {
            wallets: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    pub async fn register_wallet_promotion(
        &self,
        wallet: String,
        selection_roi: f64,
    ) -> AppResult<()> {
        debug!(
            "Registering wallet promotion: {} with ROI: {}",
            wallet, selection_roi
        );

        let mut wallets = self.wallets.write().await;
        // Preserve existing state on re-promotion: refresh ROI baselines but
        // never silently clear accumulated counters or a previously-detected
        // toxic flag (e.g. state loaded from the database on startup).
        wallets
            .entry(wallet.clone())
            .and_modify(|w| {
                w.selection_roi = selection_roi;
                w.post_promotion_roi = selection_roi;
            })
            .or_insert(ToxicWallet {
                address: wallet,
                selection_roi,
                post_promotion_roi: selection_roi,
                local_top_entries: 0,
                total_entries: 0,
                is_toxic: false,
                toxic_reason: None,
                detected_at: None,
            });

        Ok(())
    }

    pub async fn record_entry(
        &self,
        wallet: String,
        is_local_top: bool,
        current_roi: f64,
    ) -> AppResult<Option<ToxicReason>> {
        let mut wallets = self.wallets.write().await;

        if let Some(w) = wallets.get_mut(&wallet) {
            w.total_entries += 1;
            w.post_promotion_roi = current_roi;

            if is_local_top {
                w.local_top_entries += 1;
            }

            // Check toxic conditions
            if self.should_flag_as_toxic(w) {
                let reason = self.determine_toxic_reason(w);
                // Flag only on transition so we can also recover.
                if !w.is_toxic {
                    w.is_toxic = true;
                    w.toxic_reason = Some(reason);
                    w.detected_at = Some(chrono::Utc::now());
                    warn!("Wallet {} flagged as toxic: {:?}", wallet, reason);
                    return Ok(Some(reason));
                }
            } else if w.is_toxic {
                // RECOVERY (2026-08-05): a previously-flagged wallet whose
                // ROI has recovered is un-flagged. Without this, the flag was
                // permanent: a toxic wallet's signals are rejected → it never
                // trades → never re-evaluated → deadlock forever (observed:
                // 6 wallets stuck toxic, zero trades for 4h+). Re-evaluating
                // on every entry makes the flag self-healing.
                w.is_toxic = false;
                w.toxic_reason = None;
                w.detected_at = None;
                info!(
                    wallet = %wallet,
                    current_roi,
                    "ToxicFlowDetector: wallet recovered — toxic flag cleared"
                );
            }
        }

        Ok(None)
    }

    fn should_flag_as_toxic(&self, wallet: &ToxicWallet) -> bool {
        // Check ROI drop (significant deterioration)
        let roi_deterioration = wallet.selection_roi - wallet.post_promotion_roi;
        if roi_deterioration > (self.config.toxic_threshold_percent as f64) / 100.0 {
            return true;
        }

        // Check local-top squeeze (multiple entries at local top)
        if wallet.local_top_entries >= 3 && wallet.local_top_entries * 2 >= wallet.total_entries {
            return true;
        }

        false
    }

    fn determine_toxic_reason(&self, wallet: &ToxicWallet) -> ToxicReason {
        let roi_deterioration = wallet.selection_roi - wallet.post_promotion_roi;

        if roi_deterioration > (self.config.toxic_threshold_percent as f64) / 100.0 {
            return ToxicReason::RoiDrop;
        }

        if wallet.local_top_entries >= 3 && wallet.local_top_entries * 2 >= wallet.total_entries {
            return ToxicReason::LocalTopSqueeze;
        }

        ToxicReason::RoiDrop
    }

    pub async fn get_toxic_wallets(&self) -> Vec<String> {
        let wallets = self.wallets.read().await;
        wallets
            .values()
            .filter(|w| w.is_toxic)
            .map(|w| w.address.clone())
            .collect()
    }

    pub async fn is_wallet_toxic(&self, wallet: &str) -> bool {
        let wallets = self.wallets.read().await;
        wallets.get(wallet).map(|w| w.is_toxic).unwrap_or(false)
    }

    pub async fn get_toxic_rate(&self) -> f64 {
        let wallets = self.wallets.read().await;
        let total = wallets.len();
        if total == 0 {
            return 0.0;
        }

        let toxic = wallets.values().filter(|w| w.is_toxic).count();
        toxic as f64 / total as f64
    }

    pub async fn persist_to_database(
        &self,
        pool: &sqlx::Pool<sqlx::Postgres>,
        run_id: &str,
    ) -> AppResult<()> {
        // Snapshot under a short read lock, then perform the DB I/O without
        // holding the write guard — otherwise every detector operation blocks
        // for the whole batch of network round-trips.
        let wallets: Vec<ToxicWallet> = {
            let wallets = self.wallets.read().await;
            wallets.values().cloned().collect()
        };

        for wallet in &wallets {
            let toxic_reason_str = match wallet.toxic_reason {
                Some(ToxicReason::RoiDrop) => Some("roi_drop".to_string()),
                Some(ToxicReason::LocalTopSqueeze) => Some("local_top_squeeze".to_string()),
                None => None,
            };

            sqlx::query(
                r#"
                INSERT INTO toxic_wallets (
                    wallet_address, selection_roi, post_promotion_roi,
                    local_top_entries, total_entries, is_toxic,
                    toxic_reason, detected_at, run_id
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9
                ) ON CONFLICT(wallet_address) DO UPDATE SET
                    post_promotion_roi = excluded.post_promotion_roi,
                    local_top_entries = excluded.local_top_entries,
                    total_entries = excluded.total_entries,
                    is_toxic = excluded.is_toxic,
                    toxic_reason = excluded.toxic_reason,
                    detected_at = excluded.detected_at,
                    run_id = excluded.run_id,
                    updated_at = CURRENT_TIMESTAMP
                "#,
            )
            .bind(&wallet.address)
            .bind(wallet.selection_roi)
            .bind(wallet.post_promotion_roi)
            .bind(wallet.local_top_entries as i64)
            .bind(wallet.total_entries as i64)
            .bind(wallet.is_toxic)
            .bind(toxic_reason_str)
            .bind(wallet.detected_at)
            .bind(run_id)
            .execute(pool)
            .await?;
        }

        info!(
            "Persisted {} toxic wallet records to database",
            wallets.len()
        );
        Ok(())
    }

    /// Load previously-detected toxic wallet state from the database on startup.
    /// Only loads wallets marked toxic — non-toxic tracking starts fresh.
    pub async fn load_from_database(&self, pool: &sqlx::Pool<sqlx::Postgres>) -> AppResult<()> {
        let rows = sqlx::query_as::<_, ToxicWalletRow>(
            r#"
            SELECT wallet_address, selection_roi, post_promotion_roi,
                   local_top_entries::bigint, total_entries::bigint, is_toxic,
                   toxic_reason, detected_at
            FROM toxic_wallets
            WHERE is_toxic = TRUE
            "#,
        )
        .fetch_all(pool)
        .await?;

        let mut wallets = self.wallets.write().await;
        let count = rows.len();
        for row in rows {
            let toxic_reason = row.toxic_reason.as_deref().map(|r| match r {
                "roi_drop" => ToxicReason::RoiDrop,
                "local_top_squeeze" => ToxicReason::LocalTopSqueeze,
                _ => ToxicReason::RoiDrop,
            });

            // RE-VALIDATE on load (2026-08-05): the flag must still hold NOW.
            // Stale persisted flags — e.g. from the ROI unit bug (SOL PnL vs
            // ratio) or a wallet that has since recovered — must not resurrect
            // across restarts. A stale flag is a permanent deadlock: the
            // wallet's signals get rejected so it never trades, so record_entry
            // (the recovery path) never fires.
            let candidate = ToxicWallet {
                address: row.wallet_address.clone(),
                selection_roi: row.selection_roi,
                post_promotion_roi: row.post_promotion_roi,
                local_top_entries: row.local_top_entries as u32,
                total_entries: row.total_entries as u32,
                is_toxic: true,
                toxic_reason,
                detected_at: row.detected_at,
            };
            if !self.should_flag_as_toxic(&candidate) {
                info!(
                    wallet = %candidate.address,
                    selection_roi = candidate.selection_roi,
                    post_promotion_roi = candidate.post_promotion_roi,
                    "Toxic wallet loaded from DB but no longer qualifies — skipping (flag cleared)"
                );
                continue;
            }

            wallets.insert(row.wallet_address.clone(), candidate);
        }

        if count > 0 {
            warn!("Loaded {} toxic wallets from database on startup", count);
        }

        Ok(())
    }

    pub async fn get_statistics(&self) -> ToxicStatistics {
        let wallets = self.wallets.read().await;

        let total_wallets = wallets.len();
        let toxic_wallets = wallets.values().filter(|w| w.is_toxic).count();
        let total_entries: u32 = wallets.values().map(|w| w.total_entries).sum();
        let local_top_entries: u32 = wallets.values().map(|w| w.local_top_entries).sum();

        ToxicStatistics {
            total_wallets,
            toxic_wallets,
            total_entries,
            local_top_entries,
            toxic_rate: if total_wallets > 0 {
                toxic_wallets as f64 / total_wallets as f64
            } else {
                0.0
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToxicStatistics {
    pub total_wallets: usize,
    pub toxic_wallets: usize,
    pub total_entries: u32,
    pub local_top_entries: u32,
    pub toxic_rate: f64,
}

/// Row mapping for `load_from_database`.
struct ToxicWalletRow {
    wallet_address: String,
    selection_roi: f64,
    post_promotion_roi: f64,
    local_top_entries: i64,
    total_entries: i64,
    #[allow(dead_code)] // Selected by `WHERE is_toxic = TRUE`; value is not re-stored
    is_toxic: bool,
    toxic_reason: Option<String>,
    detected_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for ToxicWalletRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        // Propagate decode errors: swallowing them (e.g. is_toxic defaulting to
        // false) could make a genuinely toxic wallet appear clean.
        Ok(Self {
            wallet_address: row.try_get("wallet_address")?,
            selection_roi: row.try_get::<f64, _>("selection_roi")?,
            post_promotion_roi: row.try_get::<f64, _>("post_promotion_roi")?,
            local_top_entries: row.try_get::<i64, _>("local_top_entries")?,
            total_entries: row.try_get::<i64, _>("total_entries")?,
            is_toxic: row.try_get("is_toxic")?,
            toxic_reason: row.try_get("toxic_reason").ok().flatten(),
            detected_at: row.try_get("detected_at").ok().flatten(),
        })
    }
}

impl std::fmt::Display for ToxicStatistics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ToxicFlow: {}/{} wallets toxic ({:.1}%), {}/{} local-top entries",
            self.toxic_wallets,
            self.total_wallets,
            self.toxic_rate * 100.0,
            self.local_top_entries,
            self.total_entries
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ExperimentConfig;

    fn config(threshold_percent: u32) -> ExperimentConfig {
        ExperimentConfig {
            toxic_threshold_percent: threshold_percent,
            ..ExperimentConfig::default()
        }
    }

    // ==========================================================================
    // PROMOTION REGISTRATION
    // ==========================================================================

    #[tokio::test]
    async fn register_new_wallet_creates_baseline() {
        let detector = ToxicFlowDetector::new(config(30));
        detector
            .register_wallet_promotion("wallet-a".to_string(), 0.5)
            .await
            .unwrap();

        assert!(!detector.is_wallet_toxic("wallet-a").await);
        let stats = detector.get_statistics().await;
        assert_eq!(stats.total_wallets, 1);
        assert_eq!(stats.total_entries, 0);
        assert_eq!(stats.local_top_entries, 0);
        assert_eq!(stats.toxic_wallets, 0);
        assert_eq!(stats.toxic_rate, 0.0);
    }

    #[tokio::test]
    async fn re_promotion_updates_roi_preserves_counters() {
        let detector = ToxicFlowDetector::new(config(30));
        detector
            .register_wallet_promotion("wallet-a".to_string(), 0.5)
            .await
            .unwrap();
        // Accumulate some entries first.
        detector
            .record_entry("wallet-a".to_string(), false, 0.4)
            .await
            .unwrap();
        detector
            .record_entry("wallet-a".to_string(), true, 0.45)
            .await
            .unwrap();

        // Re-promotion refreshes both ROI baselines but must NOT clear counters.
        detector
            .register_wallet_promotion("wallet-a".to_string(), 0.9)
            .await
            .unwrap();

        let stats = detector.get_statistics().await;
        assert_eq!(stats.total_entries, 2);
        assert_eq!(stats.local_top_entries, 1);
        assert_eq!(stats.toxic_rate, 0.0);

        // A flag set before re-promotion must survive it.
        detector
            .register_wallet_promotion("wallet-b".to_string(), 0.5)
            .await
            .unwrap();
        detector
            .record_entry("wallet-b".to_string(), false, 0.0)
            .await
            .unwrap();
        assert!(detector.is_wallet_toxic("wallet-b").await);
        detector
            .register_wallet_promotion("wallet-b".to_string(), 0.1)
            .await
            .unwrap();
        assert!(
            detector.is_wallet_toxic("wallet-b").await,
            "re-promotion must not clear an existing toxic flag"
        );
    }

    // ==========================================================================
    // TOXIC DETECTION
    // ==========================================================================

    #[tokio::test]
    async fn roi_drop_flags_wallet_with_reason() {
        let detector = ToxicFlowDetector::new(config(30)); // 30% threshold
        detector
            .register_wallet_promotion("wallet-a".to_string(), 0.50)
            .await
            .unwrap();

        // Deterioration 0.50 - 0.10 = 0.40 > 0.30 → toxic.
        let reason = detector
            .record_entry("wallet-a".to_string(), false, 0.10)
            .await
            .unwrap();
        assert!(matches!(reason, Some(ToxicReason::RoiDrop)));
        assert!(detector.is_wallet_toxic("wallet-a").await);

        let wallets = detector.get_toxic_wallets().await;
        assert_eq!(wallets, vec!["wallet-a".to_string()]);
        assert_eq!(detector.get_toxic_rate().await, 1.0);
    }

    #[tokio::test]
    async fn local_top_squeeze_flags_wallet() {
        let detector = ToxicFlowDetector::new(config(30));
        detector
            .register_wallet_promotion("wallet-squeeze".to_string(), 0.5)
            .await
            .unwrap();

        // 3 entries, all at local top: 3 >= 3 and 3*2 >= 3 → squeeze.
        for i in 0..3 {
            let reason = detector
                .record_entry("wallet-squeeze".to_string(), true, 0.4)
                .await
                .unwrap();
            if i == 2 {
                assert!(matches!(reason, Some(ToxicReason::LocalTopSqueeze)));
            } else {
                assert!(reason.is_none());
            }
        }
        assert!(detector.is_wallet_toxic("wallet-squeeze").await);
    }

    #[tokio::test]
    async fn no_flag_on_mild_deterioration() {
        let detector = ToxicFlowDetector::new(config(30));
        detector
            .register_wallet_promotion("wallet-a".to_string(), 0.50)
            .await
            .unwrap();
        // Deterioration 0.10 < 0.30 and only 2 local-top of 4 entries (2*2 >= 4? no).
        let reason = detector
            .record_entry("wallet-a".to_string(), true, 0.40)
            .await
            .unwrap();
        assert!(reason.is_none());
        let reason = detector
            .record_entry("wallet-a".to_string(), true, 0.40)
            .await
            .unwrap();
        assert!(reason.is_none());
        assert!(!detector.is_wallet_toxic("wallet-a").await);
    }

    #[tokio::test]
    async fn already_toxic_does_not_re_flag() {
        let detector = ToxicFlowDetector::new(config(30));
        detector
            .register_wallet_promotion("wallet-a".to_string(), 0.5)
            .await
            .unwrap();
        detector
            .record_entry("wallet-a".to_string(), false, 0.05)
            .await
            .unwrap();
        assert!(detector.is_wallet_toxic("wallet-a").await);

        // Another bad entry while already toxic: no new reason is returned.
        let reason = detector
            .record_entry("wallet-a".to_string(), false, 0.0)
            .await
            .unwrap();
        assert!(reason.is_none());
        assert!(detector.is_wallet_toxic("wallet-a").await);
    }

    #[tokio::test]
    async fn wallet_recovers_after_roi_improves() {
        let detector = ToxicFlowDetector::new(config(30));
        detector
            .register_wallet_promotion("wallet-a".to_string(), 0.5)
            .await
            .unwrap();
        detector
            .record_entry("wallet-a".to_string(), false, 0.05)
            .await
            .unwrap();
        assert!(detector.is_wallet_toxic("wallet-a").await);

        // ROI recovers above the threshold → flag cleared.
        let reason = detector
            .record_entry("wallet-a".to_string(), false, 0.45)
            .await
            .unwrap();
        assert!(reason.is_none());
        assert!(!detector.is_wallet_toxic("wallet-a").await);
        assert!(detector.get_toxic_wallets().await.is_empty());
    }

    #[tokio::test]
    async fn record_entry_unknown_wallet_is_noop() {
        let detector = ToxicFlowDetector::new(config(30));
        let reason = detector
            .record_entry("nobody".to_string(), true, 0.0)
            .await
            .unwrap();
        assert!(reason.is_none());
        assert_eq!(detector.get_statistics().await.total_wallets, 0);
        assert!(!detector.is_wallet_toxic("nobody").await);
    }

    #[tokio::test]
    async fn toxic_rate_empty_and_mixed() {
        let detector = ToxicFlowDetector::new(config(30));
        assert_eq!(detector.get_toxic_rate().await, 0.0);

        detector
            .register_wallet_promotion("w1".to_string(), 0.5)
            .await
            .unwrap();
        detector
            .register_wallet_promotion("w2".to_string(), 0.5)
            .await
            .unwrap();
        detector
            .record_entry("w1".to_string(), false, 0.0)
            .await
            .unwrap();
        assert!((detector.get_toxic_rate().await - 0.5).abs() < 1e-9);
    }

    // ==========================================================================
    // STATISTICS + DISPLAY
    // ==========================================================================

    #[tokio::test]
    async fn statistics_aggregate_counts() {
        let detector = ToxicFlowDetector::new(config(30));
        detector
            .register_wallet_promotion("w1".to_string(), 0.5)
            .await
            .unwrap();
        detector
            .register_wallet_promotion("w2".to_string(), 0.5)
            .await
            .unwrap();
        detector
            .record_entry("w1".to_string(), true, 0.0)
            .await
            .unwrap();
        detector
            .record_entry("w2".to_string(), false, 0.4)
            .await
            .unwrap();

        let stats = detector.get_statistics().await;
        assert_eq!(stats.total_wallets, 2);
        assert_eq!(stats.toxic_wallets, 1);
        assert_eq!(stats.total_entries, 2);
        assert_eq!(stats.local_top_entries, 1);
        assert!((stats.toxic_rate - 0.5).abs() < 1e-9);

        let text = stats.to_string();
        assert!(text.contains("1/2 wallets toxic"));
        assert!(text.contains("1/2 local-top entries"));
    }

    #[tokio::test]
    async fn empty_statistics_display() {
        let detector = ToxicFlowDetector::new(config(30));
        let text = detector.get_statistics().await.to_string();
        assert!(text.contains("0/0 wallets toxic"));
    }

    // ==========================================================================
    // DATABASE ROUND-TRIP (requires TEST_DATABASE_URL)
    // ==========================================================================

    async fn test_pool() -> Option<sqlx::Pool<sqlx::Postgres>> {
        let url = std::env::var("TEST_DATABASE_URL").ok()?;
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .ok()
    }

    /// Collision-resistant unique suffix for rows in the shared test database.
    fn unique_id(prefix: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{}-{}-{}", prefix, std::process::id(), nanos)
    }

    #[tokio::test]
    async fn persist_and_load_round_trip() {
        let Some(pool) = test_pool().await else {
            eprintln!("TEST_DATABASE_URL not set — skipping DB round-trip test");
            return;
        };

        let run_id = unique_id("toxic-test");
        let addr_a = unique_id("toxic-a");
        let addr_b = unique_id("toxic-b");
        sqlx::query("DELETE FROM toxic_wallets WHERE run_id = $1 OR wallet_address = ANY($2)")
            .bind(&run_id)
            .bind(&[addr_a.clone(), addr_b.clone()])
            .execute(&pool)
            .await
            .unwrap();

        let detector = ToxicFlowDetector::new(config(30));
        // RoiDrop wallet.
        detector
            .register_wallet_promotion(addr_a.clone(), 0.6)
            .await
            .unwrap();
        detector
            .record_entry(addr_a.clone(), false, 0.1)
            .await
            .unwrap();
        // LocalTopSqueeze wallet.
        detector
            .register_wallet_promotion(addr_b.clone(), 0.5)
            .await
            .unwrap();
        for _ in 0..3 {
            detector
                .record_entry(addr_b.clone(), true, 0.4)
                .await
                .unwrap();
        }
        // Healthy wallet (must also be persisted, non-toxic).
        detector
            .register_wallet_promotion("healthy".to_string(), 0.5)
            .await
            .unwrap();

        detector.persist_to_database(&pool, &run_id).await.unwrap();

        // Reload from DB in a fresh detector: only the toxic wallets load.
        let reloaded = ToxicFlowDetector::new(config(30));
        reloaded.load_from_database(&pool).await.unwrap();
        assert!(reloaded.is_wallet_toxic(&addr_a).await);
        assert!(reloaded.is_wallet_toxic(&addr_b).await);
        assert!(!reloaded.is_wallet_toxic("healthy").await);

        // Row-level verification: reasons were serialized correctly.
        let rows: Vec<(String, Option<String>, bool)> = sqlx::query_as(
            "SELECT wallet_address, toxic_reason, is_toxic FROM toxic_wallets \
             WHERE wallet_address = ANY($1) ORDER BY wallet_address",
        )
        .bind(&[addr_a.clone(), addr_b.clone()])
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        for (addr, reason, toxic) in rows {
            assert!(toxic);
            if addr == addr_a {
                assert_eq!(reason.as_deref(), Some("roi_drop"));
            } else {
                assert_eq!(reason.as_deref(), Some("local_top_squeeze"));
            }
        }

        sqlx::query("DELETE FROM toxic_wallets WHERE run_id = $1")
            .bind(&run_id)
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn load_skips_stale_or_unknown_reason_rows() {
        let Some(pool) = test_pool().await else {
            eprintln!("TEST_DATABASE_URL not set — skipping DB test");
            return;
        };

        let run_id = unique_id("toxic-stale");
        let stale_addr = unique_id("toxic-stale");
        let unknown_addr = unique_id("toxic-unknown");
        // Clean rows that may exist from prior runs of this test.
        sqlx::query("DELETE FROM toxic_wallets WHERE run_id = $1 OR wallet_address = ANY($2)")
            .bind(&run_id)
            .bind(&[stale_addr.clone(), unknown_addr.clone()])
            .execute(&pool)
            .await
            .unwrap();

        // Stale row: previously toxic but no longer qualifies (ROI recovered).
        sqlx::query(
            "INSERT INTO toxic_wallets \
             (wallet_address, selection_roi, post_promotion_roi, local_top_entries, \
              total_entries, is_toxic, toxic_reason, detected_at, run_id) \
             VALUES ($1, 0.5, 0.5, 1, 3, TRUE, 'roi_drop', NOW(), $2)",
        )
        .bind(&stale_addr)
        .bind(&run_id)
        .execute(&pool)
        .await
        .unwrap();

        // Row with an unknown reason string → defaults to RoiDrop, still toxic.
        sqlx::query(
            "INSERT INTO toxic_wallets \
             (wallet_address, selection_roi, post_promotion_roi, local_top_entries, \
              total_entries, is_toxic, toxic_reason, detected_at, run_id) \
             VALUES ($1, 0.6, 0.0, 0, 1, TRUE, 'some_future_reason', NOW(), $2)",
        )
        .bind(&unknown_addr)
        .bind(&run_id)
        .execute(&pool)
        .await
        .unwrap();

        let detector = ToxicFlowDetector::new(config(30));
        detector.load_from_database(&pool).await.unwrap();

        // The stale wallet must NOT be resurrected.
        assert!(!detector.is_wallet_toxic(&stale_addr).await);
        // The unknown-reason wallet qualifies (deterioration 0.6 > 0.3) and loads.
        assert!(detector.is_wallet_toxic(&unknown_addr).await);

        sqlx::query("DELETE FROM toxic_wallets WHERE run_id = $1")
            .bind(&run_id)
            .execute(&pool)
            .await
            .unwrap();
    }
}
