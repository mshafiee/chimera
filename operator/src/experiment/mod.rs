//! Forward test experiment module
//!
//! Implements live tracer trades, control arms, and verdict evaluation
//! for the 21-day profitability forward test.

pub mod controls;
pub mod tracer;
pub mod ledger;
pub mod verdict;
pub mod toxic;

pub use controls::{ControlArms, ControlTrade};
pub use tracer::TracerHook;
pub use ledger::ExperimentLedger;
pub use verdict::VerdictEvaluator;
pub use toxic::{ToxicFlowDetector, ToxicReason, ToxicStatistics};
