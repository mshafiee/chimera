//! Portfolio Heat Management
//!
//! Tracks total portfolio risk exposure and blocks new positions
//! when heat limit (20% of capital) is reached.

use crate::db::DbPool;
use parking_lot::RwLock;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use std::sync::Arc;

/// Portfolio heat manager
pub struct PortfolioHeat {
    db: DbPool,
    /// Maximum portfolio heat as percentage of capital (default: 20%)
    max_heat_percent: Decimal,
    /// Total capital in SOL — wrapped in Arc<RwLock> so the background wallet-balance
    /// refresh task can update it without rebuilding the struct.
    total_capital_sol: Arc<RwLock<Decimal>>,
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
    /// Create a new portfolio heat manager
    pub fn new(db: DbPool, total_capital_sol: Decimal) -> Self {
        Self {
            db,
            max_heat_percent: dec!(20),
            total_capital_sol: Arc::new(RwLock::new(total_capital_sol)),
        }
    }

    /// Create with custom max heat percentage
    pub fn with_max_heat(
        db: DbPool,
        total_capital_sol: Decimal,
        max_heat_percent: Decimal,
    ) -> Self {
        let max_heat = max_heat_percent.max(Decimal::ZERO).min(Decimal::from(100));
        Self {
            db,
            max_heat_percent: max_heat,
            total_capital_sol: Arc::new(RwLock::new(total_capital_sol)),
        }
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
        // Include EXITING positions — they still hold capital until exit confirms.
        // Use entry_amount_sol only: heat measures capital at risk (deployed capital),
        // not mark-to-market value. Including unrealized PnL inflates heat on winners
        // (blocking new trades) and deflates it on losers (allowing over-exposure).
        // Exclude EXITING positions that have been stuck for >15 minutes (900 seconds)
        // so that permanently failed recovery attempts don't lock capital forever.
        let total_exposure_f64: f64 = sqlx::query_scalar::<_, f64>(
            r#"
            SELECT COALESCE(SUM(entry_amount_sol), 0.0)
            FROM positions
            WHERE state = 'ACTIVE' 
               OR (state = 'EXITING' AND updated_at >= datetime('now', '-900 seconds'))
            "#,
        )
        .fetch_one(&self.db)
        .await
        .map_err(|e| format!("Failed to query portfolio heat: {}", e))?;

        // Warn when EXITING positions have been stuck longer than the recovery escalation
        // threshold (5 min). These should have been reverted to ACTIVE by recovery.rs, but
        // if they persist they lock capital. Alerting here lets operators catch recovery
        // failures before they compound.
        let stale_exiting_count: i64 = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT COUNT(*)
            FROM positions
            WHERE state = 'EXITING'
              AND updated_at < datetime('now', '-300 seconds')
            "#,
        )
        .fetch_one(&self.db)
        .await
        .unwrap_or(0);
        if stale_exiting_count > 0 {
            let stale_sol: f64 = sqlx::query_scalar::<_, f64>(
                r#"
                SELECT COALESCE(SUM(entry_amount_sol), 0.0)
                FROM positions
                WHERE state = 'EXITING'
                  AND updated_at < datetime('now', '-300 seconds')
                "#,
            )
            .fetch_one(&self.db)
            .await
            .unwrap_or(0.0);
            tracing::warn!(
                stale_exiting_count,
                stale_exposure_sol = stale_sol,
                "STALE_EXITING: positions stuck >5 min are locking portfolio heat; \
                 check recovery.rs background task and RPC connectivity"
            );
        }
        let total_exposure = Decimal::from_f64_retain(total_exposure_f64).unwrap_or(Decimal::ZERO);
        let capital = *self.total_capital_sol.read();

        // Calculate heat percentage using Decimal for precision
        let current_heat_percent = if !capital.is_zero() {
            (total_exposure / capital) * Decimal::from(100)
        } else {
            Decimal::ZERO
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
        let heat = self.calculate_heat().await?;

        if !heat.can_open_position {
            return Ok(false);
        }

        // Check if new position would exceed heat limit using Decimal for precision
        let new_exposure = heat.total_exposure_sol + position_size_sol;
        let capital = *self.total_capital_sol.read();
        let new_heat_percent = if !capital.is_zero() {
            (new_exposure / capital) * Decimal::from(100)
        } else {
            Decimal::ZERO
        };

        Ok(new_heat_percent <= self.max_heat_percent)
    }

    /// Get heat breakdown by strategy
    ///
    /// # Returns
    /// Tuple of (shield_heat_sol, spear_heat_sol) using Decimal for precision
    pub async fn get_strategy_heat(&self) -> Result<(Decimal, Decimal), String> {
        let rows = sqlx::query_as::<_, (String, f64)>(
            r#"
            SELECT strategy, COALESCE(SUM(entry_amount_sol), 0.0) as heat
            FROM positions
            WHERE state = 'ACTIVE' 
               OR (state = 'EXITING' AND updated_at >= datetime('now', '-900 seconds'))
            GROUP BY strategy
            "#,
        )
        .fetch_all(&self.db)
        .await
        .map_err(|e| format!("Failed to query strategy heat: {}", e))?;

        let mut shield_heat = Decimal::ZERO;
        let mut spear_heat = Decimal::ZERO;

        for (strategy, heat_val) in rows {
            let heat = Decimal::from_f64_retain(heat_val).unwrap_or(Decimal::ZERO);
            match strategy.as_str() {
                "SHIELD" => shield_heat = heat,
                "SPEAR" => spear_heat = heat,
                _ => {}
            }
        }

        Ok((shield_heat, spear_heat))
    }

    pub async fn can_open_strategy_position(
        &self,
        strategy: crate::models::Strategy,
        position_size_sol: Decimal,
        shield_percent: u32,
        spear_percent: u32,
    ) -> Result<bool, String> {
        if !self.can_open_position(position_size_sol).await? {
            return Ok(false);
        }

        let (shield_heat, spear_heat) = self.get_strategy_heat().await?;
        let allocation_pct = match strategy {
            crate::models::Strategy::Shield => Decimal::from(shield_percent),
            crate::models::Strategy::Spear => Decimal::from(spear_percent),
            _ => return Ok(true),
        };
        if allocation_pct.is_zero() {
            // 0% allocation means this strategy is disabled — block all positions
            return Ok(false);
        }
        let allocated_sol = *self.total_capital_sol.read() * (allocation_pct / Decimal::from(100));
        let current_heat = match strategy {
            crate::models::Strategy::Shield => shield_heat,
            crate::models::Strategy::Spear => spear_heat,
            _ => Decimal::ZERO,
        };
        Ok(current_heat + position_size_sol <= allocated_sol)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_heat_calculation() {
        // This would be tested with actual database in integration tests
    }
}
