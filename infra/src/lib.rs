#![allow(clippy::too_many_arguments)]

//! Chimera Infra — adapters & concrete implementations.
//!
//! Implements the repository traits and domain services defined by
//! `chimera_core`. The dependency direction is strictly one-way:
//! `infra → core`, never the reverse. The `operator` crate re-exports these
//! modules so the legacy `chimera_operator::*` paths keep working during
//! incremental extraction.

pub mod db_abstraction;
