//! State coordinator for registry-database synchronization
//!
//! Manages consistency between in-memory state and database, handles startup recovery,
//! and provides periodic synchronization.

use crate::db_abstraction::Database;
use crate::state::registry::{PositionState, PortfolioHeatState, StateRegistry, TradeState, TradeStatus, WalletState};
use crate::state::write_queue::AsyncWriteQueue;
use rust_decimal::Decimal;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::time::interval;
use tracing::{debug, error, info, warn};

/// State coordinator manages synchronization between registry and database
pub struct StateCoordinator {
    registry: Arc<StateRegistry>,
    db: Arc<dyn Database>,
    write_queue: Arc<AsyncWriteQueue>,
    sync_interval: Duration,
    last_sync: Arc<std::sync::Mutex<Instant>>,
}

impl StateCoordinator {
    /// Create a new state coordinator
    pub fn new(
        registry: Arc<StateRegistry>,
        db: Arc<dyn Database>,
        write_queue: Arc<AsyncWriteQueue>,
        sync_interval: Duration,
    ) -> Self {
        info!("Creating state coordinator with sync interval: {:?}", sync_interval);
        Self {
            registry,
            db,
            write_queue,
            sync_interval,
            last_sync: Arc::new(std::sync::Mutex::new(Instant::now())),
        }
    }

    /// Load initial state from database into registry
    pub async fn load_initial_state(&self) -> Result<(), CoordinatorError> {
        info!("Loading initial state from database...");
        let start = Instant::now();

        // Load active trades
        self.load_active_trades().await?;

        // Load active positions
        self.load_active_positions().await?;

        // Load wallets
        self.load_wallets().await?;

        // Calculate portfolio heat
        self.calculate_portfolio_heat().await?;

        let duration = start.elapsed();
        info!("Initial state loaded in {}ms", duration.as_millis());

        Ok(())
    }

    /// Start periodic synchronization task
    pub async fn start_sync_task(&self) -> Result<(), CoordinatorError> {
        let registry = Arc::clone(&self.registry);
        let db = Arc::clone(&self.db);
        let sync_interval = self.sync_interval;
        let last_sync = Arc::clone(&self.last_sync);

        info!("Starting periodic sync task (interval: {:?})", sync_interval);

        tokio::spawn(async move {
            let mut timer = interval(sync_interval);
            timer.tick().await; // Skip first tick

            loop {
                timer.tick().await;

                debug!("Running periodic state synchronization");
                let start = Instant::now();

                // Sync active trades
                if let Err(e) = Self::sync_active_trades(&registry, &db).await {
                    error!("Failed to sync active trades: {}", e);
                }

                // Sync active positions
                if let Err(e) = Self::sync_active_positions(&registry, &db).await {
                    error!("Failed to sync active positions: {}", e);
                }

                // Sync wallets
                if let Err(e) = Self::sync_wallets(&registry, &db).await {
                    error!("Failed to sync wallets: {}", e);
                }

                *last_sync.lock().unwrap() = Instant::now();

                let duration = start.elapsed();
                debug!("State synchronization completed in {}ms", duration.as_millis());
            }
        });

        Ok(())
    }

    /// Load active trades from database
    async fn load_active_trades(&self) -> Result<(), CoordinatorError> {
        let statuses = ["PENDING", "QUEUED", "EXECUTING", "ACTIVE", "EXITING"];

        for status in &statuses {
            let trades = self.db.get_trades_by_status(status, i32::MAX).await
                .map_err(|e| CoordinatorError::DatabaseError(format!("Failed to load trades: {}", e)))?;
            let trades_count = trades.len();

            for trade in trades {
                let trade_state = TradeState {
                    trade_uuid: trade.trade_uuid.clone(),
                    status: Self::db_status_to_trade_status(&trade.status),
                    wallet_address: trade.wallet_address.clone(),
                    token_address: trade.token_address.clone(),
                    token_symbol: trade.token_symbol.clone(),
                    strategy: trade.strategy.clone(),
                    side: trade.side.clone(),
                    amount_sol: trade.amount_sol,
                    updated_at: SystemTime::now(),
                    version: 1,
                };

                self.registry.insert_trade(trade_state)
                    .map_err(|e| CoordinatorError::RegistryError(format!("Failed to insert trade: {}", e)))?;
            }

            debug!("Loaded {} trades with status {}", trades_count, status);
        }

        Ok(())
    }

    /// Load active positions from database
    async fn load_active_positions(&self) -> Result<(), CoordinatorError> {
        let positions = self.db.get_active_positions().await
            .map_err(|e| CoordinatorError::DatabaseError(format!("Failed to load positions: {}", e)))?;
        let positions_count = positions.len();

        for position in positions {
            let position_state = PositionState {
                trade_uuid: position.trade_uuid.clone(),
                wallet_address: position.wallet_address.clone(),
                token_address: position.token_address.clone(),
                token_symbol: position.token_symbol.clone(),
                state: position.state.clone(),
                entry_amount_sol: position.entry_amount_sol,
                current_price: position.current_price,
                unrealized_pnl_sol: position.unrealized_pnl_sol,
                updated_at: SystemTime::now(),
            };

            self.registry.insert_position(position_state)
                .map_err(|e| CoordinatorError::RegistryError(format!("Failed to insert position: {}", e)))?;
        }

        debug!("Loaded {} active positions", positions_count);
        Ok(())
    }

    /// Load wallets from database
    async fn load_wallets(&self) -> Result<(), CoordinatorError> {
        let wallet_details = self.db.get_wallets(None).await
            .map_err(|e| CoordinatorError::DatabaseError(format!("Failed to load wallets: {}", e)))?;
        let wallets_count = wallet_details.len();

        for wallet in wallet_details {
            let wallet_state = WalletState {
                address: wallet.address.clone(),
                status: wallet.status.clone(),
                wqs_score: wallet.wqs_score,
                win_rate: wallet.win_rate,
                updated_at: SystemTime::now(),
            };

            self.registry.upsert_wallet(wallet_state)
                .map_err(|e| CoordinatorError::RegistryError(format!("Failed to insert wallet: {}", e)))?;
        }

        debug!("Loaded {} wallets", wallets_count);
        Ok(())
    }

    /// Calculate portfolio heat from loaded state
    async fn calculate_portfolio_heat(&self) -> Result<(), CoordinatorError> {
        let positions = self.registry.get_active_positions();
        let mut total_exposure = Decimal::ZERO;
        let mut shield_exposure = Decimal::ZERO;
        let mut spear_exposure = Decimal::ZERO;

        for position in &positions {
            total_exposure += position.entry_amount_sol;

            // Note: Would need strategy info from PositionState for accurate split
            // For now, evenly distribute
            shield_exposure += position.entry_amount_sol / Decimal::from(2);
            spear_exposure += position.entry_amount_sol / Decimal::from(2);
        }

        // Add pending trades
        for trade in self.registry.get_all_trades() {
            if matches!(trade.status, TradeStatus::Pending | TradeStatus::Queued | TradeStatus::Executing) {
                if trade.side == "BUY" {
                    total_exposure += trade.amount_sol;
                    // Distribute pending heat
                    shield_exposure += trade.amount_sol / Decimal::from(2);
                    spear_exposure += trade.amount_sol / Decimal::from(2);
                }
            }
        }

        self.registry.update_portfolio_heat(PortfolioHeatState {
            total_exposure_sol: total_exposure,
            shield_exposure_sol: shield_exposure,
            spear_exposure_sol: spear_exposure,
            pending_heat_sol: Decimal::ZERO,
            last_updated: SystemTime::now(),
        }).map_err(|e| CoordinatorError::RegistryError(format!("Failed to update portfolio heat: {}", e)))?;

        debug!("Calculated portfolio heat: {} SOL (shield: {}, spear: {})",
               total_exposure, shield_exposure, spear_exposure);
        Ok(())
    }

    /// Sync active trades between registry and database
    async fn sync_active_trades(registry: &Arc<StateRegistry>, db: &Arc<dyn Database>) -> Result<(), CoordinatorError> {
        let statuses = ["PENDING", "QUEUED", "EXECUTING", "ACTIVE", "EXITING"];

        for status in &statuses {
            let db_trades = db.get_trades_by_status(status, i32::MAX).await
                .map_err(|e| CoordinatorError::DatabaseError(format!("Failed to fetch trades: {}", e)))?;

            for db_trade in db_trades {
                let reg_trade = registry.get_trade(&db_trade.trade_uuid);

                match reg_trade {
                    Some(reg_trade) => {
                        // Check if status matches
                        let reg_status: String = reg_trade.status.clone().into();
                        if reg_status != db_trade.status {
                            warn!("Status mismatch for trade {}: registry={:?}, db={}",
                                  db_trade.trade_uuid, reg_trade.status, db_trade.status);
                            // Update registry to match database (database is source of truth)
                            registry.update_trade_status(&db_trade.trade_uuid,
                                                       Self::db_status_to_trade_status(&db_trade.status))
                                .map_err(|e| CoordinatorError::RegistryError(format!("Failed to update trade status: {}", e)))?;
                        }
                    }
                    None => {
                        // Trade in DB but not in registry - add it
                        debug!("Adding missing trade to registry: {}", db_trade.trade_uuid);
                        let trade_state = TradeState {
                            trade_uuid: db_trade.trade_uuid.clone(),
                            status: Self::db_status_to_trade_status(&db_trade.status),
                            wallet_address: db_trade.wallet_address.clone(),
                            token_address: db_trade.token_address.clone(),
                            token_symbol: db_trade.token_symbol.clone(),
                            strategy: db_trade.strategy.clone(),
                            side: db_trade.side.clone(),
                            amount_sol: db_trade.amount_sol,
                            updated_at: SystemTime::now(),
                            version: 1,
                        };
                        registry.insert_trade(trade_state)
                            .map_err(|e| CoordinatorError::RegistryError(format!("Failed to insert trade: {}", e)))?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Sync active positions between registry and database
    async fn sync_active_positions(registry: &Arc<StateRegistry>, db: &Arc<dyn Database>) -> Result<(), CoordinatorError> {
        let db_positions = db.get_active_positions().await
            .map_err(|e| CoordinatorError::DatabaseError(format!("Failed to fetch positions: {}", e)))?;

        for db_position in db_positions {
            let reg_position = registry.get_position_by_trade_uuid(&db_position.trade_uuid);

            match reg_position {
                Some(reg_position) => {
                    // Check if state matches
                    if reg_position.state != db_position.state {
                        warn!("State mismatch for position {}: registry={}, db={}",
                              db_position.trade_uuid, reg_position.state, db_position.state);
                        // Update registry to match database
                        registry.update_position_state(&db_position.trade_uuid, &db_position.state)
                            .map_err(|e| CoordinatorError::RegistryError(format!("Failed to update position state: {}", e)))?;
                    }
                }
                None => {
                    // Position in DB but not in registry - add it
                    debug!("Adding missing position to registry: {}", db_position.trade_uuid);
                    let position_state = PositionState {
                        trade_uuid: db_position.trade_uuid.clone(),
                        wallet_address: db_position.wallet_address.clone(),
                        token_address: db_position.token_address.clone(),
                        token_symbol: db_position.token_symbol.clone(),
                        state: db_position.state.clone(),
                        entry_amount_sol: db_position.entry_amount_sol,
                        current_price: db_position.current_price,
                        unrealized_pnl_sol: db_position.unrealized_pnl_sol,
                        updated_at: SystemTime::now(),
                    };
                    registry.insert_position(position_state)
                        .map_err(|e| CoordinatorError::RegistryError(format!("Failed to insert position: {}", e)))?;
                }
            }
        }

        Ok(())
    }

    /// Sync wallets between registry and database
    async fn sync_wallets(registry: &Arc<StateRegistry>, db: &Arc<dyn Database>) -> Result<(), CoordinatorError> {
        let db_wallets = db.get_wallets(None).await
            .map_err(|e| CoordinatorError::DatabaseError(format!("Failed to fetch wallets: {}", e)))?;

        for db_wallet in db_wallets {
            let reg_wallet = registry.get_wallet(&db_wallet.address);

            match reg_wallet {
                Some(reg_wallet) => {
                    // Check if status matches
                    if reg_wallet.status != db_wallet.status {
                        warn!("Status mismatch for wallet {}: registry={}, db={}",
                              db_wallet.address, reg_wallet.status, db_wallet.status);
                        // Update registry to match database
                        let wallet_state = WalletState {
                            address: db_wallet.address.clone(),
                            status: db_wallet.status.clone(),
                            wqs_score: db_wallet.wqs_score,
                            win_rate: db_wallet.win_rate,
                            updated_at: SystemTime::now(),
                        };
                        registry.upsert_wallet(wallet_state)
                            .map_err(|e| CoordinatorError::RegistryError(format!("Failed to update wallet: {}", e)))?;
                    }
                }
                None => {
                    // Wallet in DB but not in registry - add it
                    debug!("Adding missing wallet to registry: {}", db_wallet.address);
                    let wallet_state = WalletState {
                        address: db_wallet.address.clone(),
                        status: db_wallet.status.clone(),
                        wqs_score: db_wallet.wqs_score,
                        win_rate: db_wallet.win_rate,
                        updated_at: SystemTime::now(),
                    };
                    registry.upsert_wallet(wallet_state)
                        .map_err(|e| CoordinatorError::RegistryError(format!("Failed to insert wallet: {}", e)))?;
                }
            }
        }

        Ok(())
    }

    /// Convert database status string to TradeStatus enum
    fn db_status_to_trade_status(status: &str) -> TradeStatus {
        match status {
            "PENDING" => TradeStatus::Pending,
            "QUEUED" => TradeStatus::Queued,
            "EXECUTING" => TradeStatus::Executing,
            "ACTIVE" => TradeStatus::Active,
            "EXITING" => TradeStatus::Exiting,
            "CLOSED" => TradeStatus::Closed,
            "FAILED" => TradeStatus::Failed,
            "DEAD_LETTER" => TradeStatus::DeadLetter,
            _ => TradeStatus::Failed,
        }
    }
}

/// Coordinator errors
#[derive(Debug, thiserror::Error)]
pub enum CoordinatorError {
    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("Registry error: {0}")]
    RegistryError(String),

    #[error("Sync error: {0}")]
    SyncError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_status_to_trade_status() {
        assert_eq!(StateCoordinator::db_status_to_trade_status("PENDING"), TradeStatus::Pending);
        assert_eq!(StateCoordinator::db_status_to_trade_status("ACTIVE"), TradeStatus::Active);
        assert_eq!(StateCoordinator::db_status_to_trade_status("UNKNOWN"), TradeStatus::Failed);
    }
}
