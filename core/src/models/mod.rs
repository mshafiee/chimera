//! Data models for Chimera Operator

mod signal;
mod trade;

pub use signal::*;
pub use trade::*;


// ── Strategy slippage bounds (moved from engine/slippage.rs 2026-08-07) ────
// Domain logic on the Strategy entity: strategy-specific Jupiter tolerance
// bounds. Kept with the entity so core owns its behavior (orphan rule).

/// Strategy-specific Jupiter tolerance bounds.
#[derive(Debug, Clone, Copy)]
pub struct SlippageBounds {
    pub floor_bps: u16,
    pub ceil_bps: u16,
}

impl Strategy {
    /// Strategy-specific Jupiter tolerance bounds.
    pub fn slippage_bounds(self) -> SlippageBounds {
        match self {
            // Tight: capital-preservation strategy, reject high-impact entries.
            Strategy::Shield => SlippageBounds {
                floor_bps: 10,
                ceil_bps: 100,
            },
            // Wider: speculative entries on thinner books.
            Strategy::Spear => SlippageBounds {
                floor_bps: 30,
                ceil_bps: 300,
            },
            // Generous: exits must fill even under stress.
            Strategy::Exit => SlippageBounds {
                floor_bps: 50,
                ceil_bps: 1500,
            },
        }
    }
}
