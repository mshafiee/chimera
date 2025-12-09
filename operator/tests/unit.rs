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

