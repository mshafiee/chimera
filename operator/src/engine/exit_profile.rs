//! Per-wallet exit profiles.
//!
//! Derives per-wallet time-exit and trailing-stop parameters from a wallet's
//! on-chain round-trip behavior (median hold duration, median win/loss size)
//! and blends them with the global `ProfitManagementConfig` via Bayesian
//! shrinkage: `weight = samples / (samples + K)`. A wallet with zero samples
//! uses the global params unchanged; a wallet with K samples is 50% trusted;
//! 200 samples is ~89% trusted.
//!
//! Loss-side parameters (hard stop, recovery gate, wick protection) are NOT
//! per-wallet — they are safety rails and stay global.
//!
//! Data source: the on-chain assessment (Helius enhanced SWAP history, up to
//! 200 txs per wallet) that already runs for the admission gate and the
//! retroactive ACTIVE audit. Profiles are persisted to `wallet_exit_profiles`
//! after each assessment — zero extra API cost.

use std::collections::HashMap;
use std::sync::Arc;

use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use tokio::sync::RwLock;

use crate::config::{ExitProfileConfig, ProfitManagementConfig};
use crate::db_abstraction::Database;
use crate::error::AppResult;
use crate::engine::onchain_assessment::OnchainWalletAssessment;

/// Raw per-wallet exit statistics derived from on-chain round trips.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WalletExitStats {
    /// Completed round trips used to build this profile.
    pub samples: usize,
    /// Median buy->sell hold duration (seconds), if any round trip had both.
    pub median_hold_secs: Option<i64>,
    pub avg_hold_secs: Option<i64>,
    pub win_rate_pct: f64,
    pub median_win_pct: f64,
    pub median_loss_pct: f64,
    pub avg_win_pct: f64,
    pub avg_loss_pct: f64,
    /// Gross wins / |gross losses|.
    pub profit_factor: f64,
}

impl WalletExitStats {
    /// Convert from an on-chain assessment (shares the same round-trip data).
    pub fn from_assessment(a: &OnchainWalletAssessment) -> Self {
        Self {
            samples: a.round_trips,
            median_hold_secs: a.median_hold_secs,
            avg_hold_secs: a.avg_hold_secs,
            win_rate_pct: a.win_rate_pct,
            median_win_pct: a.median_win_pct,
            median_loss_pct: a.median_loss_pct,
            avg_win_pct: a.avg_win_pct,
            avg_loss_pct: a.avg_loss_pct,
            profit_factor: a.profit_factor,
        }
    }

    /// True when the profile has enough samples to influence exits.
    pub fn usable(&self, min_samples: usize) -> bool {
        self.samples >= min_samples
    }
}

/// Effective per-wallet exit parameters, blended from the wallet's profile
/// and the global config via Bayesian shrinkage. When no usable profile
/// exists these equal the global defaults (behavior unchanged).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectiveExitParams {
    /// Time exit (hours) for the high-profit tier (>25%), strategy-adjusted.
    pub high_profit_hours: u64,
    /// Time exit (hours) for the medium-profit tier (>10%), strategy-adjusted.
    pub medium_profit_hours: u64,
    /// Trailing stop activation (percent of profit), pre-volatility-scaling.
    pub trailing_activation_pct: Decimal,
    /// Trailing stop distance (percent from peak), pre-STRATEGY-multiplier.
    pub trailing_distance_pct: Decimal,
}

impl EffectiveExitParams {
    /// Global defaults — identical to the pre-profile behavior.
    pub fn from_config(cfg: &ProfitManagementConfig, strategy: &str) -> Self {
        let is_spear = strategy == "SPEAR";
        Self {
            high_profit_hours: if is_spear { 24 } else { 48 },
            medium_profit_hours: if is_spear {
                12
            } else {
                cfg.time_exit_hours
            },
            trailing_activation_pct: cfg.trailing_stop_activation,
            trailing_distance_pct: cfg.trailing_stop_distance,
        }
    }
}

/// Bayesian shrinkage weight for a sample count.
fn weight(samples: usize, k: f64) -> f64 {
    let s = samples as f64;
    if k <= 0.0 || s <= 0.0 {
        return 0.0;
    }
    s / (s + k)
}

fn blend(global: f64, wallet: f64, w: f64) -> f64 {
    global * (1.0 - w) + wallet * w
}

/// Compute the per-wallet hold-time multiplier: median round-trip hold vs a
/// reference hold time (default 12h), blended with 1.0 and clamped.
fn hold_multiplier(cfg: &ExitProfileConfig, stats: &WalletExitStats) -> f64 {
    let wallet_mult = match stats.median_hold_secs {
        Some(h) if h > 0 && cfg.reference_hold_hours > 0.0 => {
            (h as f64 / 3600.0) / cfg.reference_hold_hours
        }
        _ => 1.0,
    };
    let w = weight(stats.samples, cfg.shrinkage_k);
    blend(1.0, wallet_mult, w).clamp(cfg.hold_mult_min, cfg.hold_mult_max)
}

/// Effective per-wallet exit params, or global defaults when the wallet has
/// no usable profile (or the feature is disabled).
pub fn effective_params(
    cfg: &ExitProfileConfig,
    global: &ProfitManagementConfig,
    stats: Option<&WalletExitStats>,
    strategy: &str,
) -> EffectiveExitParams {
    if !cfg.enabled {
        return EffectiveExitParams::from_config(global, strategy);
    }
    let Some(stats) = stats else {
        return EffectiveExitParams::from_config(global, strategy);
    };
    if !stats.usable(cfg.min_samples) {
        return EffectiveExitParams::from_config(global, strategy);
    }

    let is_spear = strategy == "SPEAR";
    let m = hold_multiplier(cfg, stats);
    let mult = |base: f64| -> u64 {
        ((base * m).round() as i64).clamp(1, 168) as u64
    };

    let base_high = if is_spear { 24.0 } else { 48.0 };
    let base_medium = if is_spear {
        12.0
    } else {
        global.time_exit_hours as f64
    };

    // Trailing distance: wallet winners/losers magnitude * 0.3, blended and
    // clamped to [min, max]. This is the per-wallet volatility adaptation —
    // wide swings get wide trails so normal retraces don't shake out.
    let w = weight(stats.samples, cfg.shrinkage_k);
    let raw_dist = stats
        .median_win_pct
        .max(stats.median_loss_pct.abs());
    let wallet_dist = (raw_dist * 0.3)
        .clamp(cfg.trailing_min_distance_pct, cfg.trailing_max_distance_pct);
    let distance = blend(
        global.trailing_stop_distance.to_f64().unwrap_or(10.0),
        wallet_dist,
        w,
    )
    .clamp(cfg.trailing_min_distance_pct, cfg.trailing_max_distance_pct);

    // Trailing activation: NEVER above the global value. Data (2026-08-05):
    // 60% of shadow wins ended via 4h time_exit at +0.94% avg because the
    // +5% activation never engaged on winners peaking in the 0-5% band, and
    // the old median_win*0.5 formula pushed ACTIVE whales' activation to
    // 16-19% (outlier-dominated medians). Early activation is harmless when
    // the distance is wide — the distance does the per-wallet adaptation.
    let act_global = global
        .trailing_stop_activation
        .to_f64()
        .unwrap_or(5.0)
        .max(cfg.trailing_min_activation_pct);
    let wallet_act = (stats.median_win_pct * 0.35)
        .clamp(cfg.trailing_min_activation_pct, act_global);
    let activation = blend(act_global, wallet_act, w)
        .clamp(cfg.trailing_min_activation_pct, cfg.trailing_max_activation_pct);

    EffectiveExitParams {
        high_profit_hours: mult(base_high),
        medium_profit_hours: mult(base_medium),
        trailing_activation_pct: Decimal::from_f64_retain(activation)
            .unwrap_or(global.trailing_stop_activation),
        trailing_distance_pct: Decimal::from_f64_retain(distance)
            .unwrap_or(global.trailing_stop_distance),
    }
}

/// In-memory cache of per-wallet exit profiles, refreshed from the DB.
pub struct ExitProfileCache {
    config: ExitProfileConfig,
    global: Arc<ProfitManagementConfig>,
    db: Arc<dyn Database>,
    inner: RwLock<HashMap<String, WalletExitStats>>,
}

impl ExitProfileCache {
    pub fn new(
        db: Arc<dyn Database>,
        global: Arc<ProfitManagementConfig>,
        config: ExitProfileConfig,
    ) -> Self {
        Self {
            config,
            global,
            db,
            inner: RwLock::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> &ExitProfileConfig {
        &self.config
    }

    /// Reload all profiles from the DB.
    pub async fn refresh(&self) -> AppResult<usize> {
        use crate::db_abstraction::DbPool;

        let DbPool::PostgreSQL(pool) = self.db.pool();

        #[derive(sqlx::FromRow)]
        struct Row {
            wallet_address: String,
            samples: i32,
            median_hold_secs: Option<i64>,
            avg_hold_secs: Option<i64>,
            win_rate_pct: Option<f64>,
            median_win_pct: Option<f64>,
            median_loss_pct: Option<f64>,
            avg_win_pct: Option<f64>,
            avg_loss_pct: Option<f64>,
            profit_factor: Option<f64>,
        }

        let rows: Vec<Row> = sqlx::query_as(
            r#"
            SELECT wallet_address, samples, median_hold_secs, avg_hold_secs,
                   win_rate_pct, median_win_pct, median_loss_pct,
                   avg_win_pct, avg_loss_pct, profit_factor
            FROM wallet_exit_profiles
            "#,
        )
        .fetch_all(&pool)
        .await
        .map_err(|e| crate::error::AppError::Internal(format!("exit profile load failed: {e}")))?;

        let mut map = HashMap::with_capacity(rows.len());
        for r in rows {
            map.insert(
                r.wallet_address,
                WalletExitStats {
                    samples: r.samples.max(0) as usize,
                    median_hold_secs: r.median_hold_secs,
                    avg_hold_secs: r.avg_hold_secs,
                    win_rate_pct: r.win_rate_pct.unwrap_or(0.0),
                    median_win_pct: r.median_win_pct.unwrap_or(0.0),
                    median_loss_pct: r.median_loss_pct.unwrap_or(0.0),
                    avg_win_pct: r.avg_win_pct.unwrap_or(0.0),
                    avg_loss_pct: r.avg_loss_pct.unwrap_or(0.0),
                    profit_factor: r.profit_factor.unwrap_or(0.0),
                },
            );
        }

        let n = map.len();
        *self.inner.write().await = map;
        Ok(n)
    }

    /// Raw stats for a wallet, if any.
    pub async fn stats(&self, wallet: &str) -> Option<WalletExitStats> {
        self.inner.read().await.get(wallet).cloned()
    }

    /// Effective per-wallet exit params (cheap, no DB).
    pub async fn effective(&self, wallet: &str, strategy: &str) -> EffectiveExitParams {
        let stats = self.inner.read().await.get(wallet).cloned();
        effective_params(&self.config, &self.global, stats.as_ref(), strategy)
    }
}

/// Upsert a wallet's exit profile after an assessment.
pub async fn upsert_exit_profile(
    pool: &sqlx::PgPool,
    wallet: &str,
    stats: &WalletExitStats,
) -> AppResult<()> {
    sqlx::query(
        r#"
        INSERT INTO wallet_exit_profiles
            (wallet_address, samples, median_hold_secs, avg_hold_secs,
             win_rate_pct, median_win_pct, median_loss_pct,
             avg_win_pct, avg_loss_pct, profit_factor, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW())
        ON CONFLICT (wallet_address) DO UPDATE SET
            samples = EXCLUDED.samples,
            median_hold_secs = EXCLUDED.median_hold_secs,
            avg_hold_secs = EXCLUDED.avg_hold_secs,
            win_rate_pct = EXCLUDED.win_rate_pct,
            median_win_pct = EXCLUDED.median_win_pct,
            median_loss_pct = EXCLUDED.median_loss_pct,
            avg_win_pct = EXCLUDED.avg_win_pct,
            avg_loss_pct = EXCLUDED.avg_loss_pct,
            profit_factor = EXCLUDED.profit_factor,
            updated_at = NOW()
        "#,
    )
    .bind(wallet)
    .bind(stats.samples as i32)
    .bind(stats.median_hold_secs)
    .bind(stats.avg_hold_secs)
    .bind(stats.win_rate_pct)
    .bind(stats.median_win_pct)
    .bind(stats.median_loss_pct)
    .bind(stats.avg_win_pct)
    .bind(stats.avg_loss_pct)
    .bind(stats.profit_factor)
    .execute(pool)
    .await
    .map_err(|e| crate::error::AppError::Internal(format!("exit profile upsert failed: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> ExitProfileConfig {
        ExitProfileConfig::default()
    }

    fn test_global() -> ProfitManagementConfig {
        ProfitManagementConfig::default()
    }

    fn stats(samples: usize, median_hold_secs: Option<i64>, win_pct: f64, loss_pct: f64) -> WalletExitStats {
        WalletExitStats {
            samples,
            median_hold_secs,
            median_win_pct: win_pct,
            median_loss_pct: loss_pct,
            ..Default::default()
        }
    }

    #[test]
    fn test_no_profile_uses_global() {
        let cfg = test_config();
        let global = test_global();
        let e = effective_params(&cfg, &global, None, "SHIELD");
        assert_eq!(e.high_profit_hours, 48);
        assert_eq!(e.medium_profit_hours, global.time_exit_hours);
        assert_eq!(e.trailing_activation_pct, global.trailing_stop_activation);
        assert_eq!(e.trailing_distance_pct, global.trailing_stop_distance);
    }

    #[test]
    fn test_spear_defaults() {
        let cfg = test_config();
        let global = test_global();
        let e = effective_params(&cfg, &global, None, "SPEAR");
        assert_eq!(e.high_profit_hours, 24);
        assert_eq!(e.medium_profit_hours, 12);
    }

    #[test]
    fn test_below_min_samples_uses_global() {
        let cfg = test_config();
        let global = test_global();
        let s = stats(3, Some(3600), 5.0, -2.0); // below min_samples=5
        let e = effective_params(&cfg, &global, Some(&s), "SHIELD");
        assert_eq!(e.medium_profit_hours, global.time_exit_hours);
    }

    #[test]
    fn test_short_holder_gets_shorter_time_exit() {
        let cfg = test_config();
        let global = test_global();
        // Median hold 2h vs reference 12h -> wallet mult 1/6, blended.
        let s = stats(200, Some(7200), 4.0, -2.0);
        let e = effective_params(&cfg, &global, Some(&s), "SHIELD");
        assert!(
            e.medium_profit_hours < global.time_exit_hours,
            "scalper time exit {} should be shorter than global {}",
            e.medium_profit_hours,
            global.time_exit_hours
        );
        assert!(e.medium_profit_hours >= 1);
    }

    #[test]
    fn test_long_holder_gets_longer_time_exit() {
        let cfg = test_config();
        let global = test_global();
        // Median hold 3 days -> wallet mult 6, clamped to 4.0.
        let s = stats(200, Some(3 * 86400), 30.0, -8.0);
        let e = effective_params(&cfg, &global, Some(&s), "SHIELD");
        assert!(e.high_profit_hours >= 48);
        assert!(e.high_profit_hours <= 168);
    }

    #[test]
    fn test_hold_multiplier_clamped() {
        let cfg = test_config();
        // Extremely short holder (30s median hold) — mult must not go below min.
        let s = stats(200, Some(30), 4.0, -2.0);
        let m = hold_multiplier(&cfg, &s);
        assert!(m >= cfg.hold_mult_min && m <= cfg.hold_mult_max);
        // Extremely long holder — must not exceed max.
        let s2 = stats(200, Some(30 * 86400), 4.0, -2.0);
        let m2 = hold_multiplier(&cfg, &s2);
        assert!(m2 >= cfg.hold_mult_min && m2 <= cfg.hold_mult_max);
    }

    #[test]
    fn test_trailing_distance_clamped() {
        let cfg = test_config();
        let global = test_global();
        // Tiny winners -> distance at min; huge winners -> distance at max.
        let s_small = stats(200, Some(3600), 1.0, -1.0);
        let e_small = effective_params(&cfg, &global, Some(&s_small), "SHIELD");
        assert!(e_small.trailing_distance_pct >= Decimal::from_f64_retain(cfg.trailing_min_distance_pct).unwrap());

        let s_big = stats(200, Some(3600), 500.0, -40.0);
        let e_big = effective_params(&cfg, &global, Some(&s_big), "SHIELD");
        assert!(e_big.trailing_distance_pct <= Decimal::from_f64_retain(cfg.trailing_max_distance_pct).unwrap());
    }

    #[test]
    fn test_activation_never_exceeds_global() {
        // A whale with an outlier-dominated median win must NOT get a late
        // trailing activation — 60% of shadow wins were time_exits at +0.94%
        // because the old median_win*0.5 formula pushed activation to 16-19%.
        let cfg = test_config();
        let global = test_global();
        let s = stats(100, Some(3600), 500.0, -40.0);
        let e = effective_params(&cfg, &global, Some(&s), "SHIELD");
        assert!(
            e.trailing_activation_pct <= global.trailing_stop_activation,
            "activation {} must not exceed global {}",
            e.trailing_activation_pct,
            global.trailing_stop_activation
        );
        // Distance still adapts per wallet (volatility), independent of activation.
        assert!(
            e.trailing_distance_pct > global.trailing_stop_distance,
            "wide-swing wallet should keep a wider trailing distance"
        );
    }

    #[test]
    fn test_weight_shrinkage() {
        assert_eq!(weight(0, 25.0), 0.0);
        assert!((weight(25, 25.0) - 0.5).abs() < 1e-9);
        assert!((weight(200, 25.0) - 200.0 / 225.0).abs() < 1e-9);
        assert_eq!(weight(10, 0.0), 0.0);
    }

    #[test]
    fn test_disabled_uses_global() {
        let mut cfg = test_config();
        cfg.enabled = false;
        let global = test_global();
        let s = stats(200, Some(7200), 4.0, -2.0);
        let e = effective_params(&cfg, &global, Some(&s), "SHIELD");
        assert_eq!(e.medium_profit_hours, global.time_exit_hours);
    }
}
