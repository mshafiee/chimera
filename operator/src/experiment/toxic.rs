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

    pub async fn register_wallet_promotion(&self, wallet: String, selection_roi: f64) -> AppResult<()> {
        debug!("Registering wallet promotion: {} with ROI: {}", wallet, selection_roi);

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
        wallets
            .get(wallet)
            .map(|w| w.is_toxic)
            .unwrap_or(false)
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

    pub async fn persist_to_database(&self, pool: &sqlx::Pool<sqlx::Postgres>, run_id: &str) -> AppResult<()> {
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
                "#
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

        info!("Persisted {} toxic wallet records to database", wallets.len());
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

            wallets.insert(
                row.wallet_address.clone(),
                ToxicWallet {
                    address: row.wallet_address,
                    selection_roi: row.selection_roi,
                    post_promotion_roi: row.post_promotion_roi,
                    local_top_entries: row.local_top_entries as u32,
                    total_entries: row.total_entries as u32,
                    is_toxic: true,
                    toxic_reason,
                    detected_at: row.detected_at,
                },
            );
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
