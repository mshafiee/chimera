//! Integration tests module
//!
//! This file serves as the entry point for all integration tests.
//! Rust's test runner will discover this file and run the tests
//! in the integration subdirectory.

#[path = "integration/api_tests.rs"]
mod api_tests;

#[path = "reconciliation_tests.rs"]
mod reconciliation_tests;

#[path = "integration/auth_tests.rs"]
mod auth_tests;

#[path = "integration/db_tests.rs"]
mod db_tests;

#[path = "integration/webhook_flow_tests.rs"]
mod webhook_flow_tests;

#[path = "integration/transaction_builder_tests.rs"]
mod transaction_builder_tests;

#[path = "integration/token_safety_tests.rs"]
mod token_safety_tests;

#[path = "integration/roster_merge_tests.rs"]
mod roster_merge_tests;

