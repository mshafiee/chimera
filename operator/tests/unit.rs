//! Unit tests module
//!
//! This file serves as the entry point for all unit tests.
//! Tests individual components in isolation.

#[path = "unit/circuit_breaker_tests.rs"]
mod circuit_breaker_tests;

#[path = "unit/jito_tests.rs"]
mod jito_tests;

#[path = "unit/state_machine_tests.rs"]
mod state_machine_tests;

#[path = "unit/token_parser_tests.rs"]
mod token_parser_tests;

#[path = "unit/tip_manager_tests.rs"]
mod tip_manager_tests;

#[path = "unit/recovery_tests.rs"]
mod recovery_tests;

#[path = "unit/signal_quality_tests.rs"]
mod signal_quality_tests;

#[path = "unit/momentum_exit_tests.rs"]
mod momentum_exit_tests;

#[path = "unit/kelly_sizer_tests.rs"]
mod kelly_sizer_tests;

// ── Financial-loss & missed-profit test suite ─────────────────────────────────

#[path = "unit/stop_loss_tests.rs"]
mod stop_loss_tests;

#[path = "unit/profit_target_tests.rs"]
mod profit_target_tests;

#[path = "unit/decision_recorder_tests.rs"]
mod decision_recorder_tests;

#[path = "unit/dune_monitor_tests.rs"]
mod dune_monitor_tests;

#[path = "unit/engine_handle_tests.rs"]
mod engine_handle_tests;

#[path = "unit/rent_scavenger_tests.rs"]
mod rent_scavenger_tests;

#[path = "unit/selection_coverage_tests.rs"]
mod selection_coverage_tests;

#[path = "unit/shadow_fill_tests.rs"]
mod shadow_fill_tests;

#[path = "unit/shadow_trader_tests.rs"]
mod shadow_trader_tests;


#[path = "unit/db_integrity_tests.rs"]
mod db_integrity_tests;

#[path = "unit/position_sizer_tests.rs"]
mod position_sizer_tests;

#[path = "unit/circuit_breaker_real_tests.rs"]
mod circuit_breaker_real_tests;

// ── Fix-verification tests: assert CORRECT (post-fix) behavior; a failure
//    here means a previously-fixed bug (F3/F7 hard-stop sign, F4 trailing-stop
//    ratchet, F6 silent status update) has regressed ──────────────────────────

#[path = "unit/fix_verification_tests.rs"]
mod fix_verification_tests;

#[path = "unit/v0_reconstruction_tests.rs"]
mod v0_reconstruction_tests;

#[path = "unit/signal_pipeline_tests.rs"]
mod signal_pipeline_tests;

#[path = "unit/trade_mode_tests.rs"]
mod trade_mode_tests;

// ── Jupiter Error Handling Unit Tests ───────────────────────────────────────────

#[path = "unit/jupiter_error_handling_tests.rs"]
mod jupiter_error_handling_tests;

// ── Tiered Polling Unit Tests ─────────────────────────────────────────────────────

#[path = "unit/tiered_polling_tests.rs"]
mod tiered_polling_tests;

#[path = "unit/helius_rpc_verify_tests.rs"]
mod helius_rpc_verify_tests;

#[path = "unit/webhook_restoration_tests.rs"]
mod webhook_restoration_tests;

// ── HTTP handler test suites (real router + real test DB) ──────────────────

#[path = "unit/handlers_api_tests.rs"]
mod handlers_api_tests;

#[path = "unit/handlers_risk_tests.rs"]
mod handlers_risk_tests;

#[path = "unit/handlers_scout_tests.rs"]
mod handlers_scout_tests;

#[path = "unit/handlers_signals_tests.rs"]
mod handlers_signals_tests;

#[path = "unit/handlers_operations_tests.rs"]
mod handlers_operations_tests;

#[path = "unit/handlers_webhook_lifecycle_tests.rs"]
mod handlers_webhook_lifecycle_tests;

#[path = "unit/handlers_monitoring_tests.rs"]
mod handlers_monitoring_tests;
