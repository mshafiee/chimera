//! In-memory `Database` implementation for unit tests.
//!
//! Only the methods exercised by the monitoring code under test are
//! implemented; everything else panics with `unimplemented!()` so a test
//! accidentally depending on an unexpected method fails loudly.
//! Error injection: each `*_error` flag makes the corresponding method
//! return `Err` so error paths can be exercised deterministically.

use rust_decimal::Decimal;
    use crate::db_abstraction::*;
use crate::error::{AppError, AppResult};
use rust_decimal::prelude::ToPrimitive;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct MockDb {
    pub wallets: Arc<Mutex<HashMap<String, Wallet>>>,
    pub wallet_monitoring: Arc<Mutex<HashMap<String, WalletMonitoring>>>,
    pub webhook_config: Arc<Mutex<HashMap<String, String>>>,
    pub trade_uuids: Arc<Mutex<Vec<String>>>,
    pub last_speculative_timestamps: Arc<Mutex<HashMap<String, chrono::DateTime<chrono::Utc>>>>,
    pub wallet_query_error: Arc<AtomicBool>,
    pub wallets_by_status_error: Arc<AtomicBool>,
    pub tier_query_error: Arc<AtomicBool>,
    pub monitoring_all_error: Arc<AtomicBool>,
    pub monitoring_error: Arc<AtomicBool>,
    pub signature_error: Arc<AtomicBool>,
    pub speculative_error: Arc<AtomicBool>,
    pub uuid_error: Arc<AtomicBool>,
    pub circuit_breaker_state: Arc<Mutex<Option<CircuitBreakerState>>>,
    pub evaluation_data: Arc<
        Mutex<
            Option<(
                rust_decimal::Decimal,
                rust_decimal::Decimal,
                rust_decimal::Decimal,
                rust_decimal::Decimal,
            )>,
        >,
    >,
    pub consecutive_losses: Arc<Mutex<Option<u32>>>,
    pub drawdown: Arc<Mutex<Option<(rust_decimal::Decimal, rust_decimal::Decimal)>>>,
    pub evaluation_error: Arc<AtomicBool>,
    pub consecutive_error: Arc<AtomicBool>,
    pub drawdown_error: Arc<AtomicBool>,
    pub cb_state_error: Arc<AtomicBool>,
    pub cb_update_error: Arc<AtomicBool>,
    pub exit_targets: Arc<Mutex<HashMap<String, ExitTargetData>>>,
    pub exit_target_upserts: Arc<Mutex<Vec<String>>>,
    pub exit_target_deletes: Arc<Mutex<Vec<String>>>,
    pub active_positions: Arc<Mutex<Vec<Position>>>,
    pub exit_target_error: Arc<AtomicBool>,
    pub exit_target_delete_error: Arc<AtomicBool>,
    pub active_positions_error: Arc<AtomicBool>,
}

impl MockDb {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_wallet(&self, wallet: Wallet) {
        self.wallets
            .lock()
            .unwrap()
            .insert(wallet.address.clone(), wallet);
    }

    pub fn add_wallet_monitoring(&self, wm: WalletMonitoring) {
        self.wallet_monitoring
            .lock()
            .unwrap()
            .insert(wm.wallet_address.clone(), wm);
    }

    pub fn set_webhook_config(&self, key: &str, value: String) {
        self.webhook_config
            .lock()
            .unwrap()
            .insert(key.to_string(), value);
    }

    pub fn add_trade_uuid(&self, uuid: &str) {
        self.trade_uuids.lock().unwrap().push(uuid.to_string());
    }
}

#[async_trait::async_trait]
impl Database for MockDb {
    fn pool(&self) -> DbPool {
        unimplemented!("MockDb::pool not implemented")
    }

    async fn close(&self) -> AppResult<()> {
        unimplemented!("MockDb::close not implemented")
    }

    async fn run_migrations(&self) -> AppResult<()> {
        unimplemented!("MockDb::run_migrations not implemented")
    }

    async fn startup_integrity_check(&self) -> AppResult<()> {
        unimplemented!("MockDb::startup_integrity_check not implemented")
    }

    async fn recover_executing_trades(&self) -> AppResult<u32> {
        unimplemented!("MockDb::recover_executing_trades not implemented")
    }

    async fn trade_uuid_exists(&self, trade_uuid: &str) -> AppResult<bool> {
        if self.uuid_error.load(Ordering::Relaxed) {
            return Err(AppError::Internal(
                "injected trade_uuid_exists error".to_string(),
            ));
        }
        Ok(self
            .trade_uuids
            .lock()
            .unwrap()
            .contains(&trade_uuid.to_string()))
    }

    async fn insert_trade(&self, trade: &InsertTrade) -> AppResult<i64> {
        unimplemented!("MockDb::insert_trade not implemented")
    }

    async fn update_trade_status(&self, update: &UpdateTradeStatus) -> AppResult<()> {
        unimplemented!("MockDb::update_trade_status not implemented")
    }

    async fn get_trade_by_uuid(&self, trade_uuid: &str) -> AppResult<Option<Trade>> {
        unimplemented!("MockDb::get_trade_by_uuid not implemented")
    }

    async fn get_queued_trades(&self, limit: i32) -> AppResult<Vec<Trade>> {
        unimplemented!("MockDb::get_queued_trades not implemented")
    }

    async fn get_trades_by_status(&self, status: &str, limit: i32) -> AppResult<Vec<Trade>> {
        unimplemented!("MockDb::get_trades_by_status not implemented")
    }

    async fn insert_position(&self, position: &InsertPosition) -> AppResult<i64> {
        unimplemented!("MockDb::insert_position not implemented")
    }

    async fn update_position(&self, update: &UpdatePosition) -> AppResult<()> {
        unimplemented!("MockDb::update_position not implemented")
    }

    async fn get_active_positions(&self) -> AppResult<Vec<Position>> {
        if self.active_positions_error.load(Ordering::Relaxed) {
            return Err(AppError::Internal(
                "injected active positions error".to_string(),
            ));
        }
        Ok(self.active_positions.lock().unwrap().clone())
    }

    async fn get_position_by_trade_uuid(&self, trade_uuid: &str) -> AppResult<Option<Position>> {
        unimplemented!("MockDb::get_position_by_trade_uuid not implemented")
    }

    async fn get_active_position_by_wallet_token(
        &self,
        wallet_address: &str,
        token_address: &str,
    ) -> AppResult<Option<Position>> {
        unimplemented!("MockDb::get_active_position_by_wallet_token not implemented")
    }

    async fn get_unresolved_trade_by_wallet_token(
        &self,
        wallet_address: &str,
        token_address: &str,
    ) -> AppResult<Option<String>> {
        unimplemented!("MockDb::get_unresolved_trade_by_wallet_token not implemented")
    }

    async fn close_position(
        &self,
        trade_uuid: &str,
        exit_price: rust_decimal::Decimal,
        exit_tx_signature: &str,
        realized_pnl_sol: rust_decimal::Decimal,
        realized_pnl_usd: rust_decimal::Decimal,
    ) -> AppResult<()> {
        unimplemented!("MockDb::close_position not implemented")
    }

    async fn force_close_orphan_position(&self, trade_uuid: &str, reason: &str) -> AppResult<()> {
        unimplemented!("MockDb::force_close_orphan_position not implemented")
    }

    async fn get_wallet(&self, address: &str) -> AppResult<Option<Wallet>> {
        if self.wallet_query_error.load(Ordering::Relaxed) {
            return Err(AppError::Internal("injected get_wallet error".to_string()));
        }
        Ok(self.wallets.lock().unwrap().get(address).cloned())
    }

    async fn get_active_wallets(&self) -> AppResult<Vec<Wallet>> {
        unimplemented!("MockDb::get_active_wallets not implemented")
    }

    async fn update_wallet_status(&self, address: &str, status: &str) -> AppResult<()> {
        unimplemented!("MockDb::update_wallet_status not implemented")
    }

    async fn merge_roster(&self, roster_db_path: &str) -> AppResult<u32> {
        unimplemented!("MockDb::merge_roster not implemented")
    }

    async fn get_wallets_by_status(&self, status: &str) -> AppResult<Vec<Wallet>> {
        if self.wallets_by_status_error.load(Ordering::Relaxed) {
            return Err(AppError::Internal(
                "injected get_wallets_by_status error".to_string(),
            ));
        }
        Ok(self
            .wallets
            .lock()
            .unwrap()
            .values()
            .filter(|w| w.status == status)
            .cloned()
            .collect())
    }

    async fn get_wallets_by_conviction_tier(
        &self,
        tier: chimera_core::config::ConvictionTier,
    ) -> AppResult<Vec<Wallet>> {
        use chimera_core::config::ConvictionTier;
        if self.tier_query_error.load(Ordering::Relaxed) {
            return Err(AppError::Internal("injected tier query error".to_string()));
        }
        let (min_wqs, max_wqs) = match tier {
            ConvictionTier::High => (80u32, u32::MAX),
            ConvictionTier::Regular => (60, 79),
            ConvictionTier::Emerging => (0, 59),
        };
        Ok(self
            .wallets
            .lock()
            .unwrap()
            .values()
            .filter(|w| {
                let score = w.wqs_score.and_then(|s| s.to_u32()).unwrap_or(u32::MAX);
                w.status == "ACTIVE" && score >= min_wqs && score <= max_wqs
            })
            .cloned()
            .collect())
    }

    async fn get_wallets_with_wqs(
        &self,
        status: Option<&str>,
        min_wqs: Option<i32>,
        max_wqs: Option<i32>,
    ) -> AppResult<Vec<Wallet>> {
        unimplemented!("MockDb::get_wallets_with_wqs not implemented")
    }

    async fn get_promotion_candidates(
        &self,
        min_wqs: f64,
        max_age_days: i64,
        limit: i64,
    ) -> AppResult<Vec<Wallet>> {
        unimplemented!("MockDb::get_promotion_candidates not implemented")
    }

    async fn demote_dormant_active_wallets(&self, max_age_days: i64) -> AppResult<u64> {
        unimplemented!("MockDb::demote_dormant_active_wallets not implemented")
    }

    async fn get_circuit_breaker_state(&self) -> AppResult<CircuitBreakerState> {
        if self.cb_state_error.load(Ordering::Relaxed) {
            return Err(AppError::Internal("injected cb state error".to_string()));
        }
        let state = self
            .circuit_breaker_state
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(CircuitBreakerState {
                state: "Active".to_string(),
                tripped_at: None,
                trip_reason: None,
                updated_at: String::new(),
            });
        Ok(state)
    }

    async fn update_circuit_breaker_state(
        &self,
        state: &str,
        tripped_at: Option<chrono::DateTime<chrono::Utc>>,
        trip_reason: Option<&str>,
    ) -> AppResult<()> {
        if self.cb_update_error.load(Ordering::Relaxed) {
            return Err(AppError::Internal("injected cb update error".to_string()));
        }
        self.circuit_breaker_state
            .lock()
            .unwrap()
            .replace(CircuitBreakerState {
                state: state.to_string(),
                tripped_at: tripped_at.map(|t| t.to_rfc3339()),
                trip_reason: trip_reason.map(|r| r.to_string()),
                updated_at: chrono::Utc::now().to_rfc3339(),
            });
        Ok(())
    }

    async fn get_kill_switch_state(&self) -> AppResult<KillSwitchState> {
        unimplemented!("MockDb::get_kill_switch_state not implemented")
    }

    async fn set_kill_switch_state(&self, state: &str, reason: Option<&str>) -> AppResult<()> {
        unimplemented!("MockDb::set_kill_switch_state not implemented")
    }

    async fn insert_dlq(
        &self,
        trade_uuid: Option<&str>,
        payload: &str,
        reason: &str,
        error_details: Option<&str>,
        source_ip: Option<&str>,
    ) -> AppResult<i64> {
        unimplemented!("MockDb::insert_dlq not implemented")
    }

    async fn get_admin_wallet_role(&self, wallet_address: &str) -> AppResult<Option<String>> {
        unimplemented!("MockDb::get_admin_wallet_role not implemented")
    }

    async fn get_trade_statistics(&self) -> AppResult<TradeStatistics> {
        unimplemented!("MockDb::get_trade_statistics not implemented")
    }

    async fn get_recent_trades(&self, limit: i64, offset: i64) -> AppResult<Vec<Trade>> {
        unimplemented!("MockDb::get_recent_trades not implemented")
    }

    async fn get_wallet_performance(
        &self,
        wallet_address: &str,
    ) -> AppResult<Option<WalletPerformance>> {
        unimplemented!("MockDb::get_wallet_performance not implemented")
    }

    async fn get_pool_stats(&self) -> AppResult<PoolStats> {
        unimplemented!("MockDb::get_pool_stats not implemented")
    }

    async fn insert_jito_tip(
        &self,
        tip_amount_sol: &rust_decimal::Decimal,
        bundle_signature: Option<&str>,
        strategy: Option<&str>,
        success: bool,
    ) -> AppResult<i64> {
        unimplemented!("MockDb::insert_jito_tip not implemented")
    }

    async fn get_recent_jito_tips(&self, limit: i32) -> AppResult<Vec<rust_decimal::Decimal>> {
        unimplemented!("MockDb::get_recent_jito_tips not implemented")
    }

    async fn get_jito_tip_count(&self) -> AppResult<u32> {
        unimplemented!("MockDb::get_jito_tip_count not implemented")
    }

    async fn prune_old_jito_tips(&self) -> AppResult<u64> {
        unimplemented!("MockDb::prune_old_jito_tips not implemented")
    }

    async fn get_pnl_window(
        &self,
        from_hours: &str,
        to_hours: Option<&str>,
    ) -> AppResult<rust_decimal::Decimal> {
        unimplemented!("MockDb::get_pnl_window not implemented")
    }

    async fn get_pnl_24h(&self) -> AppResult<rust_decimal::Decimal> {
        unimplemented!("MockDb::get_pnl_24h not implemented")
    }

    async fn get_pnl_7d(&self) -> AppResult<rust_decimal::Decimal> {
        unimplemented!("MockDb::get_pnl_7d not implemented")
    }

    async fn get_pnl_30d(&self) -> AppResult<rust_decimal::Decimal> {
        unimplemented!("MockDb::get_pnl_30d not implemented")
    }

    async fn get_total_realized_pnl(&self) -> AppResult<rust_decimal::Decimal> {
        unimplemented!("MockDb::get_total_realized_pnl not implemented")
    }

    async fn record_portfolio_snapshot(
        &self,
        nav_sol: rust_decimal::Decimal,
        capital_sol: rust_decimal::Decimal,
        realized_pnl_sol: rust_decimal::Decimal,
        unrealized_pnl_sol: rust_decimal::Decimal,
        open_positions: i32,
        sol_price_usd: Option<rust_decimal::Decimal>,
        trade_mode: Option<String>,
    ) -> AppResult<()> {
        unimplemented!("MockDb::record_portfolio_snapshot not implemented")
    }

    async fn get_portfolio_nav_history(
        &self,
        days: u32,
    ) -> AppResult<Vec<crate::db_abstraction::types::PortfolioSnapshot>> {
        unimplemented!("MockDb::get_portfolio_nav_history not implemented")
    }

    async fn delete_portfolio_snapshots_before(&self, days: i32) -> AppResult<u64> {
        unimplemented!("MockDb::delete_portfolio_snapshots_before not implemented")
    }

    async fn get_capital_deployed_30d(&self) -> AppResult<rust_decimal::Decimal> {
        unimplemented!("MockDb::get_capital_deployed_30d not implemented")
    }

    async fn cancel_stale_trades(&self, max_age_minutes: i32) -> AppResult<u64> {
        unimplemented!("MockDb::cancel_stale_trades not implemented")
    }

    async fn get_strategy_performance(
        &self,
        strategy: &str,
        days: i32,
    ) -> AppResult<(f64, rust_decimal::Decimal, u32)> {
        unimplemented!("MockDb::get_strategy_performance not implemented")
    }

    async fn get_consecutive_losses(&self) -> AppResult<u32> {
        unimplemented!("MockDb::get_consecutive_losses not implemented")
    }

    async fn activate_trade_and_open_position(
        &self,
        trade_uuid: &str,
        wallet_address: &str,
        token_address: &str,
        token_symbol: Option<&str>,
        strategy: &str,
        amount_sol: rust_decimal::Decimal,
        entry_price: rust_decimal::Decimal,
        tx_signature: &str,
        max_heat_sol: Option<rust_decimal::Decimal>,
        entry_sol_price_usd: Option<rust_decimal::Decimal>,
    ) -> AppResult<()> {
        unimplemented!("MockDb::activate_trade_and_open_position not implemented")
    }

    async fn atomic_portfolio_heat_check_and_open_position(
        &self,
        trade_uuid: &str,
        wallet_address: &str,
        token_address: &str,
        token_symbol: Option<&str>,
        strategy: &str,
        amount_sol: rust_decimal::Decimal,
        entry_price: rust_decimal::Decimal,
        tx_signature: &str,
        max_heat_sol: Option<rust_decimal::Decimal>,
        entry_sol_price_usd: Option<rust_decimal::Decimal>,
    ) -> AppResult<()> {
        unimplemented!("MockDb::atomic_portfolio_heat_check_and_open_position not implemented")
    }

    async fn close_position_full(
        &self,
        trade_uuid: &str,
        wallet_address: &str,
        token_address: &str,
        exit_price: rust_decimal::Decimal,
        signature: &str,
        sol_price_usd: Option<rust_decimal::Decimal>,
        exit_fraction: rust_decimal::Decimal,
        confirmed: bool,
    ) -> AppResult<bool> {
        unimplemented!("MockDb::close_position_full not implemented")
    }

    async fn update_position_token_amount(
        &self,
        trade_uuid: &str,
        token_amount: u64,
    ) -> AppResult<()> {
        unimplemented!("MockDb::update_position_token_amount not implemented")
    }

    async fn revert_position_exit(&self, position_trade_uuid: &str) -> AppResult<()> {
        unimplemented!("MockDb::revert_position_exit not implemented")
    }

    async fn get_stuck_positions(&self, stuck_seconds: i64) -> AppResult<Vec<PositionRecord>> {
        unimplemented!("MockDb::get_stuck_positions not implemented")
    }

    async fn count_shadow_positions_by_token(&self, _token_address: &str) -> AppResult<i64> {
        Ok(0)
    }

    async fn update_position_state(&self, trade_uuid: &str, new_state: &str) -> AppResult<()> {
        unimplemented!("MockDb::update_position_state not implemented")
    }

    async fn update_position_unrealized_pnl(
        &self,
        trade_uuid: &str,
        current_price: rust_decimal::Decimal,
        pnl_sol: rust_decimal::Decimal,
        pnl_pct: rust_decimal::Decimal,
    ) -> AppResult<()> {
        unimplemented!("MockDb::update_position_unrealized_pnl not implemented")
    }

    async fn get_active_positions_with_entry(&self) -> AppResult<Vec<ActivePositionEntry>> {
        unimplemented!("MockDb::get_active_positions_with_entry not implemented")
    }

    async fn get_active_position_tokens(&self) -> AppResult<Vec<ActivePositionSummary>> {
        unimplemented!("MockDb::get_active_position_tokens not implemented")
    }

    async fn get_position_peak_price(&self, trade_uuid: &str) -> AppResult<Option<String>> {
        unimplemented!("MockDb::get_position_peak_price not implemented")
    }

    async fn upsert_wallet(
        &self,
        address: &str,
        wqs_score: Option<rust_decimal::Decimal>,
        roi_7d: Option<rust_decimal::Decimal>,
        roi_30d: Option<rust_decimal::Decimal>,
        trade_count_30d: Option<i32>,
        win_rate: Option<rust_decimal::Decimal>,
        max_drawdown_30d: Option<rust_decimal::Decimal>,
        avg_trade_size_sol: Option<rust_decimal::Decimal>,
        notes: Option<&str>,
    ) -> AppResult<bool> {
        unimplemented!("MockDb::upsert_wallet not implemented")
    }

    async fn update_wallet_status_ext(
        &self,
        address: &str,
        status: &str,
        ttl_hours: Option<i32>,
        reason: Option<&str>,
    ) -> AppResult<bool> {
        unimplemented!("MockDb::update_wallet_status_ext not implemented")
    }

    async fn get_expired_ttl_wallets(&self) -> AppResult<Vec<String>> {
        unimplemented!("MockDb::get_expired_ttl_wallets not implemented")
    }

    async fn demote_wallet(&self, address: &str, reason: &str) -> AppResult<()> {
        unimplemented!("MockDb::demote_wallet not implemented")
    }

    async fn has_recent_token_loss(
        &self,
        token_address: &str,
        within_minutes: i64,
    ) -> AppResult<bool> {
        unimplemented!("MockDb::has_recent_token_loss not implemented")
    }

    async fn get_wallet_monitoring(
        &self,
        wallet_address: &str,
    ) -> AppResult<Option<WalletMonitoring>> {
        if self.monitoring_error.load(Ordering::Relaxed) {
            return Err(AppError::Internal(
                "injected get_wallet_monitoring error".to_string(),
            ));
        }
        Ok(self
            .wallet_monitoring
            .lock()
            .unwrap()
            .get(wallet_address)
            .cloned())
    }

    async fn find_webhook_with_capacity(&self, max_wallets: i64) -> AppResult<Option<String>> {
        unimplemented!("MockDb::find_webhook_with_capacity not implemented")
    }

    async fn clear_webhook_id_for_webhook(&self, webhook_id: &str) -> AppResult<()> {
        unimplemented!("MockDb::clear_webhook_id_for_webhook not implemented")
    }

    async fn upsert_wallet_monitoring(
        &self,
        wallet_address: &str,
        helius_webhook_id: Option<&str>,
        monitoring_enabled: bool,
    ) -> AppResult<()> {
        unimplemented!("MockDb::upsert_wallet_monitoring not implemented")
    }

    async fn update_wallet_monitoring_signature(
        &self,
        wallet_address: &str,
        signature: &str,
    ) -> AppResult<()> {
        if self.signature_error.load(Ordering::Relaxed) {
            return Err(AppError::Internal(
                "injected signature update error".to_string(),
            ));
        }
        if let Some(wm) = self
            .wallet_monitoring
            .lock()
            .unwrap()
            .get_mut(wallet_address)
        {
            wm.last_transaction_signature = Some(signature.to_string());
        }
        Ok(())
    }

    async fn get_wallets_needing_webhook_registration(&self) -> AppResult<Vec<String>> {
        unimplemented!("MockDb::get_wallets_needing_webhook_registration not implemented")
    }

    async fn get_active_wallets_with_webhook_ids(&self) -> AppResult<Vec<(String, String)>> {
        unimplemented!("MockDb::get_active_wallets_with_webhook_ids not implemented")
    }

    async fn clear_webhook_id(&self, wallet_address: &str) -> AppResult<()> {
        unimplemented!("MockDb::clear_webhook_id not implemented")
    }

    async fn get_stale_webhook_wallets(&self, threshold_days: i32) -> AppResult<Vec<String>> {
        unimplemented!("MockDb::get_stale_webhook_wallets not implemented")
    }

    async fn get_all_wallet_monitoring(&self) -> AppResult<Vec<WalletMonitoring>> {
        if self.monitoring_all_error.load(Ordering::Relaxed) {
            return Err(AppError::Internal(
                "injected monitoring query error".to_string(),
            ));
        }
        Ok(self
            .wallet_monitoring
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect())
    }

    async fn update_webhook_health_status(
        &self,
        wallet_address: &str,
        health_status: &str,
        webhook_id: Option<&str>,
    ) -> AppResult<()> {
        unimplemented!("MockDb::update_webhook_health_status not implemented")
    }

    async fn update_webhook_status(
        &self,
        wallet_address: &str,
        webhook_status: &str,
    ) -> AppResult<()> {
        unimplemented!("MockDb::update_webhook_status not implemented")
    }

    async fn update_last_speculative_signal(
        &self,
        wallet_address: &str,
        timestamp: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<()> {
        if self.speculative_error.load(Ordering::Relaxed) {
            return Err(AppError::Internal(
                "injected speculative update error".to_string(),
            ));
        }
        self.last_speculative_timestamps
            .lock()
            .unwrap()
            .insert(wallet_address.to_string(), timestamp);
        Ok(())
    }

    async fn get_inactivity_demotion_count(&self, wallet_address: &str) -> AppResult<i32> {
        unimplemented!("MockDb::get_inactivity_demotion_count not implemented")
    }

    async fn increment_inactivity_demotion_count(&self, wallet_address: &str) -> AppResult<()> {
        unimplemented!("MockDb::increment_inactivity_demotion_count not implemented")
    }

    async fn reset_inactivity_demotion_count(&self, wallet_address: &str) -> AppResult<()> {
        unimplemented!("MockDb::reset_inactivity_demotion_count not implemented")
    }

    async fn log_webhook_lifecycle_event(
        &self,
        wallet_address: &str,
        action: &str,
        status: &str,
        webhook_id: Option<&str>,
        details: Option<&str>,
        error_message: Option<&str>,
        duration_ms: Option<i32>,
    ) -> AppResult<()> {
        unimplemented!("MockDb::log_webhook_lifecycle_event not implemented")
    }

    async fn increment_webhook_registration_attempts(
        &self,
        wallet_address: &str,
        error: Option<&str>,
    ) -> AppResult<()> {
        unimplemented!("MockDb::increment_webhook_registration_attempts not implemented")
    }

    async fn get_webhook_configuration(&self, key: &str) -> AppResult<Option<String>> {
        Ok(self.webhook_config.lock().unwrap().get(key).cloned())
    }

    async fn update_webhook_configuration(
        &self,
        key: &str,
        value: &str,
        _updated_by: &str,
    ) -> AppResult<()> {
        self.webhook_config
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    async fn get_orphaned_webhooks(&self, helius_webhook_ids: &[String]) -> AppResult<Vec<String>> {
        unimplemented!("MockDb::get_orphaned_webhooks not implemented")
    }

    async fn upsert_exit_target(
        &self,
        trade_uuid: &str,
        entry_price: rust_decimal::Decimal,
        entry_amount_sol: rust_decimal::Decimal,
        peak_price: rust_decimal::Decimal,
        peak_profit_percent: rust_decimal::Decimal,
        targets_hit_json: &str,
        trailing_stop_active: bool,
        trailing_stop_price: rust_decimal::Decimal,
        remaining_fraction: rust_decimal::Decimal,
    ) -> AppResult<()> {
        self.exit_target_upserts
            .lock()
            .unwrap()
            .push(trade_uuid.to_string());
        self.exit_targets.lock().unwrap().insert(
            trade_uuid.to_string(),
            ExitTargetData {
                entry_price,
                entry_amount_sol,
                peak_price,
                peak_profit_percent,
                targets_hit: targets_hit_json.to_string(),
                trailing_stop_active,
                trailing_stop_price,
                remaining_fraction,
            },
        );
        Ok(())
    }

    async fn load_exit_target(&self, trade_uuid: &str) -> AppResult<Option<ExitTargetData>> {
        if self.exit_target_error.load(Ordering::Relaxed) {
            return Err(AppError::Internal("injected exit target error".to_string()));
        }
        Ok(self.exit_targets.lock().unwrap().get(trade_uuid).cloned())
    }

    async fn delete_exit_target(&self, trade_uuid: &str) -> AppResult<()> {
        if self.exit_target_delete_error.load(Ordering::Relaxed) {
            return Err(AppError::Internal(
                "injected exit target delete error".to_string(),
            ));
        }
        self.exit_target_deletes
            .lock()
            .unwrap()
            .push(trade_uuid.to_string());
        self.exit_targets.lock().unwrap().remove(trade_uuid);
        Ok(())
    }

    async fn insert_reconciliation_log(
        &self,
        trade_uuid: &str,
        expected_state: &str,
        actual_on_chain: Option<&str>,
        discrepancy: &str,
        on_chain_tx_signature: Option<&str>,
        notes: Option<&str>,
    ) -> AppResult<i64> {
        unimplemented!("MockDb::insert_reconciliation_log not implemented")
    }

    async fn get_reconciliation_status(
        &self,
        discrepancies_limit: i32,
    ) -> AppResult<ReconciliationStatus> {
        unimplemented!("MockDb::get_reconciliation_status not implemented")
    }

    async fn get_reconciliation_history(&self, limit: i32) -> AppResult<Vec<ReconciliationRun>> {
        unimplemented!("MockDb::get_reconciliation_history not implemented")
    }

    async fn count_reconciliation_runs(&self) -> AppResult<i64> {
        unimplemented!("MockDb::count_reconciliation_runs not implemented")
    }

    async fn get_reconciliation_stats(&self, time_range: &str) -> AppResult<ReconciliationStats> {
        unimplemented!("MockDb::get_reconciliation_stats not implemented")
    }

    async fn resolve_discrepancy(
        &self,
        id: i64,
        resolved_by: &str,
        resolution: &str,
    ) -> AppResult<()> {
        unimplemented!("MockDb::resolve_discrepancy not implemented")
    }

    async fn get_trades_filtered(
        &self,
        from_date: Option<&str>,
        to_date: Option<&str>,
        status_filter: Option<&str>,
        strategy_filter: Option<&str>,
        wallet_address_filter: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> AppResult<Vec<TradeDetail>> {
        unimplemented!("MockDb::get_trades_filtered not implemented")
    }

    async fn count_trades_filtered(
        &self,
        from_date: Option<&str>,
        to_date: Option<&str>,
        status_filter: Option<&str>,
        strategy_filter: Option<&str>,
        wallet_address_filter: Option<&str>,
    ) -> AppResult<i64> {
        unimplemented!("MockDb::count_trades_filtered not implemented")
    }

    async fn update_trade_costs(
        &self,
        trade_uuid: &str,
        jito_tip_sol: rust_decimal::Decimal,
        dex_fee_sol: rust_decimal::Decimal,
        slippage_cost_sol: rust_decimal::Decimal,
    ) -> AppResult<()> {
        unimplemented!("MockDb::update_trade_costs not implemented")
    }

    async fn mark_trade_dead_letter(
        &self,
        trade_uuid: &str,
        payload: &str,
        error: &str,
    ) -> AppResult<()> {
        unimplemented!("MockDb::mark_trade_dead_letter not implemented")
    }

    async fn log_config_change(
        &self,
        _key: &str,
        _old_value: Option<&str>,
        _new_value: &str,
        _changed_by: &str,
        _reason: Option<&str>,
    ) -> AppResult<()> {
        Ok(())
    }

    async fn get_dead_letter_entries(
        &self,
        limit: i32,
        offset: i32,
    ) -> AppResult<Vec<DeadLetterItem>> {
        unimplemented!("MockDb::get_dead_letter_entries not implemented")
    }

    async fn get_dead_letter_entry(&self, trade_uuid: &str) -> AppResult<Option<DeadLetterItem>> {
        unimplemented!("MockDb::get_dead_letter_entry not implemented")
    }

    async fn count_dead_letter_entries(&self) -> AppResult<i64> {
        unimplemented!("MockDb::count_dead_letter_entries not implemented")
    }

    async fn get_retryable_dlq_items(&self, limit: i64) -> AppResult<Vec<RetryableDlqItem>> {
        unimplemented!("MockDb::get_retryable_dlq_items not implemented")
    }

    async fn update_dlq_item(
        &self,
        trade_uuid: &str,
        retry_count: i64,
        can_retry: bool,
        mark_processed: bool,
    ) -> AppResult<()> {
        unimplemented!("MockDb::update_dlq_item not implemented")
    }

    async fn update_dlq_items_batch(&self, items: Vec<UpdateDlqItemParams>) -> AppResult<usize> {
        unimplemented!("MockDb::update_dlq_items_batch not implemented")
    }

    async fn get_config_audit_entries(
        &self,
        limit: i32,
        offset: i32,
    ) -> AppResult<Vec<ConfigAuditItem>> {
        unimplemented!("MockDb::get_config_audit_entries not implemented")
    }

    async fn get_config_audit_entries_by_key_prefix(
        &self,
        prefix: &str,
        limit: i32,
    ) -> AppResult<Vec<ConfigAuditItem>> {
        unimplemented!("MockDb::get_config_audit_entries_by_key_prefix not implemented")
    }

    async fn count_config_audit_entries(&self) -> AppResult<i64> {
        unimplemented!("MockDb::count_config_audit_entries not implemented")
    }

    async fn get_webhook_audit_log(
        &self,
        wallet_address: Option<&str>,
        action: Option<&str>,
        status: Option<&str>,
        limit: Option<i64>,
    ) -> AppResult<Vec<WebhookAuditLog>> {
        unimplemented!("MockDb::get_webhook_audit_log not implemented")
    }

    async fn count_trades_by_status(&self, status: &str) -> AppResult<i64> {
        unimplemented!("MockDb::count_trades_by_status not implemented")
    }

    async fn get_closed_trade_count_for_wallet(&self, wallet_address: &str) -> AppResult<i64> {
        unimplemented!("MockDb::get_closed_trade_count_for_wallet not implemented")
    }

    async fn get_wallet_copy_stats(
        &self,
        wallet_address: &str,
    ) -> AppResult<(i64, rust_decimal::Decimal)> {
        unimplemented!("MockDb::get_wallet_copy_stats not implemented")
    }

    async fn get_token_mirror_avg_pnl(
        &self,
        token_address: &str,
        window_hours: i32,
        min_samples: i32,
    ) -> AppResult<Option<rust_decimal::Decimal>> {
        unimplemented!("MockDb::get_token_mirror_avg_pnl not implemented")
    }

    async fn get_wallet_pnl_statistics(
        &self,
        wallet_address: &str,
        window_days: i32,
    ) -> AppResult<Option<(i64, rust_decimal::Decimal, rust_decimal::Decimal)>> {
        unimplemented!("MockDb::get_wallet_pnl_statistics not implemented")
    }

    async fn get_wallet_realized_pnl_window(
        &self,
        _wallet_address: &str,
        _window_hours: i32,
    ) -> AppResult<Option<rust_decimal::Decimal>> {
        unimplemented!("MockDb::get_wallet_realized_pnl_window not implemented")
    }

    async fn get_wallet_shadow_kelly_stats(
        &self,
        _wallet_address: &str,
        _window_days: i32,
    ) -> AppResult<Option<ShadowKellyStats>> {
        Ok(None)
    }

    async fn get_wallet_shadow_recent_net(
        &self,
        _wallet_address: &str,
        _window_hours: i32,
    ) -> AppResult<Option<Decimal>> {
        Ok(None)
    }

    async fn get_wallet_copy_performance(
        &self,
        wallet_address: &str,
    ) -> AppResult<Option<WalletCopyPerformance>> {
        unimplemented!("MockDb::get_wallet_copy_performance not implemented")
    }

    async fn get_trade_latency_stats(&self, hours: i32) -> AppResult<TradeLatencyStats> {
        unimplemented!("MockDb::get_trade_latency_stats not implemented")
    }

    async fn get_trade_latency_histogram(
        &self,
        hours: i32,
        bucket_bounds: &[f64],
    ) -> AppResult<Vec<LatencyBucket>> {
        unimplemented!("MockDb::get_trade_latency_histogram not implemented")
    }

    async fn get_positions(&self, state_filter: Option<&str>) -> AppResult<Vec<PositionDetail>> {
        unimplemented!("MockDb::get_positions not implemented")
    }

    async fn get_whale_buy_prices(
        &self,
        wallet_address: &str,
        token_address: &str,
        window_hours: i64,
    ) -> AppResult<Vec<rust_decimal::Decimal>> {
        unimplemented!("MockDb::get_whale_buy_prices not implemented")
    }

    async fn has_recent_net_loss(
        &self,
        token_address: &str,
        window_hours: i64,
        loss_threshold_pct: rust_decimal::Decimal,
    ) -> AppResult<bool> {
        unimplemented!("MockDb::has_recent_net_loss not implemented")
    }

    async fn get_wallets(&self, status_filter: Option<&str>) -> AppResult<Vec<WalletDetail>> {
        unimplemented!("MockDb::get_wallets not implemented")
    }

    async fn insert_trade_and_create_position(
        &self,
        trade: &InsertTrade,
        position: &InsertPosition,
    ) -> AppResult<i64> {
        unimplemented!("MockDb::insert_trade_and_create_position not implemented")
    }

    async fn update_trade_status_and_position(
        &self,
        trade_uuid: &str,
        trade_status: &str,
        position_state: Option<&str>,
    ) -> AppResult<()> {
        unimplemented!("MockDb::update_trade_status_and_position not implemented")
    }

    async fn get_evaluation_data(
        &self,
    ) -> AppResult<(
        rust_decimal::Decimal,
        rust_decimal::Decimal,
        rust_decimal::Decimal,
        rust_decimal::Decimal,
    )> {
        if self.evaluation_error.load(Ordering::Relaxed) {
            return Err(AppError::Internal("injected evaluation error".to_string()));
        }
        Ok(self.evaluation_data.lock().unwrap().unwrap_or((
            rust_decimal::Decimal::ZERO,
            rust_decimal::Decimal::ZERO,
            rust_decimal::Decimal::ZERO,
            rust_decimal::Decimal::ZERO,
        )))
    }

    async fn get_consecutive_losses_since(
        &self,
        _since: Option<chrono::DateTime<chrono::Utc>>,
    ) -> AppResult<u32> {
        if self.consecutive_error.load(Ordering::Relaxed) {
            return Err(AppError::Internal("injected consecutive error".to_string()));
        }
        Ok(self.consecutive_losses.lock().unwrap().unwrap_or(0))
    }

    async fn get_max_drawdown_percent(
        &self,
        _total_capital_sol: rust_decimal::Decimal,
    ) -> AppResult<(rust_decimal::Decimal, rust_decimal::Decimal)> {
        if self.drawdown_error.load(Ordering::Relaxed) {
            return Err(AppError::Internal("injected drawdown error".to_string()));
        }
        Ok(self
            .drawdown
            .lock()
            .unwrap()
            .unwrap_or((rust_decimal::Decimal::ZERO, rust_decimal::Decimal::ZERO)))
    }
}
