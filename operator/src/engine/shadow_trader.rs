use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::config::ProfitManagementConfig;
use crate::db_abstraction::{Database, DbPool};
use crate::engine::selection::{BuyDecision, SelectionRequest};
use crate::models::Action;
use crate::price_cache::PriceCache;

const EXIT_STRATEGIES: [&str; 5] = [
    "mirror_main",
    "fixed_1h",
    "fixed_4h",
    "fixed_24h",
    "wallet_sell",
];

const FIXED_HOLDS_SECS: [i64; 3] = [3600, 14400, 86400];

#[derive(Clone)]
pub struct ShadowConfig {
    pub enabled: bool,
    pub position_size_sol: Decimal,
    pub max_lifetime: Duration,
    pub profit_config: Arc<ProfitManagementConfig>,
    pub run_id: String,
}

impl ShadowConfig {
    pub fn from_env(
        profit_config: Arc<ProfitManagementConfig>,
        run_id: String,
    ) -> Self {
        let enabled = std::env::var("CHIMERA_SHADOW_TRADER_ENABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(true);

        let position_size_sol = std::env::var("CHIMERA_SHADOW_POSITION_SIZE_SOL")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| Decimal::from_f64_retain(v).unwrap_or(dec!(1.0)))
            .unwrap_or(dec!(1.0));

        let max_lifetime_hours = std::env::var("CHIMERA_SHADOW_MAX_LIFETIME_HOURS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(168);

        Self {
            enabled,
            position_size_sol,
            max_lifetime: Duration::from_secs(max_lifetime_hours * 3600),
            profit_config,
            run_id,
        }
    }
}

pub struct ShadowTrader {
    db: Arc<dyn Database>,
    price_cache: Arc<PriceCache>,
    config: ShadowConfig,
    peaks: Arc<tokio::sync::Mutex<HashMap<String, Decimal>>>,
}

impl ShadowTrader {
    pub fn new(
        db: Arc<dyn Database>,
        price_cache: Arc<PriceCache>,
        config: ShadowConfig,
    ) -> Self {
        Self {
            db,
            price_cache,
            config,
            peaks: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn on_signal(&self, decision: &BuyDecision, req: &SelectionRequest) {
        if !self.config.enabled {
            return;
        }

        let db = self.db.clone();
        let price_cache = self.price_cache.clone();
        let config = self.config.clone();
        let peaks = self.peaks.clone();
        let decision = decision.clone();
        let req = req.clone();

        tokio::spawn(async move {
            match req.action {
                Action::Buy => {
                    Self::open_shadow_position(db, price_cache, &config, &decision, &req, &peaks).await;
                }
                Action::Sell => {
                    Self::on_wallet_sell(db, price_cache, &config, &req).await;
                }
            }
        });
    }

    async fn open_shadow_position(
        db: Arc<dyn Database>,
        price_cache: Arc<PriceCache>,
        config: &ShadowConfig,
        decision: &BuyDecision,
        req: &SelectionRequest,
        peaks: &Arc<tokio::sync::Mutex<HashMap<String, Decimal>>>,
    ) {
        let shadow_id = uuid::Uuid::new_v4().to_string();
        let sol_price = price_cache.get_sol_price_usd();

        // Track the token in the price cache so the background updater keeps a
        // live price for the shadow monitor loop. Without this, get_price_usd
        // always returns None for tokens the main system has never opened a
        // position on (e.g. rejected signals), so every shadow position was
        // recorded with no entry price and no evaluable PnL.
        price_cache.track_token(&req.token_address);

        // Eagerly fetch a fresh price — mirrors the main system's behavior
        // after opening a position (signal_pipeline.rs). Waits briefly for the
        // async fetch to land so the entry price is the live decision-time
        // price, not a stale/None value.
        let mut entry_price = price_cache.get_price_usd(&req.token_address);
        if entry_price.is_none() {
            price_cache.eager_fetch_token(&req.token_address).await;
            for _ in 0..10 {
                if let Some(p) = price_cache.get_price_usd(&req.token_address) {
                    entry_price = Some(p);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }

        let entry_price_usd = match entry_price {
            Some(p) => p,
            None => {
                tracing::debug!(
                    shadow_id = %shadow_id,
                    token = %req.token_address,
                    wallet = %req.wallet_address,
                    "Shadow: no price available even after eager fetch, skipping position"
                );
                Self::insert_no_price_exits(&db, &shadow_id, decision, req, &config.run_id, config.position_size_sol).await;
                return;
            }
        };

        let strategy_str = decision.strategy.map(|s| s.to_string());
        let wqs = decision.wqs;
        let quality_score = decision.quality_score;
        let liquidity_usd = decision.liquidity_usd;
        let consensus_count = decision.consensus_wallet_count.and_then(|c| {
            i32::try_from(c).ok()
        });

        {
            let mut peaks = peaks.lock().await;
            peaks.insert(shadow_id.clone(), entry_price_usd);
        }

        let DbPool::PostgreSQL(pool) = db.pool();
        let ingress = req.ingress.as_str().to_string();
        let result = sqlx::query(
            r#"INSERT INTO shadow_positions (
                shadow_id, decision_id, run_id, wallet_address, token_address,
                strategy, main_admitted, main_rejection_code, main_rejection_reason,
                entry_amount_sol, entry_price_usd, entry_sol_price_usd,
                wqs, quality_score, liquidity_usd, consensus_wallet_count, ingress
            ) VALUES (
                $1, $2, $3, $4, $5,
                $6, $7, $8, $9,
                $10, $11, $12,
                $13, $14, $15, $16, $17
            )"#,
        )
        .bind(&shadow_id)
        .bind(&decision.decision_id)
        .bind(&config.run_id)
        .bind(&req.wallet_address)
        .bind(&req.token_address)
        .bind(&strategy_str)
        .bind(decision.admitted)
        .bind(decision.rejection_code.map(|s| s.to_string()))
        .bind(&decision.rejection_reason)
        .bind(config.position_size_sol)
        .bind(entry_price_usd)
        .bind(sol_price)
        .bind(wqs)
        .bind(quality_score)
        .bind(liquidity_usd)
        .bind(consensus_count)
        .bind(&ingress)
        .execute(&pool)
        .await;

        match result {
            Ok(_) => {
                tracing::debug!(
                    shadow_id = %shadow_id,
                    token = %req.token_address,
                    wallet = %req.wallet_address,
                    admitted = decision.admitted,
                    entry_price = %entry_price_usd,
                    rejection = ?decision.rejection_code,
                    "Shadow: position opened"
                );
            }
            Err(e) => {
                tracing::warn!(
                    shadow_id = %shadow_id,
                    error = %e,
                    "Shadow: failed to insert position"
                );
            }
        }
    }

    async fn on_wallet_sell(
        db: Arc<dyn Database>,
        price_cache: Arc<PriceCache>,
        config: &ShadowConfig,
        req: &SelectionRequest,
    ) {
        let DbPool::PostgreSQL(pool) = db.pool();

        let active_positions = sqlx::query_as::<_, (String, chrono::DateTime<Utc>, Decimal)>(
            r#"SELECT shadow_id, opened_at, entry_price_usd
               FROM shadow_positions
               WHERE wallet_address = $1
                 AND token_address = $2
                 AND fully_closed = FALSE
                 AND entry_price_usd IS NOT NULL"#,
        )
        .bind(&req.wallet_address)
        .bind(&req.token_address)
        .fetch_all(&pool)
        .await;

        let positions = match active_positions {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "Shadow: failed to query active positions for wallet_sell");
                return;
            }
        };

        if positions.is_empty() {
            return;
        }

        let exit_price = price_cache.get_price_usd(&req.token_address);
        let sol_price = price_cache.get_sol_price_usd();

        for (shadow_id, opened_at, entry_price) in positions {
            let already_exited: Result<Option<i32>, _> = sqlx::query_scalar(
                "SELECT 1 FROM shadow_exits WHERE shadow_id = $1 AND exit_strategy = 'wallet_sell'",
            )
            .bind(&shadow_id)
            .fetch_optional(&pool)
            .await;

            if let Ok(Some(_)) = already_exited {
                continue;
            }

            let elapsed_secs = (Utc::now() - opened_at).num_seconds();
            let (pnl_pct, pnl_sol) = Self::compute_pnl(entry_price, exit_price, config.position_size_sol);

            let _ = sqlx::query(
                r#"INSERT INTO shadow_exits (shadow_id, exit_strategy, exit_price_usd, exit_sol_price_usd, pnl_pct, pnl_sol, exit_reason, hold_duration_secs)
                   VALUES ($1, 'wallet_sell', $2, $3, $4, $5, 'wallet_sell', $6)
                   ON CONFLICT (shadow_id, exit_strategy) DO NOTHING"#,
            )
            .bind(&shadow_id)
            .bind(exit_price)
            .bind(sol_price)
            .bind(pnl_pct)
            .bind(pnl_sol)
            .bind(elapsed_secs.max(0))
            .execute(&pool)
            .await;

            tracing::debug!(
                shadow_id = %shadow_id,
                pnl_pct = %pnl_pct,
                "Shadow: wallet_sell exit triggered"
            );

            Self::check_fully_closed(&pool, &shadow_id).await;
        }
    }

    async fn insert_no_price_exits(
        db: &Arc<dyn Database>,
        shadow_id: &str,
        decision: &BuyDecision,
        req: &SelectionRequest,
        run_id: &str,
        position_size_sol: Decimal,
    ) {
        let DbPool::PostgreSQL(pool) = db.pool();
        let ingress = req.ingress.as_str().to_string();

        let _ = sqlx::query(
            r#"INSERT INTO shadow_positions (
                shadow_id, decision_id, run_id, wallet_address, token_address,
                strategy, main_admitted, main_rejection_code, main_rejection_reason,
                entry_amount_sol, ingress, fully_closed
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, TRUE)"#,
        )
        .bind(shadow_id)
        .bind(&decision.decision_id)
        .bind(run_id)
        .bind(&req.wallet_address)
        .bind(&req.token_address)
        .bind(decision.strategy.map(|s| s.to_string()))
        .bind(decision.admitted)
        .bind(decision.rejection_code.map(|s| s.to_string()))
        .bind(&decision.rejection_reason)
        .bind(position_size_sol)
        .bind(&ingress)
        .execute(&pool)
        .await;

        for strategy in &EXIT_STRATEGIES {
            let _ = sqlx::query(
                r#"INSERT INTO shadow_exits (shadow_id, exit_strategy, exit_reason, pnl_pct, pnl_sol, hold_duration_secs)
                   VALUES ($1, $2, 'no_price', $3, $4, $5)"#,
            )
            .bind(shadow_id)
            .bind(*strategy)
            .bind(Decimal::ZERO)
            .bind(Decimal::ZERO)
            .bind(0i64)
            .execute(&pool)
            .await;
        }
    }

    pub async fn check_exits(&self) {
        if !self.config.enabled {
            return;
        }

        let DbPool::PostgreSQL(pool) = self.db.pool();

        let active_positions = sqlx::query_as::<_, ShadowPositionRow>(
            r#"SELECT shadow_id, token_address, strategy, entry_price_usd, entry_amount_sol, opened_at
               FROM shadow_positions
               WHERE fully_closed = FALSE
                 AND entry_price_usd IS NOT NULL
               ORDER BY opened_at ASC"#,
        )
        .fetch_all(&pool)
        .await;

        let positions = match active_positions {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "Shadow: failed to query active positions");
                return;
            }
        };

        for pos in positions {
            self.check_position_exits(&pool, &pos).await;
        }

        self.cleanup_peaks(&pool).await;
    }

    async fn check_position_exits(
        &self,
        pool: &sqlx::PgPool,
        pos: &ShadowPositionRow,
    ) {
        // Ensure the token stays tracked so the background updater refreshes
        // its price (e.g. after a restart, tracked set is empty).
        self.price_cache.track_token(&pos.token_address);
        let current_price = match self.price_cache.get_price_usd(&pos.token_address) {
            Some(p) => p,
            None => {
                // No cached price this tick — try an eager fetch once, then
                // defer to the next 15s tick if it still fails (matches the
                // main system's position monitor behavior).
                self.price_cache.eager_fetch_token(&pos.token_address).await;
                match self.price_cache.get_price_usd(&pos.token_address) {
                    Some(p) => p,
                    None => return,
                }
            }
        };
        let sol_price = self.price_cache.get_sol_price_usd();
        let now = Utc::now();
        let elapsed_secs = (now - pos.opened_at).num_seconds();

        let mut peaks = self.peaks.lock().await;
        let peak = peaks
            .entry(pos.shadow_id.clone())
            .or_insert(pos.entry_price_usd);
        if current_price > *peak {
            *peak = current_price;
        }
        let peak_price = *peak;
        drop(peaks);

        if elapsed_secs > self.config.max_lifetime.as_secs() as i64 {
            for strategy in &EXIT_STRATEGIES {
                let _ = sqlx::query(
                    r#"INSERT INTO shadow_exits (shadow_id, exit_strategy, exit_price_usd, exit_sol_price_usd, pnl_pct, pnl_sol, exit_reason, hold_duration_secs)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                       ON CONFLICT (shadow_id, exit_strategy) DO NOTHING"#,
                )
                .bind(&pos.shadow_id)
                .bind(*strategy)
                .bind(current_price)
                .bind(sol_price)
                .bind(Self::pnl_pct(pos.entry_price_usd, current_price))
                .bind(Self::pnl_sol(pos.entry_price_usd, current_price, pos.entry_amount_sol))
                .bind("max_lifetime_expired")
                .bind(elapsed_secs.max(0))
                .execute(pool)
                .await;
            }

            Self::check_fully_closed(pool, &pos.shadow_id).await;
            return;
        }

        let mirror_exit = Self::check_mirror_main(
            &self.config.profit_config,
            pos.entry_price_usd,
            current_price,
            peak_price,
            elapsed_secs,
            &pos.strategy,
        );

        if let Some(reason) = mirror_exit {
            let _ = sqlx::query(
                r#"INSERT INTO shadow_exits (shadow_id, exit_strategy, exit_price_usd, exit_sol_price_usd, pnl_pct, pnl_sol, exit_reason, hold_duration_secs)
                   VALUES ($1, 'mirror_main', $2, $3, $4, $5, $6, $7)
                   ON CONFLICT (shadow_id, exit_strategy) DO NOTHING"#,
            )
            .bind(&pos.shadow_id)
            .bind(current_price)
            .bind(sol_price)
            .bind(Self::pnl_pct(pos.entry_price_usd, current_price))
            .bind(Self::pnl_sol(pos.entry_price_usd, current_price, pos.entry_amount_sol))
            .bind(&reason)
            .bind(elapsed_secs.max(0))
            .execute(pool)
            .await;

            tracing::debug!(
                shadow_id = %pos.shadow_id,
                reason = %reason,
                pnl_pct = %Self::pnl_pct(pos.entry_price_usd, current_price),
                "Shadow: mirror_main exit"
            );
        }

        for (i, strategy) in EXIT_STRATEGIES.iter().enumerate() {
            if *strategy == "mirror_main" || *strategy == "wallet_sell" {
                continue;
            }
            let hold_secs = FIXED_HOLDS_SECS.get(i - 1).copied().unwrap_or(86400);

            if elapsed_secs >= hold_secs {
                let _ = sqlx::query(
                    r#"INSERT INTO shadow_exits (shadow_id, exit_strategy, exit_price_usd, exit_sol_price_usd, pnl_pct, pnl_sol, exit_reason, hold_duration_secs)
                       VALUES ($1, $2, $3, $4, $5, $6, 'fixed_hold_expired', $7)
                       ON CONFLICT (shadow_id, exit_strategy) DO NOTHING"#,
                )
                .bind(&pos.shadow_id)
                .bind(*strategy)
                .bind(current_price)
                .bind(sol_price)
                .bind(Self::pnl_pct(pos.entry_price_usd, current_price))
                .bind(Self::pnl_sol(pos.entry_price_usd, current_price, pos.entry_amount_sol))
                .bind(elapsed_secs.max(0))
                .execute(pool)
                .await;

                tracing::debug!(
                    shadow_id = %pos.shadow_id,
                    strategy = %strategy,
                    hold_secs,
                    pnl_pct = %Self::pnl_pct(pos.entry_price_usd, current_price),
                    "Shadow: fixed_hold exit"
                );
            }
        }

        Self::check_fully_closed(pool, &pos.shadow_id).await;
    }

    fn check_mirror_main(
        config: &ProfitManagementConfig,
        entry_price: Decimal,
        current_price: Decimal,
        peak_price: Decimal,
        elapsed_secs: i64,
        strategy: &Option<String>,
    ) -> Option<String> {
        if entry_price.is_zero() {
            return None;
        }

        let pnl_pct = (current_price - entry_price) / entry_price * Decimal::from(100);
        let profit_pct = pnl_pct.max(Decimal::ZERO);
        let loss_pct = pnl_pct.min(Decimal::ZERO);

        let elapsed_secs_u64 = elapsed_secs as u64;

        // Order mirrors the real position monitor (stop_loss.rs + profit_targets.rs):
        // 1. Hard stop: absolute floor at -25%.
        if loss_pct <= dec!(-25) {
            return Some("stop_loss".to_string());
        }

        // 2. Recovery gate: the DOMINANT loser exit in the real monitor — after
        //    the wick window, any position still below the threshold is cut
        //    immediately (data: winners recover above -1% within 48s, losers
        //    stay below -2.5%). The mirror previously held to -8%, massively
        //    overstating losses for positions the real system exits at -2%.
        if elapsed_secs_u64 > config.recovery_gate_secs
            && loss_pct < config.recovery_gate_threshold
        {
            return Some("recovery_gate".to_string());
        }

        // 3. Adaptive stop approximation: the real monitor clamps the dynamic
        //    stop to max_stop_loss_distance. Mirror uses the flat value.
        if loss_pct <= config.max_stop_loss_distance {
            return Some("stop_loss".to_string());
        }

        // 4. Wick window: during the first wick_protection_secs, cap losses at
        //    wick_protection_max_loss_percent (fast-dump protection).
        if elapsed_secs_u64 <= config.wick_protection_secs
            && loss_pct <= config.wick_protection_max_loss_percent
        {
            return Some("stop_loss".to_string());
        }

        // 5. Trailing stop (unchanged).
        if profit_pct >= config.trailing_stop_activation {
            let trailing_stop_price = peak_price
                * (Decimal::ONE - config.trailing_stop_distance / Decimal::from(100));
            if current_price <= trailing_stop_price {
                return Some("trailing_stop".to_string());
            }
        }

        // 6. Profit targets (currently empty — trailing-only regime).
        for target in &config.targets {
            if profit_pct >= *target {
                return Some(format!("profit_target_{}", target));
            }
        }

        // 7. Tiered time exit — matches profit_targets.rs exactly:
        //    profit > 25%: SPEAR 24h / SHIELD 48h
        //    profit > 10%: SPEAR 12h / SHIELD time_exit_hours
        //    else:         SPEAR losing_spear / SHIELD losing_shield
        //    The mirror previously used a flat time_exit_hours for winners and
        //    a loss-threshold-gated losing exit — both diverge from the real
        //    monitor, which exits at the tier limit regardless of PnL level.
        let is_spear = strategy.as_deref() == Some("SPEAR");
        let exit_limit_hours = if profit_pct > dec!(25) {
            if is_spear {
                24
            } else {
                48
            }
        } else if profit_pct > dec!(10) {
            if is_spear {
                12
            } else {
                config.time_exit_hours
            }
        } else if is_spear {
            config.losing_time_exit_hours_spear
        } else {
            config.losing_time_exit_hours_shield
        };

        if elapsed_secs >= exit_limit_hours as i64 * 3600 {
            return Some("time_exit".to_string());
        }

        None
    }

    fn compute_pnl(
        entry_price: Decimal,
        exit_price: Option<Decimal>,
        amount_sol: Decimal,
    ) -> (Decimal, Decimal) {
        match exit_price {
            Some(ep) if !entry_price.is_zero() => {
                let pct = (ep - entry_price) / entry_price * Decimal::from(100);
                let sol = amount_sol * pct / Decimal::from(100);
                (pct, sol)
            }
            _ => (Decimal::ZERO, Decimal::ZERO),
        }
    }

    fn pnl_pct(entry: Decimal, exit: Decimal) -> Decimal {
        if entry.is_zero() {
            return Decimal::ZERO;
        }
        (exit - entry) / entry * Decimal::from(100)
    }

    fn pnl_sol(entry: Decimal, exit: Decimal, amount: Decimal) -> Decimal {
        if entry.is_zero() {
            return Decimal::ZERO;
        }
        amount * (exit - entry) / entry
    }

    async fn check_fully_closed(pool: &sqlx::PgPool, shadow_id: &str) {
        let exit_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM shadow_exits WHERE shadow_id = $1",
        )
        .bind(shadow_id)
        .fetch_one(pool)
        .await
        .unwrap_or(0);

        if exit_count >= EXIT_STRATEGIES.len() as i64 {
            let _ = sqlx::query(
                "UPDATE shadow_positions SET fully_closed = TRUE, closed_at = NOW() WHERE shadow_id = $1 AND fully_closed = FALSE",
            )
            .bind(shadow_id)
            .execute(pool)
            .await;
        }
    }

    /// Remove peak-tracking entries for positions that are now fully closed.
    /// Called periodically from check_exits to prevent unbounded memory growth.
    async fn cleanup_peaks(&self, pool: &sqlx::PgPool) {
        let closed_ids: Result<Vec<String>, _> = sqlx::query_scalar(
            "SELECT shadow_id FROM shadow_positions WHERE fully_closed = TRUE",
        )
        .fetch_all(pool)
        .await;

        if let Ok(ids) = closed_ids {
            if ids.is_empty() {
                return;
            }
            let mut peaks = self.peaks.lock().await;
            for id in &ids {
                peaks.remove(id);
            }
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct ShadowPositionRow {
    shadow_id: String,
    token_address: String,
    strategy: Option<String>,
    entry_price_usd: Decimal,
    entry_amount_sol: Decimal,
    opened_at: chrono::DateTime<Utc>,
}
