//! Pump.fun bonding curve state — liquidity velocity and phase detection.
//!
//! Research (arxiv 2602.14860, 655,770 tokens): **liquidity velocity is the
//! single most informative predictor of token success** — tokens reaching a
//! bonding-curve SOL level with FEWER trades (fast accumulation) have
//! substantially higher graduation probability. Slow, fragmented accumulation
//! "typically signals weak collective engagement and frequently precedes
//! stagnation."
//!
//! Also implements bonding-curve phase detection: the late-curve window
//! (approaching the 85-SOL graduation threshold) is a **dump zone** — there
//! is a proven depth discontinuity at graduation where selling before it is
//! always more profitable (virtual reserves evaporate).

use anyhow::{Context, Result};
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

/// Pump.fun bonding curve program ID (mainnet).
pub const PUMPFUN_PROGRAM_ID: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

/// SOL (lamports) at which the bonding curve graduates to a real AMM.
/// The paper: cumulative real SOL hits 85 (115 total incl. 30 virtual),
/// market cap ≈ $69,000.
pub const GRADUATION_SOL_LAMPORTS: u64 = 85_000_000_000;

/// Bonding curve phase classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BondingCurvePhase {
    /// < 60% of graduation SOL — the fast-accumulation entry window.
    Early,
    /// 60-85% — mid-curve; entry still possible but momentum must be strong.
    Mid,
    /// > 85% — the pre-graduation dump zone (depth discontinuity). EXIT.
    Late,
    /// Curve complete — token has graduated to a real AMM.
    Graduated,
}

/// Parsed pump.fun bonding curve account state.
#[derive(Debug, Clone)]
pub struct BondingCurveState {
    pub virtual_token_reserves: u64,
    pub virtual_sol_reserves: u64,
    pub real_token_reserves: u64,
    pub real_sol_reserves: u64,
    pub token_total_supply: u64,
    pub complete: bool,
}

impl BondingCurveState {
    /// Parse from the raw account data (pump.fun `BondingCurve` layout):
    /// 8-byte anchor discriminator, then six u64 LE fields, then a bool.
    pub fn from_account_data(data: &[u8]) -> Option<Self> {
        if data.len() < 49 {
            return None;
        }
        let read_u64 = |off: usize| -> u64 {
            u64::from_le_bytes(data[off..off + 8].try_into().unwrap_or([0u8; 8]))
        };
        Some(Self {
            virtual_token_reserves: read_u64(8),
            virtual_sol_reserves: read_u64(16),
            real_token_reserves: read_u64(24),
            real_sol_reserves: read_u64(32),
            token_total_supply: read_u64(40),
            complete: data[48] != 0,
        })
    }

    /// Fraction of the bonding curve complete (0.0 = launch, 1.0 = graduation).
    pub fn completion_pct(&self) -> f64 {
        (self.real_sol_reserves as f64 / GRADUATION_SOL_LAMPORTS as f64).clamp(0.0, 1.0)
    }

    /// SOL accumulated per trade — HIGH = fast accumulation = conviction.
    /// Requires the swap count (from getSignaturesForAddress on the curve
    /// account, which every swap touches).
    pub fn liquidity_velocity(&self, swap_count: u64) -> f64 {
        if swap_count == 0 {
            return 0.0;
        }
        self.real_sol_reserves as f64 / swap_count as f64 / 1e9
    }

    /// Curve phase: the late-curve window is the dump zone.
    pub fn phase(&self) -> BondingCurvePhase {
        if self.complete {
            BondingCurvePhase::Graduated
        } else {
            let pct = self.completion_pct();
            if pct > 0.85 {
                BondingCurvePhase::Late
            } else if pct > 0.60 {
                BondingCurvePhase::Mid
            } else {
                BondingCurvePhase::Early
            }
        }
    }
}

/// Derive the pump.fun bonding curve PDA for a mint.
pub fn bonding_curve_pda(mint: &str) -> Result<Pubkey> {
    let mint_pk = Pubkey::from_str(mint).context("invalid mint address")?;
    let (pda, _bump) = Pubkey::find_program_address(
        &[b"bonding-curve", mint_pk.as_ref()],
        &Pubkey::from_str(PUMPFUN_PROGRAM_ID).context("invalid pump program id")?,
    );
    Ok(pda)
}

/// Fetch and parse a token's bonding curve state from the chain.
/// Returns `Ok(None)` when the account doesn't exist (non-pump token or the
/// curve was closed after graduation).
pub fn fetch_bonding_curve(
    rpc: &RpcClient,
    mint: &str,
) -> Result<Option<BondingCurveState>> {
    let curve_pk = bonding_curve_pda(mint)?;
    let account = rpc
        .get_account(&curve_pk)
        .with_context(|| format!("fetch bonding curve account for {mint}"))?;
    Ok(BondingCurveState::from_account_data(&account.data))
}

/// Number of swaps on the curve = signatures touching the curve account
/// (each pump.fun swap updates the curve). Capped at `limit`.
pub fn fetch_swap_count(rpc: &RpcClient, mint: &str, limit: usize) -> Result<u64> {
    let curve_pk = bonding_curve_pda(mint)?;
    let sigs = rpc
        .get_signatures_for_address_with_config(
            &curve_pk,
            GetConfirmedSignaturesForAddress2Config {
                limit: Some(limit),
                ..Default::default()
            },
        )
        .with_context(|| format!("fetch curve signatures for {mint}"))?;
    Ok(sigs.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_curve_data(real_sol: u64, complete: bool) -> Vec<u8> {
        let mut data = vec![0u8; 49];
        // discriminator (8 bytes) left zeroed — parser doesn't require it
        data[8..16].copy_from_slice(&1_000_000_000_000u64.to_le_bytes()); // virtual tokens
        data[16..24].copy_from_slice(&30_000_000_000u64.to_le_bytes()); // virtual SOL (30)
        data[24..32].copy_from_slice(&500_000_000_000u64.to_le_bytes()); // real tokens
        data[32..40].copy_from_slice(&real_sol.to_le_bytes()); // real SOL
        data[40..48].copy_from_slice(&1_000_000_000_000u64.to_le_bytes()); // total supply
        data[48] = complete as u8;
        data
    }

    #[test]
    fn parses_curve_and_computes_velocity() {
        let state = BondingCurveState::from_account_data(&sample_curve_data(25_000_000_000, false))
            .unwrap();
        assert_eq!(state.real_sol_reserves, 25_000_000_000);
        assert!((state.completion_pct() - 0.294).abs() < 0.01);
        assert_eq!(state.phase(), BondingCurvePhase::Early);
        // 25 SOL over 50 swaps = 0.5 SOL/trade
        assert!((state.liquidity_velocity(50) - 0.5).abs() < 0.001);
    }

    #[test]
    fn late_curve_is_dump_zone() {
        let state = BondingCurveState::from_account_data(&sample_curve_data(78_000_000_000, false))
            .unwrap();
        assert_eq!(state.phase(), BondingCurvePhase::Late);
        assert!(state.completion_pct() > 0.85);
    }

    #[test]
    fn mid_and_graduated_phases() {
        let mid = BondingCurveState::from_account_data(&sample_curve_data(60_000_000_000, false))
            .unwrap();
        assert_eq!(mid.phase(), BondingCurvePhase::Mid);
        let grad = BondingCurveState::from_account_data(&sample_curve_data(85_000_000_000, true))
            .unwrap();
        assert_eq!(grad.phase(), BondingCurvePhase::Graduated);
    }

    #[test]
    fn rejects_short_data() {
        assert!(BondingCurveState::from_account_data(&[0u8; 48]).is_none());
    }

    #[test]
    fn zero_swaps_yields_zero_velocity() {
        let state = BondingCurveState::from_account_data(&sample_curve_data(10_000_000_000, false))
            .unwrap();
        assert_eq!(state.liquidity_velocity(0), 0.0);
    }

    #[test]
    fn pda_is_deterministic_and_program_scoped() {
        let mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
        let pda = bonding_curve_pda(mint).unwrap();
        // Same mint always yields the same PDA.
        assert_eq!(pda, bonding_curve_pda(mint).unwrap());
        // Different mints yield different PDAs.
        let other = bonding_curve_pda("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB").unwrap();
        assert_ne!(pda, other);
    }

    #[test]
    fn pda_rejects_invalid_mint() {
        assert!(bonding_curve_pda("not-a-valid-base58-address").is_err());
    }

    #[test]
    fn completion_pct_clamps_out_of_range() {
        // Above graduation clamps to 1.0, zero clamps to 0.0.
        let over = BondingCurveState::from_account_data(&sample_curve_data(200_000_000_000, false))
            .unwrap();
        assert_eq!(over.completion_pct(), 1.0);
        let zero = BondingCurveState::from_account_data(&sample_curve_data(0, false)).unwrap();
        assert_eq!(zero.completion_pct(), 0.0);
    }
}
