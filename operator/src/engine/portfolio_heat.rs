//! Portfolio Heat Management
//!
//! Tracks total portfolio risk exposure and blocks new positions
//! when heat limit (20% of capital) is reached.

use crate::db::DbPool;

/// Portfolio heat manager
pub struct PortfolioHeat {
    db: DbPool,
    /// Maximum portfolio heat as percentage of capital (default: 20%)
    max_heat_percent: f64,
    /// Total capital in SOL (for heat calculation)
    total_capital_sol: f64,
}

/// Portfolio heat result
#[derive(Debug, Clone)]
pub struct HeatResult {
    /// Current heat percentage (0.0-100.0)
    pub current_heat_percent: f64,
    /// Total exposure in SOL
    pub total_exposure_sol: f64,
    /// Available heat capacity in SOL
    pub available_heat_sol: f64,
    /// Whether new positions can be opened
    pub can_open_position: bool,
}

impl PortfolioHeat {
    /// Create a new portfolio heat manager
    pub fn new(db: DbPool, total_capital_sol: f64) -> Self {
        Self {
            db,
            max_heat_percent: 20.0, // 20% max heat
            total_capital_sol,
        }
    }

    /// Create with custom max heat percentage
    pub fn with_max_heat(db: DbPool, total_capital_sol: f64, max_heat_percent: f64) -> Self {
        Self {
            db,
            max_heat_percent: max_heat_percent.max(0.0).min(100.0),
            total_capital_sol,
        }
    }

    /// Calculate current portfolio heat
    ///
    /// # Returns
    /// HeatResult with current heat status
    pub async fn calculate_heat(&self) -> Result<HeatResult, String> {
        // Get total exposure from active positions
        let total_exposure: f64 = sqlx::query_scalar::<_, f64>(
            r#"
            SELECT COALESCE(SUM(entry_amount_sol), 0.0)
            FROM positions
            WHERE state = 'ACTIVE'
            "#
        )
        .fetch_one(&self.db)
        .await
        .map_err(|e| format!("Failed to query portfolio heat: {}", e))?;

        // Calculate heat percentage
        let current_heat_percent = if self.total_capital_sol > 0.0 {
            (total_exposure / self.total_capital_sol) * 100.0
        } else {
            0.0
        };

        // Calculate available heat
        let max_heat_sol = self.total_capital_sol * (self.max_heat_percent / 100.0);
        let available_heat_sol = max_heat_sol - total_exposure;

        // Check if can open new position
        let can_open_position = current_heat_percent < self.max_heat_percent;

        Ok(HeatResult {
            current_heat_percent,
            total_exposure_sol: total_exposure,
            available_heat_sol: available_heat_sol.max(0.0),
            can_open_position,
        })
    }

    /// Check if a new position of given size can be opened
    ///
    /// # Arguments
    /// * `position_size_sol` - Size of new position in SOL
    ///
    /// # Returns
    /// true if position can be opened, false otherwise
    pub async fn can_open_position(&self, position_size_sol: f64) -> Result<bool, String> {
        let heat = self.calculate_heat().await?;
        
        if !heat.can_open_position {
            return Ok(false);
        }

        // Check if new position would exceed heat limit
        let new_exposure = heat.total_exposure_sol + position_size_sol;
        let new_heat_percent = if self.total_capital_sol > 0.0 {
            (new_exposure / self.total_capital_sol) * 100.0
        } else {
            0.0
        };

        Ok(new_heat_percent <= self.max_heat_percent)
    }

    /// Get heat breakdown by strategy
    ///
    /// # Returns
    /// Tuple of (shield_heat_sol, spear_heat_sol)
    pub async fn get_strategy_heat(&self) -> Result<(f64, f64), String> {
        let shield_heat: f64 = sqlx::query_scalar::<_, f64>(
            r#"
            SELECT COALESCE(SUM(entry_amount_sol), 0.0)
            FROM positions
            WHERE state = 'ACTIVE' AND strategy = 'SHIELD'
            "#
        )
        .fetch_one(&self.db)
        .await
        .map_err(|e| format!("Failed to query Shield heat: {}", e))?;

        let spear_heat: f64 = sqlx::query_scalar::<_, f64>(
            r#"
            SELECT COALESCE(SUM(entry_amount_sol), 0.0)
            FROM positions
            WHERE state = 'ACTIVE' AND strategy = 'SPEAR'
            "#
        )
        .fetch_one(&self.db)
        .await
        .map_err(|e| format!("Failed to query Spear heat: {}", e))?;

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


