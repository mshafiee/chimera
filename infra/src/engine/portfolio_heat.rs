//! Portfolio Heat Management
//!
//! Tracks total portfolio risk exposure and blocks new positions
//! when heat limit (20% of capital) is reached.

use crate::db_abstraction::Database;
use crate::state::registry::StateRegistry;
use parking_lot::RwLock;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use std::sync::Arc;

/// Portfolio heat manager
pub struct PortfolioHeat {
    db: Arc<dyn Database>,
    /// Maximum portfolio heat as percentage of capital (default: 20%)
    max_heat_percent: Decimal,
    /// Total capital in SOL — wrapped in Arc<RwLock> so the background wallet-balance
    /// refresh task can update it without rebuilding the struct.
    total_capital_sol: Arc<RwLock<Decimal>>,
    /// Optional state registry for fast in-memory portfolio heat calculation
    registry: Option<Arc<StateRegistry>>,
}

/// Portfolio heat result
#[derive(Debug, Clone)]
pub struct HeatResult {
    /// Current heat percentage (0.0-100.0, using Decimal for precision)
    pub current_heat_percent: Decimal,
    /// Total exposure in SOL (using Decimal for precision)
    pub total_exposure_sol: Decimal,
    /// Available heat capacity in SOL (using Decimal for precision)
    pub available_heat_sol: Decimal,
    /// Whether new positions can be opened
    pub can_open_position: bool,
}

impl PortfolioHeat {
    pub fn new(db: Arc<dyn Database>, total_capital_sol: Decimal) -> Self {
        Self {
            db,
            max_heat_percent: dec!(20),
            total_capital_sol: Arc::new(RwLock::new(total_capital_sol)),
            registry: None,
        }
    }

    /// Create with custom max heat percentage
    pub fn with_max_heat(
        db: Arc<dyn Database>,
        total_capital_sol: Decimal,
        max_heat_percent: Decimal,
    ) -> Self {
        let max_heat = max_heat_percent.max(Decimal::ZERO).min(Decimal::from(100));
        Self {
            db,
            max_heat_percent: max_heat,
            total_capital_sol: Arc::new(RwLock::new(total_capital_sol)),
            registry: None,
        }
    }

    /// Set the state registry for fast in-memory portfolio heat calculation
    pub fn with_registry(mut self, registry: Arc<StateRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Update the capital figure from a live wallet balance query.
    /// Called by the background refresh task in main.rs every 60 seconds.
    pub fn update_capital(&self, new_capital: Decimal) {
        *self.total_capital_sol.write() = new_capital;
    }

    /// Returns true when exposure exceeds 150% of the configured heat limit.
    ///
    /// Used by the force-liquidation background task to detect external capital drains
    /// (e.g. user withdraws from wallet) that push existing positions above the heat cap.
    /// The 1.5× buffer avoids false triggers on normal market fluctuations.
    pub async fn is_critically_overexposed(&self) -> Result<bool, String> {
        let heat = self.calculate_heat().await?;
        let capital = *self.total_capital_sol.read();
        let max_heat_sol = capital * (self.max_heat_percent / Decimal::from(100));
        Ok(heat.total_exposure_sol > max_heat_sol * dec!(1.5))
    }

    /// Calculate current portfolio heat
    ///
    /// # Returns
    /// HeatResult with current heat status
    pub async fn calculate_heat(&self) -> Result<HeatResult, String> {
        // Try fast path with registry first (sub-microsecond latency)
        if let Some(ref registry) = self.registry {
            tracing::trace!("Using registry fast path for portfolio heat calculation");
            let heat_state = registry.calculate_portfolio_heat_fast();
            let total_exposure = heat_state.total_exposure_sol;

            let capital = *self.total_capital_sol.read();

            // Calculate heat percentage using Decimal for precision.
            let current_heat_percent = if !capital.is_zero() {
                (total_exposure / capital) * Decimal::from(100)
            } else {
                Decimal::from(100)
            };

            // Calculate available heat
            let max_heat_sol = capital * (self.max_heat_percent / Decimal::from(100));
            let available_heat_sol = max_heat_sol - total_exposure;

            // Check if can open new position
            let can_open_position = current_heat_percent < self.max_heat_percent;

            return Ok(HeatResult {
                current_heat_percent,
                total_exposure_sol: total_exposure,
                available_heat_sol: available_heat_sol.max(Decimal::ZERO),
                can_open_position,
            });
        }

        // Fallback to database queries (legacy path, 50-200ms latency)
        // One-time warning: this fires whenever the registry is absent, so a
        // warn on every call would spam logs (calculate_heat runs per position
        // attempt and from the background liquidation task).
        static REGISTRY_FALLBACK_WARNED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        if REGISTRY_FALLBACK_WARNED.set(()).is_ok() {
            tracing::warn!("Registry not available - using database fallback for portfolio heat (slower)");
        }
        // Include EXITING positions — they still hold capital until exit confirms.
        // Use entry_amount_sol only: heat measures capital at risk (deployed capital),
        // not mark-to-market value. Including unrealized PnL inflates heat on winners
        // (blocking new trades) and deflates it on losers (allowing over-exposure).
        // Exclude EXITING positions that have been stuck for >30 minutes (1800 seconds)
        // so that permanently failed recovery attempts don't lock capital forever.
        // 1800s chosen because RPC confirmation can take 15-20 min under congestion;
        // 900s was too short — stuck EXITING positions were dropped from heat, allowing
        // new trades to open before the exit confirmed, creating up to 2× intended exposure.
        let now = chrono::Utc::now();
        let heat_cutoff = now - chrono::Duration::seconds(1800);

        // Get active positions for heat calculation
        let positions = self
            .db
            .get_active_positions()
            .await
            .map_err(|e| format!("Failed to query portfolio heat: {}", e))?;

        let mut total_exposure = rust_decimal::Decimal::ZERO;
        let mut stale_exiting_count: i64 = 0;
        let mut stale_exposure_sol = rust_decimal::Decimal::ZERO;

        for pos in &positions {
            if pos.state == "ACTIVE" {
                total_exposure += pos.entry_amount_sol;
            } else if pos.state == "EXITING" {
                if pos.last_updated >= heat_cutoff {
                    total_exposure += pos.entry_amount_sol;
                }
                // Warn when EXITING positions have been stuck longer than the recovery
                // escalation threshold (5 min).
                if pos.last_updated < now - chrono::Duration::seconds(300) {
                    stale_exiting_count += 1;
                    stale_exposure_sol += pos.entry_amount_sol;
                }
            }
        }

        if stale_exiting_count > 0 {
            tracing::warn!(
                stale_exiting_count,
                stale_exposure_sol = %stale_exposure_sol,
                "STALE_EXITING: positions stuck >5 min are locking portfolio heat; \
                 check recovery.rs background task and RPC connectivity"
            );
        }

        // Get pending/queued/executing trades for heat calculation
        // Stale BUY trades (>5 minutes, matching get_strategy_heat's cutoff)
        // are excluded so both gates agree on what is "pending heat".
        for status in &["PENDING", "QUEUED", "EXECUTING", "RETRY"] {
            let trades = self
                .db
                .get_trades_by_status(status, i32::MAX)
                .await
                .map_err(|e| format!("Failed to query portfolio heat: {}", e))?;
            for trade in &trades {
                if trade.side == "BUY" {
                    let trade_age = chrono::Utc::now() - trade.created_at;
                    if trade_age.num_seconds() > 300 {
                        continue;
                    }
                    total_exposure += trade.amount_sol;
                }
            }
        }
        let capital = *self.total_capital_sol.read();

        // Calculate heat percentage using Decimal for precision.
        // capital==0 only when both live fetch and cache are zero (genuinely empty wallet
        // or first boot before any successful fetch), so blocking here is correct.
        let current_heat_percent = if !capital.is_zero() {
            (total_exposure / capital) * Decimal::from(100)
        } else {
            Decimal::from(100)
        };

        // Calculate available heat
        let max_heat_sol = capital * (self.max_heat_percent / Decimal::from(100));
        let available_heat_sol = max_heat_sol - total_exposure;

        // Check if can open new position
        let can_open_position = current_heat_percent < self.max_heat_percent;

        Ok(HeatResult {
            current_heat_percent,
            total_exposure_sol: total_exposure,
            available_heat_sol: available_heat_sol.max(Decimal::ZERO),
            can_open_position,
        })
    }

    /// Check if a new position of given size can be opened
    ///
    /// # Arguments
    /// * `position_size_sol` - Size of new position in SOL (using Decimal for precision)
    ///
    /// # Returns
    /// true if position can be opened, false otherwise
    pub async fn can_open_position(&self, position_size_sol: Decimal) -> Result<bool, String> {
        // Reject non-positive sizes outright so the capacity accounting stays
        // sound (a negative size would reduce reported exposure).
        if position_size_sol <= Decimal::ZERO {
            return Ok(false);
        }

        let heat = self.calculate_heat().await?;

        if !heat.can_open_position {
            tracing::warn!(
                capital = %(*self.total_capital_sol.read()),
                max_heat_percent = %self.max_heat_percent,
                current_heat_percent = %heat.current_heat_percent,
                total_exposure = %heat.total_exposure_sol,
                available_heat = %heat.available_heat_sol,
                "[PORTFOLIO_HEAT] General heat check: BLOCKED - current heat {}% > max heat {}%",
                heat.current_heat_percent,
                self.max_heat_percent
            );
            return Ok(false);
        }

        // Check if new position would exceed heat limit using Decimal for precision.
        let new_exposure = heat.total_exposure_sol + position_size_sol;
        let capital = *self.total_capital_sol.read();
        let new_heat_percent = if !capital.is_zero() {
            (new_exposure / capital) * Decimal::from(100)
        } else {
            Decimal::from(100)
        };

        let result = new_heat_percent <= self.max_heat_percent;
        
        tracing::info!(
            capital = %capital,
            max_heat_percent = %self.max_heat_percent,
            current_heat_percent = %heat.current_heat_percent,
            total_exposure = %heat.total_exposure_sol,
            new_exposure = %new_exposure,
            new_heat_percent = %new_heat_percent,
            position_size = %position_size_sol,
            can_open = result,
            "[PORTFOLIO_HEAT] Position check: {} ({}% <= {}%)",
            if result { "PASS" } else { "BLOCK" },
            new_heat_percent,
            self.max_heat_percent
        );

        Ok(result)
    }

    /// Get heat breakdown by strategy
    ///
    /// # Returns
    /// Tuple of (shield_heat_sol, spear_heat_sol) using Decimal for precision
    pub async fn get_strategy_heat(&self) -> Result<(Decimal, Decimal), String> {
        // [T-M1] Use 1800 s to match calculate_heat. The previous 900 s threshold caused
        // get_strategy_heat to drop EXITING positions from the strategy allocation check
        // before calculate_heat dropped them from the total heat, creating a window where
        // the strategy limit appeared to have headroom while total heat was still at cap.
        let now = chrono::Utc::now();
        let heat_cutoff = now - chrono::Duration::seconds(1800);

        let positions = self
            .db
            .get_active_positions()
            .await
            .map_err(|e| format!("Failed to query strategy heat: {}", e))?;

        let mut shield_heat = Decimal::ZERO;
        let mut spear_heat = Decimal::ZERO;

        for pos in &positions {
            let include = pos.state == "ACTIVE"
                || (pos.state == "EXITING" && pos.last_updated >= heat_cutoff);
            if include {
                match pos.strategy.as_str() {
                    "SHIELD" => shield_heat += pos.entry_amount_sol,
                    "SPEAR" => spear_heat += pos.entry_amount_sol,
                    _ => {}
                }
            }
        }
        
        for status in &["PENDING", "QUEUED", "EXECUTING", "RETRY"] {
            let trades = self
                .db
                .get_trades_by_status(status, i32::MAX)
                .await
                .map_err(|e| format!("Failed to query strategy heat: {}", e))?;
            for trade in &trades {
                if trade.side == "BUY" {
                    // FIX: Exclude trades that have been pending for >5 minutes
                    // to prevent stale queue entries from blocking new trades
                    let trade_age = chrono::Utc::now() - trade.created_at;
                    let is_stale = trade_age.num_seconds() > 300; // 5 minutes
                    
                    if !is_stale {
                        match trade.strategy.as_str() {
                            "SHIELD" => shield_heat += trade.amount_sol,
                            "SPEAR" => spear_heat += trade.amount_sol,
                            _ => {}
                        }
                    } else {
                        tracing::warn!(
                            trade_uuid = %trade.trade_uuid,
                            strategy = %trade.strategy,
                            amount = %trade.amount_sol,
                            age_seconds = trade_age.num_seconds(),
                            "[PORTFOLIO_HEAT] Excluding stale pending trade from heat calculation"
                        );
                    }
                }
            }
        }

        Ok((shield_heat, spear_heat))
    }

    /// `get_strategy_heat` excluding one trade from BOTH heat sources —
    /// the positions table (write-queue flush) and the in-flight status
    /// trades (PENDING/QUEUED/EXECUTING/RETRY). Used by the execution-time
    /// re-check so the trade being processed is counted exactly once (in
    /// `position_size_sol`), never twice (2026-08-18).
    pub async fn get_strategy_heat_excluding(
        &self,
        exclude_trade_uuid: &str,
    ) -> Result<(Decimal, Decimal), String> {
        let now = chrono::Utc::now();
        let heat_cutoff = now - chrono::Duration::seconds(1800);

        let positions = self
            .db
            .get_active_positions_excluding(exclude_trade_uuid)
            .await
            .map_err(|e| format!("Failed to query strategy heat: {}", e))?;

        let mut shield_heat = Decimal::ZERO;
        let mut spear_heat = Decimal::ZERO;

        for pos in &positions {
            let include = pos.state == "ACTIVE"
                || (pos.state == "EXITING" && pos.last_updated >= heat_cutoff);
            if include {
                match pos.strategy.as_str() {
                    "SHIELD" => shield_heat += pos.entry_amount_sol,
                    "SPEAR" => spear_heat += pos.entry_amount_sol,
                    _ => {}
                }
            }
        }

        for status in &["PENDING", "QUEUED", "EXECUTING", "RETRY"] {
            let trades = self
                .db
                .get_trades_by_status(status, i32::MAX)
                .await
                .map_err(|e| format!("Failed to query strategy heat: {}", e))?;
            for trade in &trades {
                if trade.trade_uuid == exclude_trade_uuid {
                    continue;
                }
                if trade.side == "BUY" {
                    let trade_age = chrono::Utc::now() - trade.created_at;
                    let is_stale = trade_age.num_seconds() > 300;
                    if !is_stale {
                        match trade.strategy.as_str() {
                            "SHIELD" => shield_heat += trade.amount_sol,
                            "SPEAR" => spear_heat += trade.amount_sol,
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok((shield_heat, spear_heat))
    }

    pub async fn can_open_strategy_position(
        &self,
        strategy: chimera_core::models::Strategy,
        position_size_sol: Decimal,
        shield_percent: u32,
        spear_percent: u32,
    ) -> Result<bool, String> {
        self.can_open_strategy_position_excluding(
            strategy,
            position_size_sol,
            shield_percent,
            spear_percent,
            None,
        )
        .await
    }

    /// Strategy allocation check that can exclude the trade being processed
    /// (2026-08-18). The pipeline's execution-time re-check runs AFTER the
    /// trade's own position row is flushed (write queue) — without the
    /// exclusion, `current_heat + own_size` double-counts it and every
    /// entry larger than half the strategy allocation self-blocks.
    pub async fn can_open_strategy_position_excluding(
        &self,
        strategy: chimera_core::models::Strategy,
        position_size_sol: Decimal,
        shield_percent: u32,
        spear_percent: u32,
        exclude_trade_uuid: Option<&str>,
    ) -> Result<bool, String> {
        // Non-positive sizes are rejected before anything else so capacity
        // accounting stays sound.
        if position_size_sol <= Decimal::ZERO {
            return Ok(false);
        }

        // Self-excluded mode (2026-08-18): both gates derive from the
        // excluded heats. The general can_open_position / calculate_heat
        // path reads the registry fast path, which still contains the
        // trade being processed — using it here would double-count the
        // same exposure the exclusion exists to remove.
        let (shield_heat, spear_heat) = match exclude_trade_uuid {
            Some(uuid) => self.get_strategy_heat_excluding(uuid).await?,
            None => {
                if !self.can_open_position(position_size_sol).await? {
                    return Ok(false);
                }
                self.get_strategy_heat().await?
            }
        };

        // General heat gate in excluded mode: total exposure (ex-self) plus
        // the new size against the 30% critical-heat ceiling used by the
        // pipeline re-check.
        if let Some(_uuid) = exclude_trade_uuid {
            let total_exposure = shield_heat + spear_heat;
            let capital = *self.total_capital_sol.read();
            let max_heat_sol = capital * (self.max_heat_percent / Decimal::from(100));
            if total_exposure + position_size_sol > max_heat_sol {
                tracing::warn!(
                    trade_uuid = %_uuid,
                    strategy = ?strategy,
                    total_exposure_ex_self = %total_exposure,
                    position_size = %position_size_sol,
                    max_heat_sol = %max_heat_sol,
                    "[PORTFOLIO_HEAT] General heat check (self-excluded): BLOCKED"
                );
                return Ok(false);
            }
        }

        let allocation_pct = match strategy {
            chimera_core::models::Strategy::Shield => Decimal::from(shield_percent),
            chimera_core::models::Strategy::Spear => Decimal::from(spear_percent),
            _ => return Ok(true),
        };
        if allocation_pct.is_zero() {
            // 0% allocation means this strategy is disabled — block all positions
            return Ok(false);
        }
        let capital = *self.total_capital_sol.read();
        let max_heat_sol = capital * (self.max_heat_percent / Decimal::from(100));
        let allocated_sol = max_heat_sol * (allocation_pct / Decimal::from(100));
        let current_heat = match strategy {
            chimera_core::models::Strategy::Shield => shield_heat,
            chimera_core::models::Strategy::Spear => spear_heat,
            _ => Decimal::ZERO,
        };
        
        // Diagnostic logging for allocation checks
        let result = current_heat + position_size_sol <= allocated_sol;
        tracing::info!(
            strategy = ?strategy,
            capital = %capital,
            max_heat_percent = %self.max_heat_percent,
            max_heat_sol = %max_heat_sol,
            allocation_pct = %allocation_pct,
            allocated_sol = %allocated_sol,
            shield_heat = %shield_heat,
            spear_heat = %spear_heat,
            current_heat = %current_heat,
            position_size = %position_size_sol,
            can_open = result,
            "[PORTFOLIO_HEAT] Strategy allocation check: {} ({} + {} <= {})",
            if result { "PASS" } else { "BLOCK" },
            current_heat,
            position_size_sol,
            allocated_sol
        );
        
        Ok(result)
    }

    /// Returns the 150% heat threshold limit directly in SOL
    pub fn get_critical_threshold_sol(&self) -> Decimal {
        let capital = *self.total_capital_sol.read();
        let max_heat_sol = capital * (self.max_heat_percent / Decimal::from(100));
        max_heat_sol * dec!(1.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_abstraction::{Position, Trade};
    use crate::engine::kelly_sizer::tests::MockDatabase;
    use crate::state::registry::{PositionState, StateRegistry};
    use parking_lot::RwLock;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::SystemTime;

    fn position(
        uuid: &str,
        strategy: &str,
        state: &str,
        entry_amount_sol: Decimal,
        last_updated: chrono::DateTime<chrono::Utc>,
    ) -> Position {
        Position {
            id: 0,
            trade_uuid: uuid.to_string(),
            wallet_address: "wallet".to_string(),
            token_address: format!("token-{uuid}"),
            token_symbol: None,
            strategy: strategy.to_string(),
            entry_amount_sol,
            entry_price: Decimal::ONE,
            entry_tx_signature: "sig".to_string(),
            current_price: None,
            unrealized_pnl_sol: None,
            unrealized_pnl_percent: None,
            state: state.to_string(),
            exit_price: None,
            exit_tx_signature: None,
            realized_pnl_sol: None,
            realized_pnl_usd: None,
            entry_sol_price_usd: None,
            opened_at: last_updated,
            last_updated,
            closed_at: None,
            token_amount: None,
        }
    }

    fn buy_trade(
        uuid: &str,
        strategy: &str,
        amount_sol: Decimal,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Trade {
        Trade {
            id: 0,
            trade_uuid: uuid.to_string(),
            wallet_address: "wallet".to_string(),
            token_address: format!("token-{uuid}"),
            token_symbol: None,
            strategy: strategy.to_string(),
            side: "BUY".to_string(),
            amount_sol,
            price_at_signal: None,
            tx_signature: None,
            status: "PENDING".to_string(),
            retry_count: 0,
            error_message: None,
            pnl_sol: None,
            pnl_usd: None,
            jito_tip_sol: Decimal::ZERO,
            dex_fee_sol: Decimal::ZERO,
            slippage_cost_sol: Decimal::ZERO,
            total_cost_sol: Decimal::ZERO,
            net_pnl_sol: None,
            pnl_data_valid: true,
            created_at,
            updated_at: created_at,
        }
    }

    fn heat_db(
        positions: Vec<Position>,
        trades: HashMap<String, Vec<Trade>>,
    ) -> Arc<dyn crate::db_abstraction::Database> {
        Arc::new(MockDatabase {
            active_positions: RwLock::new(positions),
            trades_by_status: RwLock::new(trades),
            ..Default::default()
        })
    }

    fn now_ago_secs(secs: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now() - chrono::Duration::seconds(secs)
    }

    #[tokio::test]
    async fn test_heat_db_fallback_basic() {
        // Capital 100, one ACTIVE 10 SOL position -> 10% heat.
        let db = heat_db(
            vec![position(
                "p1",
                "SHIELD",
                "ACTIVE",
                dec!(10),
                now_ago_secs(60),
            )],
            HashMap::new(),
        );
        let heat = PortfolioHeat::new(db, dec!(100));
        let result = heat.calculate_heat().await.unwrap();

        assert_eq!(result.current_heat_percent, dec!(10));
        assert_eq!(result.total_exposure_sol, dec!(10));
        assert_eq!(result.available_heat_sol, dec!(10)); // 20 - 10
        assert!(result.can_open_position);
    }

    #[tokio::test]
    async fn test_heat_exiting_fresh_included_stale_excluded() {
        let db = heat_db(
            vec![
                position("fresh", "SHIELD", "EXITING", dec!(5), now_ago_secs(60)),
                position("stale", "SPEAR", "EXITING", dec!(7), now_ago_secs(3600)),
            ],
            HashMap::new(),
        );
        let heat = PortfolioHeat::new(db, dec!(100));
        let result = heat.calculate_heat().await.unwrap();

        // Stale EXITING (>30min) dropped from exposure; fresh (5) counted.
        assert_eq!(result.total_exposure_sol, dec!(5));
        assert_eq!(result.current_heat_percent, dec!(5));
    }

    #[tokio::test]
    async fn test_heat_stale_exiting_warns_and_counts() {
        // EXITING position 600s old: within 1800s heat cutoff (counts) but
        // older than the 300s stale-exiting warning threshold (warns).
        let db = heat_db(
            vec![position(
                "stuck",
                "SHIELD",
                "EXITING",
                dec!(8),
                now_ago_secs(600),
            )],
            HashMap::new(),
        );
        let heat = PortfolioHeat::new(db, dec!(100));
        let result = heat.calculate_heat().await.unwrap();
        assert_eq!(result.total_exposure_sol, dec!(8));
    }

    #[tokio::test]
    async fn test_heat_pending_trades_counted_and_stale_excluded() {
        let mut trades = HashMap::new();
        trades.insert(
            "PENDING".to_string(),
            vec![
                buy_trade("t1", "SHIELD", dec!(3), now_ago_secs(10)),
                buy_trade("t2", "SHIELD", dec!(4), now_ago_secs(600)), // stale
            ],
        );
        let db = heat_db(Vec::new(), trades);
        let heat = PortfolioHeat::new(db, dec!(100));
        let result = heat.calculate_heat().await.unwrap();

        assert_eq!(result.total_exposure_sol, dec!(3));
    }

    #[tokio::test]
    async fn test_heat_zero_capital_blocks() {
        let db = heat_db(Vec::new(), HashMap::new());
        let heat = PortfolioHeat::new(db, dec!(0));
        let result = heat.calculate_heat().await.unwrap();
        assert_eq!(result.current_heat_percent, dec!(100));
        assert!(!result.can_open_position);
    }

    #[tokio::test]
    async fn test_heat_db_error() {
        let db: Arc<dyn crate::db_abstraction::Database> = Arc::new(MockDatabase {
            fail_active_positions: RwLock::new(true),
            ..Default::default()
        });
        let heat = PortfolioHeat::new(db, dec!(100));
        let err = heat.calculate_heat().await.unwrap_err();
        assert!(err.contains("Failed to query portfolio heat"));
    }

    #[tokio::test]
    async fn test_heat_registry_fast_path() {
        let registry = Arc::new(StateRegistry::new());
        registry
            .insert_position(PositionState {
                trade_uuid: "pos-1".to_string(),
                wallet_address: "wallet".to_string(),
                token_address: "token-1".to_string(),
                token_symbol: None,
                state: "ACTIVE".to_string(),
                strategy: "SHIELD".to_string(),
                entry_amount_sol: dec!(30),
                current_price: None,
                unrealized_pnl_sol: None,
                updated_at: SystemTime::now(),
            })
            .unwrap();

        let db = heat_db(Vec::new(), HashMap::new());
        let heat = PortfolioHeat::new(db, dec!(100)).with_registry(registry);
        let result = heat.calculate_heat().await.unwrap();

        assert_eq!(result.total_exposure_sol, dec!(30));
        assert_eq!(result.current_heat_percent, dec!(30));
        assert!(!result.can_open_position);
        assert_eq!(result.available_heat_sol, dec!(0)); // 20 - 30 -> max(0)
    }

    #[tokio::test]
    async fn test_registry_fast_path_zero_capital() {
        let registry = Arc::new(StateRegistry::new());
        let db = heat_db(Vec::new(), HashMap::new());
        let heat = PortfolioHeat::new(db, dec!(0)).with_registry(registry);
        let result = heat.calculate_heat().await.unwrap();
        assert_eq!(result.current_heat_percent, dec!(100));
    }

    #[tokio::test]
    async fn test_can_open_position_gates() {
        // Heat already at 30% (blocked) and at 10% (pass).
        let db = heat_db(
            vec![position(
                "p1",
                "SHIELD",
                "ACTIVE",
                dec!(30),
                now_ago_secs(60),
            )],
            HashMap::new(),
        );
        let heat = PortfolioHeat::new(db, dec!(100));
        assert!(!heat.can_open_position(dec!(1)).await.unwrap());

        // Negative size rejected outright.
        let db = heat_db(Vec::new(), HashMap::new());
        let heat = PortfolioHeat::new(db, dec!(100));
        assert!(!heat.can_open_position(dec!(-5)).await.unwrap());
        assert!(!heat.can_open_position(Decimal::ZERO).await.unwrap());

        // 15 SOL position on 100 capital -> 15% <= 20% pass.
        let db = heat_db(Vec::new(), HashMap::new());
        let heat = PortfolioHeat::new(db, dec!(100));
        assert!(heat.can_open_position(dec!(15)).await.unwrap());

        // 25 SOL would exceed -> block.
        let db = heat_db(Vec::new(), HashMap::new());
        let heat = PortfolioHeat::new(db, dec!(100));
        assert!(!heat.can_open_position(dec!(25)).await.unwrap());
    }

    #[tokio::test]
    async fn test_is_critically_overexposed() {
        let db = heat_db(
            vec![position(
                "p1",
                "SHIELD",
                "ACTIVE",
                dec!(40),
                now_ago_secs(60),
            )],
            HashMap::new(),
        );
        let heat = PortfolioHeat::new(db, dec!(100));
        // 40 > 20 * 1.5 = 30 -> overexposed
        assert!(heat.is_critically_overexposed().await.unwrap());

        let db = heat_db(
            vec![position(
                "p1",
                "SHIELD",
                "ACTIVE",
                dec!(20),
                now_ago_secs(60),
            )],
            HashMap::new(),
        );
        let heat = PortfolioHeat::new(db, dec!(100));
        // 20 == 30? No -> not overexposed
        assert!(!heat.is_critically_overexposed().await.unwrap());
    }

    #[tokio::test]
    async fn test_get_strategy_heat() {
        let mut trades = HashMap::new();
        trades.insert(
            "PENDING".to_string(),
            vec![
                buy_trade("t1", "SHIELD", dec!(2), now_ago_secs(10)),
                buy_trade("t2", "SPEAR", dec!(3), now_ago_secs(10)),
                buy_trade("t3", "SHIELD", dec!(4), now_ago_secs(600)), // stale -> warn
            ],
        );
        let db = heat_db(
            vec![
                position("p1", "SHIELD", "ACTIVE", dec!(10), now_ago_secs(60)),
                position("p2", "SPEAR", "ACTIVE", dec!(5), now_ago_secs(60)),
                position("p3", "SHIELD", "EXITING", dec!(1), now_ago_secs(3600)), // stale
                position("p4", "OTHER", "ACTIVE", dec!(2), now_ago_secs(60)),     // ignored
            ],
            trades,
        );
        let heat = PortfolioHeat::new(db, dec!(100));
        let (shield, spear) = heat.get_strategy_heat().await.unwrap();
        assert_eq!(shield, dec!(12)); // 10 + 2
        assert_eq!(spear, dec!(8)); // 5 + 3
    }

    #[tokio::test]
    async fn test_can_open_strategy_position() {
        // 100 capital, 20% max heat -> 20 SOL max. SHIELD allocation 50% -> 10 SOL.
        let db = heat_db(Vec::new(), HashMap::new());
        let heat = PortfolioHeat::new(db, dec!(100));
        assert!(heat
            .can_open_strategy_position(chimera_core::models::Strategy::Shield, dec!(5), 50, 50)
            .await
            .unwrap());
        assert!(!heat
            .can_open_strategy_position(chimera_core::models::Strategy::Shield, dec!(11), 50, 50)
            .await
            .unwrap());

        // 0% allocation -> disabled.
        let db = heat_db(Vec::new(), HashMap::new());
        let heat = PortfolioHeat::new(db, dec!(100));
        assert!(!heat
            .can_open_strategy_position(chimera_core::models::Strategy::Shield, dec!(1), 0, 50)
            .await
            .unwrap());

        // Unknown strategy (Exit) -> Ok(true) after general gate.
        let db = heat_db(Vec::new(), HashMap::new());
        let heat = PortfolioHeat::new(db, dec!(100));
        assert!(heat
            .can_open_strategy_position(chimera_core::models::Strategy::Exit, dec!(1), 50, 50)
            .await
            .unwrap());

        // General gate fails first (heat at 30%) -> false.
        let db = heat_db(
            vec![position(
                "p1",
                "SHIELD",
                "ACTIVE",
                dec!(30),
                now_ago_secs(60),
            )],
            HashMap::new(),
        );
        let heat = PortfolioHeat::new(db, dec!(100));
        assert!(!heat
            .can_open_strategy_position(chimera_core::models::Strategy::Shield, dec!(1), 50, 50)
            .await
            .unwrap());
    }

    /// Self-exclusion regression (2026-08-18): the execution-time re-check
    /// runs after the queued trade's own rows are flushed — position row AND
    /// in-flight status trade. Without exclusion, `current + own` charges
    /// the trade twice and any entry larger than half the strategy
    /// allocation self-blocks (production: all four 0.75 SOL entries on
    /// 2026-08-18 dead-lettered against the 1.2 SOL Shield allocation).
    #[tokio::test]
    async fn test_can_open_strategy_position_self_exclusion() {
        // 100 capital, 20% heat -> 20 SOL. SHIELD 60% -> 12 SOL allocation.
        // The trade being processed ("self") is a 7.5 SOL SHIELD position
        // (flushed) AND a QUEUED 7.5 SOL trade (in-flight) — both count
        // toward heat in the naive path.
        let self_uuid = "self-trade";
        let mut trades = HashMap::new();
        trades.insert(
            "QUEUED".to_string(),
            vec![buy_trade(self_uuid, "SHIELD", dec!(7.5), now_ago_secs(10))],
        );
        let db = heat_db(
            vec![position(
                self_uuid,
                "SHIELD",
                "ACTIVE",
                dec!(7.5),
                now_ago_secs(10),
            )],
            trades,
        );
        let heat = PortfolioHeat::new(db, dec!(100));

        // Naive (non-excluding): 7.5 (position) + 7.5 (queued) + 7.5 (new)
        // = 22.5 > 12 -> blocked. This documents the double-count.
        assert!(!heat
            .can_open_strategy_position(chimera_core::models::Strategy::Shield, dec!(7.5), 60, 40)
            .await
            .unwrap());

        // Self-excluding: the trade's own exposure is counted exactly once
        // (in position_size_sol) -> 0 + 7.5 <= 12 -> allowed.
        let mut trades = HashMap::new();
        trades.insert(
            "QUEUED".to_string(),
            vec![buy_trade(self_uuid, "SHIELD", dec!(7.5), now_ago_secs(10))],
        );
        let db = heat_db(
            vec![position(
                self_uuid,
                "SHIELD",
                "ACTIVE",
                dec!(7.5),
                now_ago_secs(10),
            )],
            trades,
        );
        let heat = PortfolioHeat::new(db, dec!(100));
        assert!(heat
            .can_open_strategy_position_excluding(
                chimera_core::models::Strategy::Shield,
                dec!(7.5),
                60,
                40,
                Some(self_uuid)
            )
            .await
            .unwrap());

        // Exclusion must not mask OTHER positions: a different trade's 7.5
        // SOL + our new 7.5 = 15 > 12 -> still blocked.
        let mut trades = HashMap::new();
        trades.insert(
            "QUEUED".to_string(),
            vec![buy_trade("other-trade", "SHIELD", dec!(7.5), now_ago_secs(10))],
        );
        let db = heat_db(
            vec![position(
                "other-pos",
                "SHIELD",
                "ACTIVE",
                dec!(7.5),
                now_ago_secs(10),
            )],
            trades,
        );
        let heat = PortfolioHeat::new(db, dec!(100));
        assert!(!heat
            .can_open_strategy_position_excluding(
                chimera_core::models::Strategy::Shield,
                dec!(7.5),
                60,
                40,
                Some(self_uuid)
            )
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_update_capital_and_critical_threshold() {
        let db = heat_db(Vec::new(), HashMap::new());
        let heat = PortfolioHeat::new(db, dec!(100));
        assert_eq!(heat.get_critical_threshold_sol(), dec!(30)); // 100*20%*1.5

        heat.update_capital(dec!(200));
        assert_eq!(heat.get_critical_threshold_sol(), dec!(60));
    }

    #[tokio::test]
    async fn test_with_max_heat_clamped() {
        let db = heat_db(Vec::new(), HashMap::new());
        let heat = PortfolioHeat::with_max_heat(db, dec!(100), dec!(150));
        assert_eq!(heat.max_heat_percent, dec!(100));

        let db = heat_db(Vec::new(), HashMap::new());
        let heat = PortfolioHeat::with_max_heat(db, dec!(100), dec!(-10));
        assert_eq!(heat.max_heat_percent, Decimal::ZERO);

        let db = heat_db(Vec::new(), HashMap::new());
        let heat = PortfolioHeat::with_max_heat(db, dec!(100), dec!(25));
        assert_eq!(heat.max_heat_percent, dec!(25));
        let result = heat.calculate_heat().await.unwrap();
        assert_eq!(result.available_heat_sol, dec!(25));
    }
}
