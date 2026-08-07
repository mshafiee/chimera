//! Rejection-rate wallet mute.
//!
//! Tracks per-wallet BUY-decision rejection rates over a rolling window.
//! When a wallet's *hard* rejection rate exceeds a threshold, the wallet is
//! time-boxed muted — its signals are short-circuited before any expensive
//! token-safety or liquidity checks, eliminating wasted processing.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::config::RejectionMuteConfig;
use crate::error::AppResult;

/// Rejection codes that indicate the wallet fundamentally trades untradeable
/// assets. These count toward the mute threshold.
const HARD_REJECTION_CODES: &[&str] = &[
    "NON_SPECULATIVE_TOKEN",
    "TOKEN_UNSAFE",
    "PUMPFUN_INSUFFICIENT_LIQUIDITY",
    "PUMPFUN_BONDING_CURVE",
    "INVALID_TOKEN_ADDRESS",
    "TOKEN_FAST_CHECK_ERRORED",
];

#[derive(Debug, Clone)]
struct MutedWallet {
    address: String,
    window: VecDeque<bool>,
    is_muted: bool,
    muted_at: Option<DateTime<Utc>>,
    muted_until: Option<DateTime<Utc>>,
}

impl MutedWallet {
    fn new(address: String) -> Self {
        Self {
            address,
            window: VecDeque::new(),
            is_muted: false,
            muted_at: None,
            muted_until: None,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct MutedWalletRow {
    wallet_address: String,
    muted_at: Option<DateTime<Utc>>,
    muted_until: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct RejectionMuteDetector {
    wallets: Arc<RwLock<HashMap<String, MutedWallet>>>,
    config: RejectionMuteConfig,
}

impl RejectionMuteDetector {
    pub fn new(config: RejectionMuteConfig) -> Self {
        Self {
            wallets: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Classify a rejection code as "hard" (structural, untradeable asset).
    pub fn is_hard_rejection(code: &str) -> bool {
        HARD_REJECTION_CODES.contains(&code)
    }

    /// Record a BUY decision outcome for a wallet.
    ///
    /// Called from `SelectionService::decide()` after the decision is finalized.
    /// Maintains a rolling window of the last `window_size` decisions. If the
    /// hard-rejection rate exceeds the threshold, the wallet is muted.
    pub async fn record_decision(
        &self,
        wallet: &str,
        admitted: bool,
        rejection_code: Option<&str>,
    ) -> AppResult<()> {
        if !self.config.enabled {
            return Ok(());
        }

        let mut wallets = self.wallets.write().await;
        let entry = wallets
            .entry(wallet.to_string())
            .or_insert_with(|| MutedWallet::new(wallet.to_string()));

        // If currently muted, check for expiry.
        if entry.is_muted {
            if let Some(until) = entry.muted_until {
                if Utc::now() >= until {
                    // Mute expired — reset for a fresh evaluation window.
                    entry.is_muted = false;
                    entry.muted_at = None;
                    entry.muted_until = None;
                    entry.window.clear();
                    // Fall through to record this signal in the fresh window.
                } else {
                    // Still muted — do not pollute the frozen window.
                    return Ok(());
                }
            }
        }

        let is_hard = !admitted && rejection_code.map(Self::is_hard_rejection).unwrap_or(false);

        // Push to rolling window, evicting oldest if over capacity.
        entry.window.push_back(is_hard);
        if entry.window.len() > self.config.window_size as usize {
            entry.window.pop_front();
        }

        // Evaluate mute condition.
        if !entry.is_muted && entry.window.len() >= self.config.min_window_samples as usize {
            let hard = entry.window.iter().filter(|&&h| h).count();
            let rate = hard as f64 / entry.window.len() as f64;
            if rate >= self.config.hard_rejection_threshold {
                let now = Utc::now();
                let until = now + ChronoDuration::hours(self.config.mute_duration_hours as i64);
                entry.is_muted = true;
                entry.muted_at = Some(now);
                entry.muted_until = Some(until);
                warn!(
                    wallet = %wallet,
                    hard_rate = format!("{:.0}%", rate * 100.0),
                    window = entry.window.len(),
                    muted_until = %until,
                    "RejectionMuteDetector: wallet muted (high hard-rejection rate)"
                );
            }
        }

        Ok(())
    }

    /// Gate check: is this wallet currently muted?
    ///
    /// Read-only; called on the hot path in `decide_buy()`. Expiry is handled
    /// lazily — a muted wallet whose `muted_until` has passed returns `false`
    /// here and its window resets on the next `record_decision` call.
    pub async fn is_wallet_muted(&self, wallet: &str) -> bool {
        if !self.config.enabled {
            return false;
        }
        let wallets = self.wallets.read().await;
        if let Some(w) = wallets.get(wallet) {
            if w.is_muted {
                if let Some(until) = w.muted_until {
                    return Utc::now() < until;
                }
                return true;
            }
        }
        false
    }

    /// List all currently-muted wallet addresses (for diagnostics / API).
    pub async fn get_muted_wallets(&self) -> Vec<String> {
        let wallets = self.wallets.read().await;
        wallets
            .values()
            .filter(|w| {
                w.is_muted
                    && w.muted_until
                        .map(|until| Utc::now() < until)
                        .unwrap_or(true)
            })
            .map(|w| w.address.clone())
            .collect()
    }

    /// Persist all tracked wallet state to the database (UPSERT).
    /// Snapshots under a short read lock, then does I/O lock-free.
    pub async fn persist_to_database(
        &self,
        pool: &sqlx::Pool<sqlx::Postgres>,
        run_id: &str,
    ) -> AppResult<()> {
        let snapshot: Vec<MutedWallet> = {
            let wallets = self.wallets.read().await;
            wallets.values().cloned().collect()
        };

        for wallet in &snapshot {
            sqlx::query(
                r#"
                INSERT INTO muted_wallets (
                    wallet_address, is_muted, muted_at, muted_until, window_size, run_id
                ) VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (wallet_address) DO UPDATE SET
                    is_muted     = excluded.is_muted,
                    muted_at     = excluded.muted_at,
                    muted_until  = excluded.muted_until,
                    window_size  = excluded.window_size,
                    run_id       = excluded.run_id,
                    updated_at   = CURRENT_TIMESTAMP
                "#,
            )
            .bind(&wallet.address)
            .bind(wallet.is_muted)
            .bind(wallet.muted_at)
            .bind(wallet.muted_until)
            .bind(wallet.window.len() as i64)
            .bind(run_id)
            .execute(pool)
            .await?;
        }

        info!("Persisted {} muted-wallet records to database", snapshot.len());
        Ok(())
    }

    /// Load still-active mutes from the database on startup.
    /// Only restores wallets where `is_muted = TRUE AND muted_until > NOW()`.
    /// The rolling window starts empty (fresh evaluation after the mute lapses).
    pub async fn load_from_database(&self, pool: &sqlx::Pool<sqlx::Postgres>) -> AppResult<()> {
        let rows = sqlx::query_as::<_, MutedWalletRow>(
            r#"
            SELECT wallet_address, muted_at, muted_until
            FROM muted_wallets
            WHERE is_muted = TRUE AND muted_until > NOW()
            "#,
        )
        .fetch_all(pool)
        .await?;

        let mut wallets = self.wallets.write().await;
        let count = rows.len();
        for row in rows {
            wallets.insert(
                row.wallet_address.clone(),
                MutedWallet {
                    address: row.wallet_address,
                    window: VecDeque::new(),
                    is_muted: true,
                    muted_at: row.muted_at,
                    muted_until: row.muted_until,
                },
            );
        }
        if count > 0 {
            warn!("Loaded {} active muted wallets from database on startup", count);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> RejectionMuteConfig {
        RejectionMuteConfig {
            enabled: true,
            window_size: 10,
            min_window_samples: 5,
            hard_rejection_threshold: 0.80,
            mute_duration_hours: 6,
        }
    }

    #[tokio::test]
    async fn test_hard_rejection_classification() {
        assert!(RejectionMuteDetector::is_hard_rejection(
            "NON_SPECULATIVE_TOKEN"
        ));
        assert!(RejectionMuteDetector::is_hard_rejection("TOKEN_UNSAFE"));
        assert!(RejectionMuteDetector::is_hard_rejection(
            "PUMPFUN_INSUFFICIENT_LIQUIDITY"
        ));
        assert!(!RejectionMuteDetector::is_hard_rejection(
            "SIGNAL_QUALITY_TOO_LOW"
        ));
        assert!(!RejectionMuteDetector::is_hard_rejection("WQS_TOO_LOW"));
        assert!(!RejectionMuteDetector::is_hard_rejection("WALLET_MUTED"));
    }

    #[tokio::test]
    async fn test_mute_after_threshold_hard_rejections() {
        let det = RejectionMuteDetector::new(test_config());
        // 9 hard rejections, 1 admitted = 90% hard rate, >= min_samples(5), >= threshold(80%)
        for _ in 0..9 {
            det.record_decision("walletA", false, Some("TOKEN_UNSAFE"))
                .await
                .unwrap();
        }
        det.record_decision("walletA", true, None).await.unwrap();
        assert!(
            det.is_wallet_muted("walletA").await,
            "wallet should be muted at 90% hard rate"
        );
    }

    #[tokio::test]
    async fn test_no_mute_below_min_samples() {
        let det = RejectionMuteDetector::new(test_config()); // min_samples = 5
        // Only 4 hard rejections — below min_samples(5)
        for _ in 0..4 {
            det.record_decision("walletB", false, Some("NON_SPECULATIVE_TOKEN"))
                .await
                .unwrap();
        }
        assert!(
            !det.is_wallet_muted("walletB").await,
            "should NOT mute below min_samples"
        );
    }

    #[tokio::test]
    async fn test_no_mute_when_rate_below_threshold() {
        let det = RejectionMuteDetector::new(test_config()); // threshold = 80%
        // 3 hard, 7 soft = 30% — well below threshold
        for _ in 0..3 {
            det.record_decision("walletC", false, Some("TOKEN_UNSAFE"))
                .await
                .unwrap();
        }
        for _ in 0..7 {
            det.record_decision("walletC", false, Some("SIGNAL_QUALITY_TOO_LOW"))
                .await
                .unwrap();
        }
        assert!(
            !det.is_wallet_muted("walletC").await,
            "should NOT mute at 30% hard rate"
        );
    }

    #[tokio::test]
    async fn test_admitted_signals_dilute_rate() {
        let det = RejectionMuteDetector::new(test_config()); // threshold = 80%, window = 10
        // 3 admitted + 7 hard = 70% — below threshold.
        // Admits recorded first so the window fills before the mute check
        // can trigger on a transient 100% spike.
        for _ in 0..3 {
            det.record_decision("walletD", true, None).await.unwrap();
        }
        for _ in 0..7 {
            det.record_decision("walletD", false, Some("TOKEN_UNSAFE"))
                .await
                .unwrap();
        }
        assert!(
            !det.is_wallet_muted("walletD").await,
            "3 admits should dilute to 70% — not muted"
        );
    }

    #[tokio::test]
    async fn test_disabled_never_mutes() {
        let mut cfg = test_config();
        cfg.enabled = false;
        let det = RejectionMuteDetector::new(cfg);
        for _ in 0..10 {
            det.record_decision("walletE", false, Some("TOKEN_UNSAFE"))
                .await
                .unwrap();
        }
        assert!(
            !det.is_wallet_muted("walletE").await,
            "disabled detector should never mute"
        );
    }

    #[tokio::test]
    async fn test_muted_wallet_freezes_window() {
        let det = RejectionMuteDetector::new(test_config()); // window=10, threshold=80%, min=5
        // Mute the wallet with 5 hard rejections (100%)
        for _ in 0..5 {
            det.record_decision("walletF", false, Some("TOKEN_UNSAFE"))
                .await
                .unwrap();
        }
        assert!(det.is_wallet_muted("walletF").await);
        // Send 5 more signals while muted — should NOT change anything
        for _ in 0..5 {
            det.record_decision("walletF", true, None).await.unwrap();
        }
        assert!(det.is_wallet_muted("walletF").await, "still muted");
    }

    #[tokio::test]
    async fn test_mute_expires_after_duration() {
        let mut cfg = test_config();
        cfg.mute_duration_hours = 0; // expires immediately (0 hours)
        let det = RejectionMuteDetector::new(cfg);
        for _ in 0..5 {
            det.record_decision("walletG", false, Some("TOKEN_UNSAFE"))
                .await
                .unwrap();
        }
        // The mute was set; muted_until is ~now, so next check may be expired.
        // record_decision should reset on the next call after expiry.
        // Give it a signal to trigger the expiry reset path:
        det.record_decision("walletG", true, None).await.unwrap();
        assert!(
            !det.is_wallet_muted("walletG").await,
            "wallet should be unmuted after 0h expiry + new signal"
        );
    }
}
