//! Pure engine domain services (moved from the operator crate 2026-08-07).
//!
//! These modules reference only core domain types (config, constants, error,
//! models, price_cache, jupiter helpers). They have no database drivers and
//! no web frameworks. Modules that depend on infra (db, token, helius) or on
//! the operator-only cluster (executor, selection, monitoring) live in
//! `chimera_infra::engine` or remain in the operator crate.

pub mod channel;
pub mod degradation;
pub mod dex_comparator;
pub mod market_regime;
pub mod mev_protection;
pub mod rejection_mute;
pub mod rpc_cache;
pub mod run_context;
pub mod signal_quality;
pub mod slippage;
pub mod tip_inlining;
pub mod v0_reconstruction;
pub mod volume_cache;

pub use channel::*;
pub use degradation::*;
pub use dex_comparator::{DexComparator, RouteSelection};
pub use market_regime::{MarketRegime, MarketRegimeDetector};
pub use mev_protection::MevProtection;
pub use rejection_mute::*;
pub use rpc_cache::{CacheStats, RpcCache};
pub use run_context::RunContext;
pub use signal_quality::{QualityCategory, SignalFactors, SignalQuality};
pub use slippage::*;
pub use tip_inlining::*;
pub use v0_reconstruction::*;
pub use volume_cache::VolumeCache;
