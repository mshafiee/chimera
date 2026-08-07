//! Engine services with infrastructure dependencies (moved from the operator
//! crate 2026-08-07). These implement trading logic over the infra adapters
//! (db_abstraction) and core domain types.

pub mod kelly_sizer;
pub mod momentum_exit;
pub mod portfolio_heat;
pub mod tips;

pub use kelly_sizer::{KellyResult, KellySizer};
pub use momentum_exit::{MomentumExit, MomentumExitAction};
pub use portfolio_heat::{HeatResult, PortfolioHeat};
pub use tips::TipManager;
