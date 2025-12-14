//! Portfolio Heat Management
//!
//! Tracks total portfolio risk exposure and blocks new positions
//! when heat limit (20% of capital) is reached.

use crate::db::DbPool;
use rust_decimal::prelude::*;

/// Portfolio heat manager
pub struct PortfolioHeat {
    db: DbPool,
    /// Maximum portfolio heat as percentage of capital (default: 20%)
    max_heat_percent: Decimal,
    /// Total capital in SOL (for heat calculation, using Decimal for precision)
    total_capital_sol: Decimal,
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
            max_heat_percent: Decimal::from_f64_retain(20.0).unwrap_or(Decimal::ZERO), // 20% max heat
            total_capital_sol,
        }
    }

    /// Create with custom max heat percentage
    pub fn with_max_heat(db: DbPool, total_capital_sol: Decimal, max_heat_percent: Decimal) -> Self {
        let max_heat = max_heat_percent.max(Decimal::ZERO).min(Decimal::from(100));
        Self {
            db,
            max_heat_percent: max_heat,
            total_capital_sol,
        }
    }

    /// Calculate current portfolio heat
    ///
    /// # Returns
    /// HeatResult with current heat status
    pub async fn calculate_heat(&self) -> Result<HeatResult, String> {
        // Get total exposure from active positions (convert from database f64 to Decimal)
        let total_exposure_f64: f64 = sqlx::query_scalar::<_, f64>(
            r#"
            SELECT COALESCE(SUM(entry_amount_sol), 0.0)
            FROM positions
            WHERE state = 'ACTIVE'
            "#
        )
        .fetch_one(&self.db)
        .await
        .map_err(|e| format!("Failed to query portfolio heat: {}", e))?;
        let total_exposure = Decimal::from_f64_retain(total_exposure_f64).unwrap_or(Decimal::ZERO);

        // Calculate heat percentage using Decimal for precision
        let current_heat_percent = if !self.total_capital_sol.is_zero() {
            (total_exposure / self.total_capital_sol) * Decimal::from(100)
        } else {
            Decimal::ZERO
        };

        // Calculate available heat
        let max_heat_sol = self.total_capital_sol * (self.max_heat_percent / Decimal::from(100));
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
        let new_heat_percent = if !self.total_capital_sol.is_zero() {
            (new_exposure / self.total_capital_sol) * Decimal::from(100)
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
        let shield_heat_f64: f64 = sqlx::query_scalar::<_, f64>(
            r#"
            SELECT COALESCE(SUM(entry_amount_sol), 0.0)
            FROM positions
            WHERE state = 'ACTIVE' AND strategy = 'SHIELD'
            "#
        )
        .fetch_one(&self.db)
        .await
        .map_err(|e| format!("Failed to query Shield heat: {}", e))?;
        let shield_heat = Decimal::from_f64_retain(shield_heat_f64).unwrap_or(Decimal::ZERO);

        let spear_heat_f64: f64 = sqlx::query_scalar::<_, f64>(
            r#"
            SELECT COALESCE(SUM(entry_amount_sol), 0.0)
            FROM positions
            WHERE state = 'ACTIVE' AND strategy = 'SPEAR'
            "#
        )
        .fetch_one(&self.db)
        .await
        .map_err(|e| format!("Failed to query Spear heat: {}", e))?;
        let spear_heat = Decimal::from_f64_retain(spear_heat_f64).unwrap_or(Decimal::ZERO);

        Ok((shield_heat, spear_heat))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heat_calculation() {
        // This would be tested with actual database in integration tests
    }
}




