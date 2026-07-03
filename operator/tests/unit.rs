//! Unit tests module
//!
//! This file serves as the entry point for all unit tests.
//! Tests individual components in isolation.

#[path = "unit/circuit_breaker_tests.rs"]
mod circuit_breaker_tests;

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

#[path = "unit/db_integrity_tests.rs"]
mod db_integrity_tests;

#[path = "unit/position_sizer_tests.rs"]
mod position_sizer_tests;

#[path = "unit/circuit_breaker_real_tests.rs"]
mod circuit_breaker_real_tests;

// ── Fix-verification tests: assert CORRECT behavior (fail until bugs are fixed) ──

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
