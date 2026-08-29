//! Integration tests module
//!
//! This file is the entry point for all integration tests. Cargo only
//! auto-discovers files directly under `tests/`; files in the `integration/`
//! subdirectory are NEVER compiled unless they are explicitly registered here
//! via `mod` + `#[path]`. Every new test file added to `tests/integration/`
//! MUST be registered in this module list or its tests are silently skipped.

mod common;

#[path = "integration/api_tests.rs"]
mod api_tests;

#[path = "integration/reconciliation_tests.rs"]
mod reconciliation_tests;

#[path = "integration/auth_tests.rs"]
mod auth_tests;

#[path = "integration/db_tests.rs"]
mod db_tests;

#[path = "integration/webhook_flow_tests.rs"]
mod webhook_flow_tests;

#[path = "integration/tiered_polling_integration_tests.rs"]
mod tiered_polling_integration_tests;

#[path = "integration/transaction_builder_tests.rs"]
mod transaction_builder_tests;

#[path = "integration/token_safety_tests.rs"]
mod token_safety_tests;

#[path = "integration/roster_merge_tests.rs"]
mod roster_merge_tests;

#[path = "integration/consensus_detection_tests.rs"]
mod consensus_detection_tests;

#[path = "integration/helius_token_age_tests.rs"]
mod helius_token_age_tests;

#[path = "integration/volatility_tests.rs"]
mod volatility_tests;

#[path = "integration/dex_comparison_tests.rs"]
mod dex_comparison_tests;

#[path = "integration/safety_validation_tests.rs"]
mod safety_validation_tests;

// ── Financial-loss & missed-profit integration tests ──────────────────────────

#[path = "integration/position_lifecycle_tests.rs"]
mod position_lifecycle_tests;

// ── Execution correctness & capital-protection proof tests ─────────────────────

#[path = "integration/execution_proof_tests.rs"]
mod execution_proof_tests;

// ── A1/A2 accounting characterization tests ────────────────────────────────────

#[path = "integration/accounting_characterization_tests.rs"]
mod accounting_characterization_tests;

#[path = "integration/selection_service_tests.rs"]
mod selection_service_tests;

#[path = "integration/smart_money_cluster_tests.rs"]
mod smart_money_cluster_tests;

#[path = "integration/dune_bootstrap_tests.rs"]
mod dune_bootstrap_tests;

#[path = "integration/parallel_execution_test.rs"]
mod parallel_execution_test;

// ── Jupiter Integration Tests ─────────────────────────────────────────────────────

#[path = "integration/jupiter_v2_integration_tests.rs"]
mod jupiter_v2_integration_tests;

#[path = "integration/jito_integration_tests.rs"]
mod jito_integration_tests;

#[path = "integration/friction_gating_tests.rs"]
mod friction_gating_tests;

// ── DLQ terminal risk-gate rejections (2026-08-23) ────────────────────────────

#[path = "integration/dlq_terminal_rejection_tests.rs"]
mod dlq_terminal_rejection_tests;

// ── Profitability verdict gate tests (Phase C4 go/no-go) ──────────────────────

#[path = "integration/profitability_verdict_tests.rs"]
mod profitability_verdict_tests;

// ── Dormancy-demotion promotion grace (2026-08-29) ──────────────────────────

#[path = "integration/dormancy_grace_tests.rs"]
mod dormancy_grace_tests;
