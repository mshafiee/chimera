//! Chimera Core — domain-pure foundation crate.
//!
//! Architectural law (see docs/architecture/rust-workspace.md):
//! this crate must NEVER import database drivers (sqlx, diesel) or web
//! frameworks (axum, actix). It hosts domain constants and framework-agnostic
//! utilities. The `operator` crate re-exports these modules so the legacy
//! `chimera_operator::*` paths keep working during incremental extraction.

pub mod config;
pub mod constants;
pub mod error;
pub mod retry;
pub mod utils;
