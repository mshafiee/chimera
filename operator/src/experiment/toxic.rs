use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use sqlx::SqlitePool;

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
        wallets.insert(
            wallet.clone(),
            ToxicWallet {
                address: wallet,
                selection_roi,
                post_promotion_roi: selection_roi,
                local_top_entries: 0,
                total_entries: 0,
                is_toxic: false,
                toxic_reason: None,
                detected_at: None,
            },
        );
        
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
                w.is_toxic = true;
                w.toxic_reason = Some(reason.clone());
                w.detected_at = Some(chrono::Utc::now());
                
                warn!("Wallet {} flagged as toxic: {:?}", wallet, reason);
                return Ok(Some(reason));
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
        if wallet.local_top_entries >= 3 && wallet.local_top_entries >= (wallet.total_entries / 2) {
            return true;
        }
        
        false
    }

    fn determine_toxic_reason(&self, wallet: &ToxicWallet) -> ToxicReason {
        let roi_deterioration = wallet.selection_roi - wallet.post_promotion_roi;
        
        if roi_deterioration > (self.config.toxic_threshold_percent as f64) / 100.0 {
            return ToxicReason::RoiDrop;
        }
        
        if wallet.local_top_entries >= 3 && wallet.local_top_entries >= (wallet.total_entries / 2) {
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

    pub async fn persist_to_database(&self, pool: &SqlitePool, run_id: &str) -> AppResult<()> {
        let wallets = self.wallets.read().await;
        
        for wallet in wallets.values() {
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
                    updated_at = CURRENT_TIMESTAMP
                "#
            )
            .bind(&wallet.address)
            .bind(wallet.selection_roi)
            .bind(wallet.post_promotion_roi)
            .bind(wallet.local_top_entries as i64)
            .bind(wallet.total_entries as i64)
            .bind(wallet.is_toxic as i32)
            .bind(toxic_reason_str)
            .bind(wallet.detected_at.map(|dt| dt.to_rfc3339()))
            .bind(run_id)
            .execute(pool)
            .await?;
        }
        
        info!("Persisted {} toxic wallet records to database", wallets.len());
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
