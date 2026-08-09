//! Forward test experiment module
//!
//! Implements live tracer trades, control arms, and verdict evaluation
//! for the 21-day profitability forward test.

pub mod controls;
pub mod ledger;
pub mod toxic;
pub mod tracer;
pub mod verdict;

pub use controls::{ControlArms, ControlTrade};
pub use ledger::ExperimentLedger;
pub use toxic::{ToxicFlowDetector, ToxicReason, ToxicStatistics};
pub use tracer::TracerHook;
pub use verdict::VerdictEvaluator;
