//! Unified BUY/SELL decision engine (Phase B1).
//!
//! Both ingress paths — the direct webhook handler (`handlers/webhook.rs`) and
//! the production Helius monitoring path (`handlers/monitoring.rs`) — route
//! every signal through a single `decide` function. This eliminates the
//! historical divergence where the Helius path clamped the copied wallet's
//! amount instead of using `PositionSizer`, skipped quality/consensus/regime
//! scoring, and used a different set of admission checks.
//!
//! ## Decision pipeline order (BUY)
//! 1. Wallet fetch + ACTIVE status gate
//! 2. Hard WQS gate (<70 drop; ≥80 → SHIELD; 70–79 → SPEAR)
//! 3. Non-speculative / pump.fun bonding-curve skip
//! 4. Token fast_check (freeze/mint authority, honeypot)
//! 5. Token-age enforcement
//! 6. Liquidity floor (strategy-specific) + 24h volume (telemetry until B3)
//! 7. Consensus detection (SignalAggregator)
//!    7e. Stop-loss re-entry cooldown (block re-buying tokens we just lost on)
//!    7f. Whale averaging-down gate (block copying a falling-knife buyer)
//!    7g. Pump-chase gate (block buying the top of a fresh pump)
//! 8. Signal-quality score
//! 9. Market-regime multiplier
//! 10. `PositionSizer` size (Kelly + confidence)
//! 11. Portfolio heat + strategy-allocation heat admission
//!
//! ## SELL
//! Exit signals only verify wallet status and that an active position exists
//! to close. Sizing is always the full remaining position (handled downstream);
//! quality/heat gates do not apply to exits so protective sells always proceed.
//!
//! Every decision carries a `decision_id` and its full input set so it can be
//! logged and, in Phase C, persisted as an immutable run-scoped record.

use std::sync::Arc;

use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use rust_decimal::Decimal;

use crate::db_abstraction::Database;
use crate::engine::position_sizer::{PositionSizer, SizingFactors};
use crate::engine::{MarketRegimeDetector, PortfolioHeat, SignalQuality};
use crate::models::{Action, Strategy};
use crate::monitoring::helius::HeliusClient;
use crate::monitoring::signal_aggregator::SignalAggregator;
use crate::token::{is_non_speculative, is_pumpfun_token, TokenParser};

/// Ingress path a signal arrived through. Recorded for telemetry and, in
/// Phase C, for source-slot latency analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ingress {
    /// Direct operator webhook (`/api/v1/webhook`)
    Webhook,
    /// Helius webhook monitoring path (`/monitoring/helius-webhook`)
    Helius,
}

impl Ingress {
    pub fn as_str(&self) -> &'static str {
        match self {
            Ingress::Webhook => "webhook",
            Ingress::Helius => "helius",
        }
    }
}

/// Scalar thresholds extracted once from `AppConfig`. Bundling them here lets
/// the service be constructed independently of the two handler state structs
/// (which historically exposed overlapping but differently-named copies).
#[derive(Debug, Clone)]
pub struct SelectionConfig {
    pub total_capital_sol: Decimal,
    /// Maximum single-position size in SOL — caps SELL amounts so a caller
    /// cannot close more than the configured ceiling.
    pub max_position_sol: Decimal,
    pub shield_signal_quality_threshold: f64,
    pub spear_signal_quality_threshold: f64,
    pub shield_percent: u32,
    pub spear_percent: u32,
    pub min_liquidity_shield_usd: Decimal,
    pub min_liquidity_spear_usd: Decimal,
    /// Minimum liquidity for graduated pump.fun tokens (USD).
    pub min_liquidity_pumpfun_usd: Decimal,
    /// When true, pump.fun tokens with sufficient DEX liquidity are allowed.
    pub allow_graduated_pumpfun: bool,
    /// Minimum token age in hours. Tokens younger than this are rejected.
    /// Unknown age (API failure): rejected for SPEAR, warned-and-allowed for SHIELD.
    pub min_token_age_hours: f64,
    /// Minimum token age in hours for pump.fun tokens.
    pub min_token_age_pumpfun_hours: f64,
    /// Age waiver floor for statistically-proven wallets (2026-08-07):
    /// wallets passing the t-stat gate trade early entries by design; their
    /// signals are age-waived to this floor instead of the global
    /// `min_token_age_*` (30 min). 0.1h = 6 min — still filters instant rugs.
    /// 0.0 disables the waiver.
    pub min_token_age_proven_hours: f64,
    /// Minimum WQS score for a wallet to be eligible for copying.
    /// Wallets below this are rejected entirely. Configurable via env var
    /// CHIMERA_SELECTION__MIN_WQS_SCORE (default: 70.0).
    pub min_wqs_score: f64,
    /// Maximum position size for low-WQS wallets (below spear_lite_wqs_threshold).
    /// These wallets are admitted but with very small positions to limit risk
    /// while accumulating a track record. Default: 0.10 SOL.
    pub spear_lite_max_size_sol: Decimal,
    /// WQS threshold below which spear_lite_max_size_sol applies.
    /// Wallets with WQS < this value get micro-positions. Default: 40.0.
    pub spear_lite_wqs_threshold: f64,
    /// When true, BUY signals require either multi-wallet consensus (≥2
    /// tracked wallets on the same token) or a wallet with a proven copy-trade
    /// track record (`min_proven_trades` closed trades, optionally with
    /// positive 30d copy PnL). Single-wallet signals from unproven wallets are
    /// rejected — the negative-EV class (verified 2026-08-04..06: 17/17
    /// wallets net-negative, 2 wins / 49 closed trades). Env:
    /// CHIMERA_SELECTION__REQUIRE_CONSENSUS_OR_PROVEN (default true).
    pub require_consensus_or_proven: bool,
    /// Minimum closed copy-trades for the "proven wallet" branch of the
    /// consensus-OR-proven gate. Env: CHIMERA_SELECTION__MIN_PROVEN_TRADES.
    pub min_proven_trades: i32,
    /// Proven branch also requires positive 30d copy PnL.
    /// Env: CHIMERA_SELECTION__REQUIRE_PROVEN_POSITIVE_PNL (default true).
    pub require_proven_positive_pnl: bool,
    /// Shadow-mirror token gate (2026-08-06): admit a token only when its
    /// rolling `mirror_main` shadow average (the whale's own round trip under
    /// our exit rails, pre-cost) is >= `mirror_gate_min_avg_pct` with at least
    /// `mirror_gate_min_samples` exits in `mirror_gate_window_hours`.
    /// Post-cost breakeven is ~1.4%, so +1.5% clears it. Verified on 48h
    /// shadow data: negative-mirror tokens avg -1.82% (est -3.2% net) vs
    /// positive-mirror tokens +2.73% (est +1.3% net). Tokens with insufficient
    /// samples are rejected (SHADOW_MIRROR_INSUFFICIENT) and routed to entry
    /// confirmation, where a price-hold provides the admission evidence.
    pub mirror_gate_enabled: bool,
    pub mirror_gate_min_avg_pct: Decimal,
    pub mirror_gate_min_samples: i32,
    pub mirror_gate_window_hours: i32,
    /// Mirror-gate sample carve-out (2026-08-26): tokens admitted by the
    /// token-age trial are fresh by construction — they can never have the
    /// full `mirror_gate_min_samples` (10) of shadow exits within the window,
    /// so the standard floor would nullify every trial admission
    /// (SHADOW_MIRROR_INSUFFICIENT was the binding constraint post-trial).
    /// Trial tokens instead require only this many deduped exits — and a
    /// thin-but-negative average still rejects via SHADOW_MIRROR_NEGATIVE,
    /// so dump protection is preserved. 0 disables the carve-out.
    pub mirror_gate_trial_min_samples: i32,
    /// Momentum bypass for the shadow-mirror gate (2026-08-11): if the token's
    /// price_cache shows momentum above this percentage (positive trend),
    /// bypass the shadow-mirror sample requirement. Tokens with sustained
    /// upward momentum have proven themselves without needing 10 shadow exits.
    pub momentum_bypass_min_pct: Decimal,
    /// Master switch for the momentum bypass (2026-08-14, default OFF).
    /// Price-cache history spans seconds-to-minutes for fresh tokens, so the
    /// "momentum" it measures is micro-tick noise on an in-progress pump —
    /// deduplicated shadow data shows late entries into pumping tokens are
    /// the losing class (fixed_1h -4.2%/trade deduped vs +314 SOL inflated).
    /// Opt-in only: CHIMERA_SELECTION__MOMENTUM_BYPASS_ENABLED=true.
    pub momentum_bypass_enabled: bool,
    /// Proven-wallet WQS waiver (2026-08-15, default ON): waive the min WQS
    /// floor for wallets proven by deduped shadow statistics (t-stat or
    /// shadow-total). WQS measures the whale's own PnL, not copy PnL — the
    /// two diverge post-dedup. Env:
    /// CHIMERA_SELECTION__WQS_PROVEN_WAIVER_ENABLED.
    pub wqs_proven_waiver_enabled: bool,
    /// Wallet profitability gate (2026-08-07): only admit wallets whose
    /// shadow mirror_main PnL is statistically significant (t-statistic >
    /// threshold). Research: wallet selection is the dominant factor in
    /// copier profitability — 11.3% AUC drop when removed (arxiv 2601.08641).
    /// The only profitable copier strategy used t-stat > 1.645 as a hard gate.
    pub wallet_tstat_enabled: bool,
    pub wallet_tstat_threshold: f64,
    pub wallet_tstat_min_samples: i32,
    pub wallet_tstat_window_days: i32,
    /// Shadow total-PnL proven branch (2026-08-13): a wallet ALSO counts as
    /// "proven" if it has >= `shadow_proven_min_samples` mirror_main shadow
    /// exits with total PnL >= `shadow_proven_min_total_pnl_sol`. This captures
    /// high-variance "moonshot" wallets (e.g. 8% win / +278% avg, +195 SOL over
    /// 70 signals) that the t-stat gate rejects (huge std -> low t). Total PnL
    /// is the realized copy-profitability ground truth. OR'd with the t-stat
    /// path in `wallet_is_proven`. Env: CHIMERA_SELECTION__SHADOW_PROVEN_ENABLED.
    pub shadow_proven_enabled: bool,
    pub shadow_proven_min_samples: i32,
    pub shadow_proven_min_total_pnl_sol: f64,
    /// Token liquidity-velocity gate (2026-08-07): for pump.fun bonding-curve
    /// tokens, only admit those in the FAST-accumulation phase — "liquidity
    /// velocity is the single most informative predictor of graduation"
    /// (arxiv 2602.14860). Velocity = real_sol_reserves / swap_count; slow,
    /// fragmented accumulation signals weak engagement. Also rejects tokens
    /// in the late-curve dump zone (depth discontinuity at graduation).
    pub token_velocity_gate_enabled: bool,
    pub token_min_liquidity_velocity: f64,
    pub token_max_curve_completion: f64,
    /// Smart-money cluster gate (2026-08-07): a single-wallet signal may also
    /// pass the consensus-OR-proven gate when >= `cluster_min_profitable_wallets`
    /// statistically-profitable wallets (t-stat > threshold, see
    /// `wallet_tstat_*`) have BUY signals on the same token within the cluster
    /// window (12h). Research: "10+ smart money wallets buying the same token
    /// within 48 hours indicates coordinated conviction" (Nansen); wallet
    /// selection is the dominant copier-profitability factor (arxiv 2601.08641).
    pub cluster_gate_enabled: bool,
    pub cluster_min_profitable_wallets: usize,
    /// Whale averaging-down gate (2026-08-08): reject BUYs when the signal
    /// wallet has >= `avg_down_min_buys` prior buys on the token within
    /// `avg_down_window_hours` and its latest buy is >= `avg_down_min_drop_pct`
    /// below its FIRST buy — the whale is averaging into a falling knife.
    /// Verified on 2026-08-08: both big losers (-9.4%, -12.2%) were entries
    /// after the whale's 3rd-4th buy into tokens that shadow-closed at
    /// -83%..-99%. Fresh entries and pyramiding-up whales are unaffected.
    pub averaging_down_enabled: bool,
    pub averaging_down_window_hours: i64,
    pub averaging_down_min_buys: usize,
    pub averaging_down_min_drop_pct: Decimal,
    /// Pump-chase gate (2026-08-08): reject BUYs on tokens already up more
    /// than `pump_chase_max_delta_pct` over the last 15 minutes, unless the
    /// signal is multi-wallet consensus or a smart-money cluster (those can
    /// ride pumps with crowd support). Verified: the -2.05% exit on
    /// DcdNm2UX was entered at +15.6%/15m — the top of a pump.
    pub pump_chase_enabled: bool,
    pub pump_chase_max_delta_pct: Decimal,
    /// Stop-loss re-entry cooldown (2026-08-08): after a closed position on a
    /// token lost >= `stop_loss_cooldown_loss_pct` of its entry amount, block
    /// new BUYs on that token for `stop_loss_cooldown_hours`. Verified:
    /// 9p84TE2Z was re-entered 3x after losses and shadow-closed at -83%.
    pub stop_loss_cooldown_enabled: bool,
    pub stop_loss_cooldown_hours: i64,
    pub stop_loss_cooldown_loss_pct: Decimal,
    /// Entry-price guard (2026-08-11): reject BUYs when the current token
    /// price has pumped more than this percentage above the whale's entry.
    /// Prevents the copier from buying the top of a pump that already
    /// happened between the whale's trade and the webhook delivery.
    pub pump_since_whale_guard_enabled: bool,
    pub max_pump_since_whale_pct: Decimal,
    /// Repeat-signal gate (2026-08-11): require at least this many prior
    /// signals on a token before admitting a live trade. One-shot tokens
    /// (single signal, never traded again) have an 8% win rate and generate
    /// 59% of all losses. Repeat tokens (2+) have 18% win rate and +18.4%
    /// avg win move.
    pub repeat_signal_gate_enabled: bool,
    pub repeat_signal_min_prior: i64,
    /// Entry drift guard (2026-08-22): reject BUYs when the current tradable
    /// price has drifted more than `max_entry_drift_pct` in EITHER direction
    /// from the signal-time reference price (the whale's entry). Measured
    /// execution gap (reconcile n=88): shadow marks +0.04%/trade at decision
    /// time vs realized -1.98% gross / -2.99% net — entries re-admitted
    /// 33-310s late (consensus wait, entry-confirmation hold) buy matured
    /// pumps. Shares the pump-since-whale unit conversion (decimals +
    /// plausibility clamp) and fails open with it when prices are unknown.
    pub entry_drift_guard_enabled: bool,
    pub max_entry_drift_pct: Decimal,
    /// WQS trial admission (2026-08-23): sub-floor wallets with WQS >=
    /// `wqs_trial_min_score` are admitted to SPEAR at the existing
    /// spear-lite micro cap instead of being hard-rejected. Rationale: the
    /// 12h prod comparison found 10/10 shadow mirror_main wins (+5.21% avg)
    /// on WQS-10.0 rejects — the documented star-copy-target profile —
    /// blocked while awaiting deduped-shadow significance for the waiver.
    /// Layering: spear-lite caps size (0.25 SOL) AND the consensus-or-proven
    /// gate still blocks solo entries, so trial wallets can only ever
    /// contribute to multi-wallet consensus. Disable to restore hard floor.
    pub wqs_trial_enabled: bool,
    pub wqs_trial_min_score: f64,
    /// Recency-weighted proven overlay (2026-08-24): once any proven path
    /// passes, the wallet's most recent `proven_recency_trades` closed
    /// copy-trades must not be net-negative — long-window aggregates (30d
    /// t-stat, all-time ledger) go stale silently. 0 disables. See
    /// `wallet_is_proven`.
    pub proven_recency_trades: i64,
    /// Token-age trial admission (2026-08-26): SHIELD BUYs on tokens below
    /// the global `min_token_age_*` floor are TRIAL-ADMITTED at
    /// `token_age_trial_max_size_sol` instead of hard-rejected — but only
    /// above the instant-rug zone (`min_token_age_proven_hours`). Evidence:
    /// paper EV is concentrated in sub-floor tokens (`TOKEN_TOO_NEW` rejects
    /// carried +21.6/+7.5/+7.5/+6.4 shadow SOL in one 72h window); trial
    /// entries pay the minimum viable round-trip cost while accumulating live
    /// evidence. Layered safety: liquidity/mirror/quality/drift/pump-chase
    /// gates still apply downstream, and skip-below-min sizing makes the cap
    /// the entry floor. 0-size-cap or disabled restores hard rejection.
    pub token_age_trial_enabled: bool,
    pub token_age_trial_max_size_sol: Decimal,
    /// Per-wallet realized-loss pause (2026-08-26): when the wallet's
    /// REALIZED copy PnL (`positions.realized_net_pnl_sol`, CLOSED, valid)
    /// within the trailing window is worse than `-max_loss`, all its BUYs are
    /// rejected with WALLET_LOSS_PAUSED until trades age out of the window.
    /// Realized loss is ground truth no stale aggregate catches in time.
    /// Disable to restore gate-only discipline.
    pub wallet_loss_pause_enabled: bool,
    pub wallet_loss_pause_max_loss_sol: Decimal,
    pub wallet_loss_pause_window_hours: i32,
}

impl SelectionConfig {
    /// Short fingerprint of the threshold set, for decision records. Not a
    /// cryptographic hash — a stable identifier for "which thresholds were in
    /// force" so Phase C can cohort decisions by config version.
    pub fn hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(self.total_capital_sol.to_string().as_bytes());
        hasher.update(self.max_position_sol.to_string().as_bytes());
        hasher.update(self.shield_signal_quality_threshold.to_le_bytes());
        hasher.update(self.spear_signal_quality_threshold.to_le_bytes());
        hasher.update(self.shield_percent.to_le_bytes());
        hasher.update(self.spear_percent.to_le_bytes());
        hasher.update(self.min_liquidity_shield_usd.to_string().as_bytes());
        hasher.update(self.min_liquidity_spear_usd.to_string().as_bytes());
        hasher.update(self.min_liquidity_pumpfun_usd.to_string().as_bytes());
        hasher.update(u8::from(self.allow_graduated_pumpfun).to_le_bytes());
        hasher.update(self.min_token_age_hours.to_le_bytes());
        hasher.update(self.min_token_age_pumpfun_hours.to_le_bytes());
        hasher.update(self.min_token_age_proven_hours.to_le_bytes());
        hasher.update(self.min_wqs_score.to_le_bytes());
        hasher.update(self.spear_lite_max_size_sol.to_string().as_bytes());
        hasher.update(self.spear_lite_wqs_threshold.to_le_bytes());
        hasher.update(u8::from(self.require_consensus_or_proven).to_le_bytes());
        hasher.update(self.min_proven_trades.to_le_bytes());
        hasher.update(u8::from(self.require_proven_positive_pnl).to_le_bytes());
        hasher.update(u8::from(self.mirror_gate_enabled).to_le_bytes());
        hasher.update(self.mirror_gate_min_avg_pct.to_string().as_bytes());
        hasher.update(self.mirror_gate_min_samples.to_le_bytes());
        hasher.update(self.mirror_gate_window_hours.to_le_bytes());
        hasher.update(self.mirror_gate_trial_min_samples.to_le_bytes());
        hasher.update(self.momentum_bypass_min_pct.to_string().as_bytes());
        hasher.update(u8::from(self.momentum_bypass_enabled).to_le_bytes());
        hasher.update(u8::from(self.wqs_proven_waiver_enabled).to_le_bytes());
        hasher.update(u8::from(self.wallet_tstat_enabled).to_le_bytes());
        hasher.update(self.wallet_tstat_threshold.to_le_bytes());
        hasher.update(self.wallet_tstat_min_samples.to_le_bytes());
        hasher.update(self.wallet_tstat_window_days.to_le_bytes());
        hasher.update(u8::from(self.shadow_proven_enabled).to_le_bytes());
        hasher.update(self.shadow_proven_min_samples.to_le_bytes());
        hasher.update(self.shadow_proven_min_total_pnl_sol.to_le_bytes());
        hasher.update(u8::from(self.token_velocity_gate_enabled).to_le_bytes());
        hasher.update(self.token_min_liquidity_velocity.to_le_bytes());
        hasher.update(self.token_max_curve_completion.to_le_bytes());
        hasher.update(u8::from(self.cluster_gate_enabled).to_le_bytes());
        hasher.update((self.cluster_min_profitable_wallets as u64).to_le_bytes());
        hasher.update(u8::from(self.averaging_down_enabled).to_le_bytes());
        hasher.update(self.averaging_down_window_hours.to_le_bytes());
        hasher.update((self.averaging_down_min_buys as u64).to_le_bytes());
        hasher.update(self.averaging_down_min_drop_pct.to_string().as_bytes());
        hasher.update(u8::from(self.pump_chase_enabled).to_le_bytes());
        hasher.update(self.pump_chase_max_delta_pct.to_string().as_bytes());
        hasher.update(u8::from(self.stop_loss_cooldown_enabled).to_le_bytes());
        hasher.update(self.stop_loss_cooldown_hours.to_le_bytes());
        hasher.update(self.stop_loss_cooldown_loss_pct.to_string().as_bytes());
        hasher.update(u8::from(self.pump_since_whale_guard_enabled).to_le_bytes());
        hasher.update(self.max_pump_since_whale_pct.to_string().as_bytes());
        hasher.update(u8::from(self.repeat_signal_gate_enabled).to_le_bytes());
        hasher.update(self.repeat_signal_min_prior.to_le_bytes());
        hasher.update(u8::from(self.entry_drift_guard_enabled).to_le_bytes());
        hasher.update(self.max_entry_drift_pct.to_string().as_bytes());
        hasher.update(u8::from(self.wqs_trial_enabled).to_le_bytes());
        hasher.update(self.wqs_trial_min_score.to_le_bytes());
        hasher.update(self.proven_recency_trades.to_le_bytes());
        hasher.update(u8::from(self.token_age_trial_enabled).to_le_bytes());
        hasher.update(self.token_age_trial_max_size_sol.to_string().as_bytes());
        hasher.update(u8::from(self.wallet_loss_pause_enabled).to_le_bytes());
        hasher.update(self.wallet_loss_pause_max_loss_sol.to_string().as_bytes());
        hasher.update(self.wallet_loss_pause_window_hours.to_le_bytes());
        hex::encode(&hasher.finalize()[..8])
    }
}

/// Input to a selection decision. `source_amount_sol` is the copied wallet's
/// own swap amount and is recorded as **telemetry only** — it is never used
/// for sizing (the `PositionSizer` governs all BUY sizes).
#[derive(Debug, Clone)]
pub struct SelectionRequest {
    pub wallet_address: String,
    pub token_address: String,
    pub action: Action,
    pub source_amount_sol: Decimal,
    pub ingress: Ingress,
    /// Optional Solana slot of the source transaction (Helius only).
    pub source_slot: Option<u64>,
    /// On-chain timestamp of the copied wallet's source transaction. Used
    /// purely for telemetry (`decision_records.source_block_time`) so
    /// entry-lag slippage (`decided_at - source_block_time`) becomes
    /// measurable per signal. Never feeds any gate or sizing decision.
    pub source_block_time: Option<chrono::DateTime<chrono::Utc>>,
    /// For SELL: the fraction of the position to exit (None = full).
    pub exit_fraction: Option<Decimal>,
    /// Whale's entry price in SOL per raw token unit (2026-08-11):
    /// `swap.amount_in / swap.amount_out`. Used by the entry-price guard to
    /// reject BUYs on tokens that already pumped significantly since the
    /// whale's entry. None when unavailable (gate fails open).
    pub whale_entry_price: Option<Decimal>,
}

/// Typed result of a selection decision. `admitted == true` means the signal
/// passed every gate; the handler then persists + queues it. On rejection the
/// handler inserts a DLQ entry using `rejection_code`/`rejection_reason`.
#[derive(Debug, Clone)]
pub struct BuyDecision {
    pub decision_id: String,
    pub admitted: bool,
    pub rejection_reason: Option<String>,
    pub rejection_code: Option<&'static str>,
    pub strategy: Option<Strategy>,
    pub size_sol: Option<Decimal>,
    /// Copied wallet's own amount (telemetry only; never used for sizing).
    pub source_amount_sol: Decimal,
    pub wqs: Option<f64>,
    pub wqs_confidence: Option<f64>,
    pub quality_score: Option<f64>,
    pub consensus_wallet_count: Option<usize>,
    pub regime_multiplier: Option<Decimal>,
    pub token_age_hours: Option<f64>,
    pub liquidity_usd: Option<Decimal>,
    /// 24h DEX volume in USD. None until the DexScreener feed (B3) wires it.
    pub volume_24h_usd: Option<Decimal>,
    pub price_impact_pct: Option<Decimal>,
    pub config_hash: String,
    pub ingress: Ingress,
    pub is_consensus: bool,
    /// True when the token fast-path check returned an error (RPC/network
    /// failure, not a clean pass/reject). The caller sets `force_slow_path` on
    /// the signal so the engine enforces slow-path verification before entry.
    pub fast_check_errored: bool,
}

impl BuyDecision {
    fn rejected(
        req: &SelectionRequest,
        config_hash: &str,
        code: &'static str,
        reason: String,
    ) -> Self {
        Self {
            decision_id: uuid::Uuid::new_v4().to_string(),
            admitted: false,
            rejection_reason: Some(reason),
            rejection_code: Some(code),
            strategy: None,
            size_sol: None,
            source_amount_sol: req.source_amount_sol,
            wqs: None,
            wqs_confidence: None,
            quality_score: None,
            consensus_wallet_count: None,
            regime_multiplier: None,
            token_age_hours: None,
            liquidity_usd: None,
            volume_24h_usd: None,
            price_impact_pct: None,
            config_hash: config_hash.to_string(),
            ingress: req.ingress,
            is_consensus: false,
            fast_check_errored: false,
        }
    }
}

/// Shared selection engine. Built once in `main.rs` with every capability the
/// two ingress paths collectively need, then shared (Arc) by both.
pub struct SelectionService {
    db: Arc<dyn Database>,
    token_parser: Arc<TokenParser>,
    portfolio_heat: Option<Arc<PortfolioHeat>>,
    signal_aggregator: Option<Arc<SignalAggregator>>,
    market_regime: Option<Arc<MarketRegimeDetector>>,
    helius_client: Option<Arc<HeliusClient>>,
    position_sizer: Option<Arc<PositionSizer>>,
    dexscreener: Option<Arc<crate::monitoring::dexscreener::DexScreenerClient>>,
    toxic_detector: Option<Arc<crate::experiment::ToxicFlowDetector>>,
    decision_recorder: Option<Arc<crate::engine::DecisionRecorder>>,
    quote_client: Option<Arc<crate::engine::transaction_builder::TransactionBuilder>>,
    latency_tracker: Option<Arc<crate::engine::LatencyTracker>>,
    /// Optional wallet-performance tracker for per-wallet copy-performance sizing.
    wallet_performance: Option<Arc<crate::monitoring::WalletPerformanceTracker>>,
    /// Shadow paper trader: trades every signal for evaluation.
    shadow_trader: Option<Arc<crate::engine::ShadowTrader>>,
    /// Rejection-rate wallet mute detector.
    mute_detector: Option<Arc<crate::engine::rejection_mute::RejectionMuteDetector>>,
    /// Shared price cache — used by the pump-chase gate (15m price delta).
    /// Optional: the gate fails open when not wired.
    price_cache: Option<Arc<crate::price_cache::PriceCache>>,
    config: SelectionConfig,
    config_hash: String,
}

impl SelectionService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Arc<dyn Database>,
        token_parser: Arc<TokenParser>,
        portfolio_heat: Option<Arc<PortfolioHeat>>,
        signal_aggregator: Option<Arc<SignalAggregator>>,
        market_regime: Option<Arc<MarketRegimeDetector>>,
        helius_client: Option<Arc<HeliusClient>>,
        position_sizer: Option<Arc<PositionSizer>>,
        config: SelectionConfig,
    ) -> Self {
        let config_hash = config.hash();
        Self {
            db,
            token_parser,
            portfolio_heat,
            signal_aggregator,
            market_regime,
            helius_client,
            position_sizer,
            dexscreener: None,
            toxic_detector: None,
            decision_recorder: None,
            quote_client: None,
            latency_tracker: None,
            wallet_performance: None,
            shadow_trader: None,
            mute_detector: None,
            price_cache: None,
            config,
            config_hash,
        }
    }

    /// Attach the DexScreener client (B3) for volume data.
    pub fn with_dexscreener(
        mut self,
        client: Arc<crate::monitoring::dexscreener::DexScreenerClient>,
    ) -> Self {
        self.dexscreener = Some(client);
        self
    }

    /// Attach the wallet-performance tracker for tiered copy-performance sizing
    /// (proven wallets get a larger allocation). No-op if not attached (floor sizing applies).
    pub fn with_wallet_performance(
        mut self,
        tracker: Arc<crate::monitoring::WalletPerformanceTracker>,
    ) -> Self {
        self.wallet_performance = Some(tracker);
        self
    }

    /// Attach the ToxicFlowDetector (B3) for toxic-wallet gating.
    pub fn with_toxic_detector(
        mut self,
        detector: Arc<crate::experiment::ToxicFlowDetector>,
    ) -> Self {
        self.toxic_detector = Some(detector);
        self
    }

    /// Attach the DecisionRecorder (C1) for fire-and-forget decision persistence.
    pub fn with_decision_recorder(
        mut self,
        recorder: Arc<crate::engine::DecisionRecorder>,
    ) -> Self {
        self.decision_recorder = Some(recorder);
        self
    }

    /// Attach a Jupiter quote client + latency tracker (C3) for shadow-fill
    /// calibration. When present, admitted decisions spawn a fire-and-forget
    /// task that captures a decision-time quote and a delayed requote to model
    /// realistic paper fill prices.
    pub fn with_shadow_fill(
        mut self,
        quote_client: Arc<crate::engine::transaction_builder::TransactionBuilder>,
        latency_tracker: Arc<crate::engine::LatencyTracker>,
    ) -> Self {
        self.quote_client = Some(quote_client);
        self.latency_tracker = Some(latency_tracker);
        self
    }

    /// Optional variant: attach shadow-fill only if the quote client built
    /// successfully. A `None` quote client disables calibration (decisions are
    /// still recorded; `quote_json` stays NULL).
    pub fn with_shadow_fill_opt(
        mut self,
        quote_client: Option<Arc<crate::engine::transaction_builder::TransactionBuilder>>,
        latency_tracker: Arc<crate::engine::LatencyTracker>,
    ) -> Self {
        if let Some(qc) = quote_client {
            self.quote_client = Some(qc);
            self.latency_tracker = Some(latency_tracker);
        }
        self
    }

    /// Attach the ShadowTrader (paper trades every signal for evaluation).
    pub fn with_shadow_trader(mut self, trader: Arc<crate::engine::ShadowTrader>) -> Self {
        self.shadow_trader = Some(trader);
        self
    }

    /// Attach the RejectionMuteDetector for rejection-rate-based wallet muting.
    pub fn with_mute_detector(
        mut self,
        detector: Arc<crate::engine::rejection_mute::RejectionMuteDetector>,
    ) -> Self {
        self.mute_detector = Some(detector);
        self
    }

    /// Attach the shared price cache for the pump-chase gate (15m delta).
    /// The gate fails open (does not reject) when no cache is attached.
    pub fn with_price_cache(mut self, price_cache: Arc<crate::price_cache::PriceCache>) -> Self {
        self.price_cache = Some(price_cache);
        self
    }

    pub fn config_hash(&self) -> &str {
        &self.config_hash
    }

    /// Access the attached DecisionRecorder, if any. Handlers use this to
    /// link a persisted decision to its trade (`link_trade`) and to attach
    /// Jupiter quotes (`update_quote`).
    pub fn decision_recorder(&self) -> Option<&Arc<crate::engine::DecisionRecorder>> {
        self.decision_recorder.as_ref()
    }

    /// Evaluate a signal through the unified decision pipeline.
    ///
    /// When a [`DecisionRecorder`] is attached, every decision (admitted or
    /// rejected) is persisted fire-and-forget as the last step before
    /// returning, so the full admission funnel is captured for the run.
    pub async fn decide(&self, req: &SelectionRequest) -> BuyDecision {
        self.decide_with_options(req, false).await
    }

    /// Like [`decide`], but `bypass_consensus_proven` skips the
    /// consensus-OR-proven gate (step 7b). Used by the entry-confirmation
    /// manager: a single-wallet unproven signal whose token held its price
    /// through the confirmation window is re-evaluated with the price-hold
    /// acting as the admission criterion instead of the hard gate. All other
    /// gates (quality, sizing, heat, safety) still run.
    pub async fn decide_with_options(
        &self,
        req: &SelectionRequest,
        bypass_consensus_proven: bool,
    ) -> BuyDecision {
        let received_at = chrono::Utc::now();
        let decision = match req.action {
            Action::Buy => self.decide_buy(req, bypass_consensus_proven).await,
            Action::Sell => self.decide_sell(req).await,
        };
        tracing::debug!(
            ingress = ?req.ingress,
            decision = %req.action,
            token = %req.token_address,
            wallet = %req.wallet_address,
            admitted = decision.admitted,
            rejection_code = ?decision.rejection_code,
            strategy = ?decision.strategy,
            size_sol = ?decision.size_sol,
            decision_id = %decision.decision_id,
            "selection: decision finalized"
        );
        if let Some(ref recorder) = self.decision_recorder {
            // trade_uuid is linked by the caller after the trade row is
            // inserted (the Helius path derives it from the decision size, so
            // it is not available here). See DecisionRecorder::link_trade.
            recorder.record(&decision, req, None, received_at);
        }
        // Shadow paper trader: fork every signal (fire-and-forget).
        if let Some(ref shadow) = self.shadow_trader {
            shadow.on_signal(&decision, req);
        }
        // Rejection-rate mute detector: record BUY decision outcomes for
        // rolling-window rejection-rate tracking. Only BUY decisions are
        // meaningful (SELL rejections have different semantics).
        if let Some(ref mute) = self.mute_detector {
            if matches!(req.action, Action::Buy) {
                let _ = mute
                    .record_decision(
                        &req.wallet_address,
                        decision.admitted,
                        decision.rejection_code,
                    )
                    .await;
            }
        }
        // C3: shadow-fill calibration for admitted decisions (fire-and-forget).
        if decision.admitted
            && decision.size_sol.is_some()
            && self.quote_client.is_some()
            && self.latency_tracker.is_some()
            && self.decision_recorder.is_some()
        {
            let decided_at = chrono::Utc::now();
            let decide_latency_us = decided_at
                .signed_duration_since(received_at)
                .num_microseconds()
                .unwrap_or(0)
                .max(0) as u64;
            let size_sol = decision.size_sol.and_then(|d| d.to_f64()).unwrap_or(0.0);
            if size_sol > 0.0 {
                tokio::spawn(crate::engine::shadow_fill::capture_and_model_fill(
                    self.quote_client.clone().unwrap(),
                    self.latency_tracker.clone().unwrap(),
                    self.decision_recorder.clone().unwrap(),
                    decision.decision_id.clone(),
                    req.token_address.clone(),
                    size_sol,
                    decide_latency_us,
                    matches!(req.action, Action::Buy),
                ));
            }
        }
        decision
    }

    async fn decide_sell(&self, req: &SelectionRequest) -> BuyDecision {
        // 1. Wallet status gate (fail-closed on DB error / unknown wallet).
        let wallet = match self.db.get_wallet(&req.wallet_address).await {
            Ok(Some(w)) => w,
            Ok(None) => {
                let reason = format!("Unknown wallet {}", req.wallet_address);
                tracing::info!(
                    ingress = ?req.ingress,
                    decision = "SELL",
                    token = %req.token_address,
                    wallet = %req.wallet_address,
                    rejection_code = "UNKNOWN_WALLET",
                    reason = %reason,
                    "selection: SELL rejected by gate"
                );
                return BuyDecision::rejected(req, &self.config_hash, "UNKNOWN_WALLET", reason);
            }
            Err(e) => {
                let reason = format!("DB error fetching wallet: {}", e);
                tracing::info!(
                    ingress = ?req.ingress,
                    decision = "SELL",
                    token = %req.token_address,
                    wallet = %req.wallet_address,
                    rejection_code = "WALLET_LOOKUP_ERROR",
                    reason = %reason,
                    error = %e,
                    "selection: SELL rejected by gate"
                );
                return BuyDecision::rejected(
                    req,
                    &self.config_hash,
                    "WALLET_LOOKUP_ERROR",
                    reason,
                );
            }
        };
        if wallet.status != "ACTIVE" {
            let reason = format!("Wallet status {} != ACTIVE", wallet.status);
            tracing::info!(
                ingress = ?req.ingress,
                decision = "SELL",
                token = %req.token_address,
                wallet = %req.wallet_address,
                rejection_code = "WALLET_NOT_ACTIVE",
                reason = %reason,
                wallet_status = %wallet.status,
                "selection: SELL rejected by gate"
            );
            return BuyDecision::rejected(req, &self.config_hash, "WALLET_NOT_ACTIVE", reason);
        }

        // 2. Only exit if we actually hold an active position for this token.
        match self
            .db
            .get_active_position_by_wallet_token(&req.wallet_address, &req.token_address)
            .await
        {
            Ok(Some(_)) => {}
            Ok(None) => {
                let reason = "No active position to close".to_string();
                tracing::info!(
                    ingress = ?req.ingress,
                    decision = "SELL",
                    token = %req.token_address,
                    wallet = %req.wallet_address,
                    rejection_code = "NO_ACTIVE_POSITION",
                    reason = %reason,
                    "selection: SELL rejected by gate"
                );
                return BuyDecision::rejected(req, &self.config_hash, "NO_ACTIVE_POSITION", reason);
            }
            Err(e) => {
                let reason = format!("Position lookup failed: {}", e);
                tracing::info!(
                    ingress = ?req.ingress,
                    decision = "SELL",
                    token = %req.token_address,
                    wallet = %req.wallet_address,
                    rejection_code = "POSITION_LOOKUP_ERROR",
                    reason = %reason,
                    error = %e,
                    "selection: SELL rejected by gate"
                );
                return BuyDecision::rejected(
                    req,
                    &self.config_hash,
                    "POSITION_LOOKUP_ERROR",
                    reason,
                );
            }
        }

        let wqs = wallet.wqs_score.and_then(|d| d.to_f64());
        // Apply exit_fraction so a partial exit is recorded at the size that
        // will actually be sold, not the full source amount.
        let exit_fraction = req.exit_fraction.unwrap_or(Decimal::ONE);
        let size_sol =
            Some((req.source_amount_sol * exit_fraction).min(self.config.max_position_sol));
        tracing::debug!(
            ingress = ?req.ingress,
            decision = "SELL",
            token = %req.token_address,
            wallet = %req.wallet_address,
            admitted = true,
            size_sol = ?size_sol,
            wqs = ?wqs,
            wqs_confidence = ?wallet.wqs_confidence.and_then(|d| d.to_f64()),
            "selection: SELL admitted"
        );
        BuyDecision {
            decision_id: uuid::Uuid::new_v4().to_string(),
            admitted: true,
            rejection_reason: None,
            rejection_code: None,
            // Exit strategy label; the real strategy of the open position is
            // read from the position row downstream.
            strategy: Some(Strategy::Exit),
            // SELL size = source amount, capped to max_position_sol so a
            // caller cannot close more than the configured ceiling.
            size_sol,
            source_amount_sol: req.source_amount_sol,
            wqs,
            wqs_confidence: wallet.wqs_confidence.and_then(|d| d.to_f64()),
            quality_score: None,
            consensus_wallet_count: None,
            regime_multiplier: None,
            token_age_hours: None,
            liquidity_usd: None,
            volume_24h_usd: None,
            price_impact_pct: None,
            config_hash: self.config_hash.clone(),
            ingress: req.ingress,
            is_consensus: false,
            fast_check_errored: false,
        }
    }

    async fn decide_buy(
        &self,
        req: &SelectionRequest,
        bypass_consensus_proven: bool,
    ) -> BuyDecision {
        // ── 0. Token address format validation (cheap, fail fast) ──────────
        if req
            .token_address
            .parse::<solana_sdk::pubkey::Pubkey>()
            .is_err()
        {
            let reason = format!("Invalid Solana token address: {}", req.token_address);
            tracing::info!(
                ingress = ?req.ingress,
                decision = "BUY",
                token = %req.token_address,
                wallet = %req.wallet_address,
                rejection_code = "INVALID_TOKEN_ADDRESS",
                reason = %reason,
                "selection: BUY rejected by gate"
            );
            return BuyDecision::rejected(req, &self.config_hash, "INVALID_TOKEN_ADDRESS", reason);
        }

        // ── 1. Wallet fetch + ACTIVE status gate ────────────────────────────
        let wallet = match self.db.get_wallet(&req.wallet_address).await {
            Ok(Some(w)) => w,
            Ok(None) => {
                let reason = "Unknown wallet — not in roster".to_string();
                tracing::info!(
                    ingress = ?req.ingress,
                    decision = "BUY",
                    token = %req.token_address,
                    wallet = %req.wallet_address,
                    rejection_code = "UNKNOWN_WALLET",
                    reason = %reason,
                    "selection: BUY rejected by gate"
                );
                return BuyDecision::rejected(req, &self.config_hash, "UNKNOWN_WALLET", reason);
            }
            Err(e) => {
                let reason = format!("DB error fetching wallet: {}", e);
                tracing::info!(
                    ingress = ?req.ingress,
                    decision = "BUY",
                    token = %req.token_address,
                    wallet = %req.wallet_address,
                    rejection_code = "WALLET_LOOKUP_ERROR",
                    reason = %reason,
                    error = %e,
                    "selection: BUY rejected by gate"
                );
                return BuyDecision::rejected(
                    req,
                    &self.config_hash,
                    "WALLET_LOOKUP_ERROR",
                    reason,
                );
            }
        };
        if wallet.status != "ACTIVE" {
            let reason = format!("Wallet status {} != ACTIVE", wallet.status);
            tracing::info!(
                ingress = ?req.ingress,
                decision = "BUY",
                token = %req.token_address,
                wallet = %req.wallet_address,
                rejection_code = "WALLET_NOT_ACTIVE",
                reason = %reason,
                wallet_status = %wallet.status,
                "selection: BUY rejected by gate"
            );
            return BuyDecision::rejected(req, &self.config_hash, "WALLET_NOT_ACTIVE", reason);
        }

        // B3: Toxic-wallet gate — reject signals from wallets flagged toxic.
        if let Some(ref detector) = self.toxic_detector {
            if detector.is_wallet_toxic(&req.wallet_address).await {
                let reason =
                    "Wallet flagged as toxic — post-promotion ROI deterioration".to_string();
                tracing::warn!(
                    ingress = ?req.ingress,
                    decision = "BUY",
                    token = %req.token_address,
                    wallet = %req.wallet_address,
                    rejection_code = "TOXIC_WALLET",
                    reason = %reason,
                    "selection: BUY rejected by gate"
                );
                return BuyDecision::rejected(req, &self.config_hash, "TOXIC_WALLET", reason);
            }
        }

        // Per-wallet realized-loss pause (2026-08-26): a wallet whose REALIZED
        // copy PnL over the trailing window already burned more than
        // `wallet_loss_pause_max_loss_sol` stops copying until the window
        // rolls off — realized loss is ground truth that no WQS/t-stat
        // aggregate goes stale fast enough to see (ArcebCcX: −0.16 SOL across
        // 8 trades in one 72h window while compliant with every other gate).
        // Fail-open on DB error: a transient stats outage must not halt all
        // trading (the global circuit breaker owns that risk role).
        if self.config.wallet_loss_pause_enabled {
            match self
                .db
                .get_wallet_realized_pnl_window(
                    &req.wallet_address,
                    self.config.wallet_loss_pause_window_hours,
                )
                .await
            {
                Ok(Some(pnl)) if pnl <= -self.config.wallet_loss_pause_max_loss_sol => {
                    let reason = format!(
                        "Wallet realized {} SOL within {}h — paused (max_loss {})",
                        pnl,
                        self.config.wallet_loss_pause_window_hours,
                        -self.config.wallet_loss_pause_max_loss_sol
                    );
                    tracing::warn!(
                        ingress = ?req.ingress,
                        decision = "BUY",
                        token = %req.token_address,
                        wallet = %req.wallet_address,
                        rejection_code = "WALLET_LOSS_PAUSED",
                        reason = %reason,
                        realized_pnl_sol = %pnl,
                        pause_max_loss_sol = %self.config.wallet_loss_pause_max_loss_sol,
                        window_hours = self.config.wallet_loss_pause_window_hours,
                        "selection: BUY rejected by gate"
                    );
                    return BuyDecision::rejected(
                        req,
                        &self.config_hash,
                        "WALLET_LOSS_PAUSED",
                        reason,
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!(
                        wallet = %req.wallet_address,
                        error = %e,
                        "Loss-pause check failed — fail-open (stats outage)"
                    );
                }
            }
        }

        // Rejection-rate mute gate — short-circuit wallets with overwhelming
        // hard-rejection rates (non-speculative / unsafe / illiquid pump.fun).
        if let Some(ref detector) = self.mute_detector {
            if detector.is_wallet_muted(&req.wallet_address).await {
                let reason = "Wallet muted — sustained high hard-rejection rate".to_string();
                tracing::info!(
                    ingress = ?req.ingress,
                    decision = "BUY",
                    token = %req.token_address,
                    wallet = %req.wallet_address,
                    rejection_code = "WALLET_MUTED",
                    "selection: BUY rejected by rejection-mute gate"
                );
                return BuyDecision::rejected(req, &self.config_hash, "WALLET_MUTED", reason);
            }
        }

        let mut wallet_wqs = wallet.wqs_score.and_then(|d| d.to_f64()).unwrap_or(0.0);
        let wqs_confidence = wallet.wqs_confidence.and_then(|d| d.to_f64());
        let wallet_success_rate = wallet
            .win_rate
            .unwrap_or(Decimal::from_f64_retain(0.5).unwrap_or(Decimal::ZERO));

        // ── 2. Hard WQS gate + strategy assignment ──────────────────────────
        // Configurable minimum WQS; ≥80 → SHIELD; min..80 → SPEAR.
        // pump.fun tokens always use SHIELD (SPEAR has 0% win rate on pump.fun).
        let min_wqs = self.config.min_wqs_score;
        // Proven-ness resolved once per decision (2026-08-18): reused by the
        // WQS waiver here AND the sizing factors at the sizing step — one
        // oracle, one DB round trip. None = not yet resolved (WQS ≥ floor
        // wallets skip the waiver query; the sizing site resolves lazily
        // only when the proven boost is configured).
        let mut wallet_proven: Option<bool> = None;
        if wallet_wqs < min_wqs {
            // Proven-wallet WQS waiver (2026-08-15): WQS measures the whale's
            // OWN PnL; the deduped mirror_main t-stat / shadow-total evidence
            // measures how profitable the wallet is to COPY with our exits —
            // the actual objective. Post-dedup these diverge: the two best
            // copy targets (t=2.65/n=175, t=2.33/n=78) sit at WQS 10 and were
            // hard-blocked here while several WQS-80 wallets are dedup-negative.
            // Waive the floor only for wallets already proven by deduped
            // shadow statistics (same pattern as the proven age waiver).
            let proven_waiver = self.config.wqs_proven_waiver_enabled
                && self.wallet_is_proven(&req.wallet_address).await;
            // Cache for the sizing site: when the waiver is disabled the
            // wallet is rejected here regardless, so Some(false) is exact.
            wallet_proven = Some(proven_waiver);
            if !proven_waiver {
                // WQS trial admission (2026-08-23): near-floor wallets
                // (>= wqs_trial_min_score) enter at spear-lite micro size —
                // see SelectionConfig field docs for the layering. The clamp
                // to `min_wqs` below mirrors the proven-waiver path so
                // quality scoring does not re-reject what this gate waived.
                let trial_admitted =
                    self.config.wqs_trial_enabled && wallet_wqs >= self.config.wqs_trial_min_score;
                if !trial_admitted {
                    let reason =
                        format!("Wallet WQS {:.1} below minimum {:.1}", wallet_wqs, min_wqs);
                    tracing::info!(
                        ingress = ?req.ingress,
                        decision = "BUY",
                        token = %req.token_address,
                        wallet = %req.wallet_address,
                        rejection_code = "WQS_TOO_LOW",
                        reason = %reason,
                        wallet_wqs = wallet_wqs,
                        min_wqs = min_wqs,
                        "selection: BUY rejected by gate"
                    );
                    return BuyDecision::rejected(req, &self.config_hash, "WQS_TOO_LOW", reason);
                }
                tracing::info!(
                    ingress = ?req.ingress,
                    decision = "BUY",
                    token = %req.token_address,
                    wallet = %req.wallet_address,
                    wallet_wqs = wallet_wqs,
                    min_wqs = min_wqs,
                    trial_min = self.config.wqs_trial_min_score,
                    "WQS floor trial admission: sub-floor wallet enters at spear-lite micro size"
                );
            } else {
                tracing::info!(
                    ingress = ?req.ingress,
                    decision = "BUY",
                    token = %req.token_address,
                    wallet = %req.wallet_address,
                    wallet_wqs = wallet_wqs,
                    min_wqs = min_wqs,
                    "WQS floor waived: wallet proven by deduped shadow statistics"
                );
            }
            // Score quality at the floor, not the raw sub-floor WQS — for
            // BOTH the proven waiver and the trial admission: the 40%-
            // weighted WQS term would otherwise pin a sub-floor wallet at
            // ~0.34 max — knife-edge against the 0.30 SPEAR threshold — and
            // re-reject here what step 2 just waived.
            wallet_wqs = min_wqs;
        }
        let is_pumpfun = is_pumpfun_token(&req.token_address);
        // NOTE (2026-08-18): proven wallets were considered for Shield
        // routing here, but waived wallets carry wallet_wqs = min_wqs (15)
        // after the floor rewrite and score signal quality at that floor —
        // which clears SPEAR's threshold but not SHIELD's higher one. Shield
        // routing would re-reject at the quality gate exactly what the
        // waiver admitted. Proven sizing under SPEAR is enabled instead by
        // raising spear_max_size_sol (config) above proven_size_sol.
        let strategy = if wallet_wqs >= 80.0 || is_pumpfun {
            Strategy::Shield
        } else {
            Strategy::Spear
        };

        // ── 3. Non-speculative / pump.fun bonding-curve skip ────────────────
        if is_non_speculative(&req.token_address) {
            let reason = "Stablecoin/WSOL — no profit potential".to_string();
            tracing::info!(
                ingress = ?req.ingress,
                decision = "BUY",
                token = %req.token_address,
                wallet = %req.wallet_address,
                rejection_code = "NON_SPECULATIVE_TOKEN",
                reason = %reason,
                strategy = ?strategy,
                is_pumpfun = is_pumpfun,
                "selection: BUY rejected by gate"
            );
            return BuyDecision::rejected(req, &self.config_hash, "NON_SPECULATIVE_TOKEN", reason);
        }
        let is_pumpfun = is_pumpfun_token(&req.token_address); // computed above for strategy routing
        if is_pumpfun && !self.config.allow_graduated_pumpfun {
            let reason = "pump.fun token — graduated-pumpfun disabled in config".to_string();
            tracing::info!(
                ingress = ?req.ingress,
                decision = "BUY",
                token = %req.token_address,
                wallet = %req.wallet_address,
                rejection_code = "PUMPFUN_BONDING_CURVE",
                reason = %reason,
                strategy = ?strategy,
                is_pumpfun = is_pumpfun,
                allow_graduated_pumpfun = self.config.allow_graduated_pumpfun,
                "selection: BUY rejected by gate"
            );
            return BuyDecision::rejected(req, &self.config_hash, "PUMPFUN_BONDING_CURVE", reason);
        }

        // ── 4. Token fast_check ─────────────────────────────────────────────
        let fast_check_errored = false;
        let fast_check_liquidity: Option<Decimal> = match self
            .token_parser
            .fast_check(&req.token_address, strategy)
            .await
        {
            Ok(result) if !result.safe => {
                let reason = result
                    .rejection_reason
                    .unwrap_or_else(|| "Token failed safety check".to_string());
                tracing::warn!(
                    ingress = ?req.ingress,
                    decision = "BUY",
                    token = %req.token_address,
                    wallet = %req.wallet_address,
                    rejection_code = "TOKEN_UNSAFE",
                    reason = %reason,
                    strategy = ?strategy,
                    is_pumpfun = is_pumpfun,
                    "selection: BUY rejected by gate"
                );
                return BuyDecision::rejected(req, &self.config_hash, "TOKEN_UNSAFE", reason);
            }
            Ok(result) => result.liquidity_usd,
            Err(e) => {
                // Fail-closed: a token whose safety check errored (RPC/network
                // failure) is NOT admitted. The webhook path previously relied
                // on the caller setting `force_slow_path`, but the Helius
                // ingress never honored that flag — a token could reach
                // execution with zero token-safety verification. Rejecting here
                // enforces slow-path-equivalent safety uniformly.
                let reason = format!("Token fast-check errored — rejected (fail-closed): {}", e);
                tracing::warn!(
                    ingress = ?req.ingress,
                    decision = "BUY",
                    token = %req.token_address,
                    wallet = %req.wallet_address,
                    rejection_code = "TOKEN_FAST_CHECK_ERRORED",
                    reason = %reason,
                    error = %e,
                    "selection: BUY rejected by gate"
                );
                return BuyDecision::rejected(
                    req,
                    &self.config_hash,
                    "TOKEN_FAST_CHECK_ERRORED",
                    reason,
                );
            }
        };

        // ── 5. Token-age enforcement ────────────────────────────────────────
        let token_age_hours = if let Some(ref helius) = self.helius_client {
            helius
                .get_token_age_hours(&req.token_address)
                .await
                .ok()
                .flatten()
        } else {
            None
        };

        let min_age = if is_pumpfun {
            self.config.min_token_age_pumpfun_hours
        } else {
            self.config.min_token_age_hours
        };
        // Set when the token-age trial admits a sub-floor token (below);
        // carried into sizing as an extra micro-position cap.
        let mut age_trial_cap: Option<Decimal> = None;
        if min_age > 0.0 {
            match token_age_hours {
                Some(age) if age < min_age => {
                    // Proven-wallet age waiver (2026-08-07): wallets with
                    // statistically significant shadow PnL (t-stat gate) trade
                    // early entries by design — their fresh-token signals are
                    // the edge. Age-waive them to
                    // `min_token_age_proven_hours` (default 0.1h = 6 min,
                    // still filters instant rug pulls). Unproven wallets keep
                    // the full maturity filter.
                    let proven = self.wallet_has_significant_pnl(&req.wallet_address).await;
                    let effective_min = if proven && self.config.min_token_age_proven_hours > 0.0 {
                        self.config.min_token_age_proven_hours
                    } else {
                        min_age
                    };
                    if age >= effective_min {
                        tracing::info!(
                            ingress = ?req.ingress,
                            decision = "BUY",
                            token = %req.token_address,
                            wallet = %req.wallet_address,
                            token_age_hours = age,
                            global_min = min_age,
                            proven_floor = effective_min,
                            strategy = ?strategy,
                            is_pumpfun = is_pumpfun,
                            "Proven-wallet age waiver: token below global min but above proven floor"
                        );
                    } else {
                        // Token-age trial admission (2026-08-26): instead of
                        // hard-rejecting, admit SHIELD BUYs on sub-floor
                        // tokens at a micro-size cap — above the instant-rug
                        // floor (`min_token_age_proven_hours`) only. Evidence:
                        // the 72h window's paper PnL was concentrated in
                        // TOKEN_TOO_NEW rejects (+21.6/+7.5/+7.5/+6.4 shadow
                        // SOL). Downstream gates (consensus-or-proven,
                        // liquidity floor, mirror gate, quality, drift,
                        // pump-chase) still apply; sizing clamps to the cap.
                        let trial_eligible = self.config.token_age_trial_enabled
                            && strategy == Strategy::Shield
                            && self.config.min_token_age_proven_hours > 0.0
                            && age >= self.config.min_token_age_proven_hours;
                        if trial_eligible {
                            tracing::info!(
                                ingress = ?req.ingress,
                                decision = "BUY",
                                token = %req.token_address,
                                wallet = %req.wallet_address,
                                token_age_hours = age,
                                global_min = min_age,
                                proven_floor = self.config.min_token_age_proven_hours,
                                trial_cap_sol = %self.config.token_age_trial_max_size_sol,
                                strategy = ?strategy,
                                is_pumpfun = is_pumpfun,
                                "Token-age TRIAL admission: sub-floor token admitted at micro size"
                            );
                            age_trial_cap = Some(self.config.token_age_trial_max_size_sol);
                        } else {
                            let reason = format!(
                                "Token age {:.1}h below minimum {:.1}h",
                                age, effective_min
                            );
                            tracing::info!(
                                ingress = ?req.ingress,
                                decision = "BUY",
                                token = %req.token_address,
                                wallet = %req.wallet_address,
                                rejection_code = "TOKEN_TOO_NEW",
                                reason = %reason,
                                token_age_hours = age,
                                min_age = effective_min,
                                strategy = ?strategy,
                                is_pumpfun = is_pumpfun,
                                "selection: BUY rejected by gate"
                            );
                            let mut decision = BuyDecision::rejected(
                                req,
                                &self.config_hash,
                                "TOKEN_TOO_NEW",
                                reason,
                            );
                            // Telemetry (2026-08-26): rejected() zeroes
                            // token_age_hours, leaving decision_records unable to
                            // audit how far below the age floor blocked tokens
                            // sat. Keep the offending age on the record.
                            decision.token_age_hours = Some(age);
                            return decision;
                        }
                    }
                }
                None => {
                    // Unknown age — policy: reject SPEAR, allow SHIELD.
                    if strategy == Strategy::Spear {
                        let reason = "Token age unknown — rejected for SPEAR (conservative policy)"
                            .to_string();
                        tracing::info!(
                            ingress = ?req.ingress,
                            decision = "BUY",
                            token = %req.token_address,
                            wallet = %req.wallet_address,
                            rejection_code = "TOKEN_AGE_UNKNOWN",
                            reason = %reason,
                            strategy = ?strategy,
                            is_pumpfun = is_pumpfun,
                            min_age = min_age,
                            "selection: BUY rejected by gate"
                        );
                        return BuyDecision::rejected(
                            req,
                            &self.config_hash,
                            "TOKEN_AGE_UNKNOWN",
                            reason,
                        );
                    }
                    tracing::warn!(
                        token = %req.token_address,
                        "Token age unknown — allowed for SHIELD (warn-and-allow policy)"
                    );
                }
                _ => {} // age known and above threshold — proceed
            }
        }

        // ── 6. Liquidity floor + volume ─────────────────────────────────────
        let liquidity_usd = match fast_check_liquidity {
            Some(liq) => liq,
            None => match self.token_parser.get_liquidity(&req.token_address).await {
                Ok(liq) => liq,
                Err(e) => {
                    tracing::warn!(
                        token = %req.token_address,
                        error = %e,
                        "Liquidity fetch failed; defaulting to $0 (fail-closed)"
                    );
                    Decimal::ZERO
                }
            },
        };

        let min_liquidity = if is_pumpfun {
            self.config.min_liquidity_pumpfun_usd
        } else {
            match strategy {
                Strategy::Shield => self.config.min_liquidity_shield_usd,
                Strategy::Spear => self.config.min_liquidity_spear_usd,
                Strategy::Exit => Decimal::ZERO,
            }
        };
        if !SignalQuality::passes_liquidity_floor(liquidity_usd, min_liquidity) {
            let (code, reason) = if is_pumpfun {
                (
                    "PUMPFUN_INSUFFICIENT_LIQUIDITY",
                    format!(
                        "pump.fun token liquidity ${} below graduated threshold ${}",
                        liquidity_usd, min_liquidity
                    ),
                )
            } else {
                (
                    "LIQUIDITY_BELOW_MINIMUM",
                    format!(
                        "Liquidity ${} below strategy minimum ${}",
                        liquidity_usd, min_liquidity
                    ),
                )
            };
            tracing::info!(
                ingress = ?req.ingress,
                decision = "BUY",
                token = %req.token_address,
                wallet = %req.wallet_address,
                rejection_code = code,
                reason = %reason,
                liquidity_usd = %liquidity_usd,
                min_liquidity = %min_liquidity,
                strategy = ?strategy,
                is_pumpfun = is_pumpfun,
                "selection: BUY rejected by gate"
            );
            return BuyDecision::rejected(req, &self.config_hash, code, reason);
        }

        // ── 6b. 24h volume via DexScreener (B3, fail-open) ─────────────────
        let volume_24h_usd = if let Some(ref dex) = self.dexscreener {
            dex.get_volume_24h(&req.token_address).await
        } else {
            None
        };

        // ── 7. Consensus detection (read-only peek) ────────────────────────
        // The signal itself is recorded into the aggregator window only AFTER
        // the decision is admitted (step 12) — signals rejected on quality,
        // sizing, or heat must not inflate consensus_wallet_count or fabricate
        // consensus for subsequent signals.
        let mut consensus_wallet_count: Option<usize> = None;
        let is_consensus = if let Some(ref aggregator) = self.signal_aggregator {
            let count = aggregator
                .peek_consensus_wallet_count(&req.token_address)
                .await;
            consensus_wallet_count = Some(count.max(1));
            count >= 2
        } else {
            false
        };

        // Smart-money cluster: distinct statistically-profitable wallets with
        // BUY signals on this token within the (12h) cluster window.
        let mut profitable_cluster_count: Option<usize> = None;
        let is_smart_money_cluster = if let Some(ref aggregator) = self.signal_aggregator {
            let count = aggregator
                .peek_profitable_cluster_count(&req.token_address)
                .await;
            profitable_cluster_count = Some(count);
            count >= self.config.cluster_min_profitable_wallets
        } else {
            false
        };

        // ── 7b. Consensus-OR-proven gate ───────────────────────────────────
        // Single-wallet signals from wallets without a proven copy-trade
        // record are the negative-EV class (all wallets net-negative since
        // 2026-08-04; config note: only the 0.50+ signal-quality band is
        // gross-profitable). Require either multi-wallet consensus or a wallet
        // with >= min_proven_trades closed copy-trades (and, when configured,
        // positive 30d copy PnL) before admitting a BUY. Exit/SELL decisions
        // are never gated here.
        if !bypass_consensus_proven
            && self.config.require_consensus_or_proven
            && !is_consensus
            && !is_smart_money_cluster
            && strategy != Strategy::Exit
        {
            let proven = self.wallet_is_proven(&req.wallet_address).await;
            if !proven {
                let reason = format!(
                    "Single-wallet signal from unproven wallet — requires consensus (≥2 wallets), smart-money cluster (≥{} profitable wallets), or ≥{} closed copy-trades{}",
                    self.config.cluster_min_profitable_wallets,
                    self.config.min_proven_trades,
                    if self.config.require_proven_positive_pnl {
                        " with positive 30d copy PnL"
                    } else {
                        ""
                    }
                );
                tracing::info!(
                    ingress = ?req.ingress,
                    decision = "BUY",
                    token = %req.token_address,
                    wallet = %req.wallet_address,
                    rejection_code = "SINGLE_WALLET_UNPROVEN",
                    reason = %reason,
                    strategy = ?strategy,
                    is_consensus,
                    is_smart_money_cluster,
                    profitable_cluster_count,
                    "selection: BUY rejected by gate"
                );
                return BuyDecision::rejected(
                    req,
                    &self.config_hash,
                    "SINGLE_WALLET_UNPROVEN",
                    reason,
                );
            }
        }

        // ── 7c. Shadow-mirror token gate ───────────────────────────────────
        // Token-level EV: only admit tokens whose whale round-trips (shadow
        // `mirror_main`, rolling window) clear the post-cost breakeven.
        // Verified 2026-08-06 on 48h shadow data: negative-mirror tokens
        // average -1.82% (est -3.2% net of ~1.4% costs) vs positive-mirror
        // tokens +2.73% (est +1.3% net). Insufficient data fails closed but
        // the handler routes those signals to entry confirmation, where a
        // price-hold provides the admission evidence (the gate is bypassed in
        // the confirmation re-decision).
        if !bypass_consensus_proven && self.config.mirror_gate_enabled {
            // Sample-floor carve-out (2026-08-26): age-trial tokens are fresh
            // by construction — they can never hold the full sample floor.
            // Lower it for them (negative averages still reject below).
            let effective_min_samples = if age_trial_cap.is_some()
                && self.config.mirror_gate_trial_min_samples > 0
                && self.config.mirror_gate_trial_min_samples < self.config.mirror_gate_min_samples
            {
                self.config.mirror_gate_trial_min_samples
            } else {
                self.config.mirror_gate_min_samples
            };
            let avg = match self
                .db
                .get_token_mirror_avg_pnl(
                    &req.token_address,
                    self.config.mirror_gate_window_hours,
                    effective_min_samples,
                )
                .await
            {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!(
                        token = %req.token_address,
                        error = %e,
                        "Mirror-gate check failed — treating as insufficient evidence (fail-closed)"
                    );
                    None
                }
            };
            match avg {
                Some(avg) if avg < self.config.mirror_gate_min_avg_pct => {
                    let reason = format!(
                        "Token shadow-mirror avg {:.2}% below minimum {:.2}% ({} samples, {}h window)",
                        avg,
                        self.config.mirror_gate_min_avg_pct,
                        effective_min_samples,
                        self.config.mirror_gate_window_hours
                    );
                    tracing::info!(
                        ingress = ?req.ingress,
                        decision = "BUY",
                        token = %req.token_address,
                        wallet = %req.wallet_address,
                        rejection_code = "SHADOW_MIRROR_NEGATIVE",
                        reason = %reason,
                        avg_pnl_pct = %avg,
                        "selection: BUY rejected by gate"
                    );
                    return BuyDecision::rejected(
                        req,
                        &self.config_hash,
                        "SHADOW_MIRROR_NEGATIVE",
                        reason,
                    );
                }
                Some(_) => {}
                None => {
                    // Momentum bypass (2026-08-11): if the token's price_cache
                    // shows positive momentum, bypass the shadow-mirror sample
                    // requirement. Tokens with sustained upward price trend have
                    // proven themselves — the momentum IS the evidence.
                    // Disabled by default since 2026-08-14: price-cache
                    // history for fresh tokens is seconds old, so this
                    // admitted late entries into in-progress pumps.
                    let mut momentum_bypassed = false;
                    if self.config.momentum_bypass_enabled {
                        if let Some(ref price_cache) = self.price_cache {
                            let history = price_cache.price_history_read();
                            if let Some(token_history) = history.get(&req.token_address) {
                                if token_history.len() >= 3 {
                                    let latest = token_history.back().map(|(_, p)| *p);
                                    let oldest = token_history.front().map(|(_, p)| *p);
                                    if let (Some(latest), Some(oldest)) = (latest, oldest) {
                                        if oldest > Decimal::ZERO {
                                            let momentum_pct =
                                                ((latest - oldest) / oldest) * Decimal::from(100);
                                            if momentum_pct >= self.config.momentum_bypass_min_pct {
                                                momentum_bypassed = true;
                                                tracing::info!(
                                                    ingress = ?req.ingress,
                                                    decision = "BUY",
                                                    token = %req.token_address,
                                                    wallet = %req.wallet_address,
                                                    momentum_pct = %momentum_pct,
                                                    "Shadow-mirror gate bypassed: token has positive price-cache momentum ({} samples)",
                                                    token_history.len()
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if !momentum_bypassed {
                        let reason = format!(
                            "Token has <{} shadow-mirror samples in {}h — insufficient evidence",
                            effective_min_samples,
                            self.config.mirror_gate_window_hours
                        );
                        tracing::info!(
                            ingress = ?req.ingress,
                            decision = "BUY",
                            token = %req.token_address,
                            wallet = %req.wallet_address,
                            rejection_code = "SHADOW_MIRROR_INSUFFICIENT",
                            reason = %reason,
                            effective_min_samples = effective_min_samples,
                            age_trial = age_trial_cap.is_some(),
                            "selection: BUY rejected by gate"
                        );
                        return BuyDecision::rejected(
                            req,
                            &self.config_hash,
                            "SHADOW_MIRROR_INSUFFICIENT",
                            reason,
                        );
                    }
                }
            }
        }

        // ── 7d. Token liquidity-velocity gate ───────────────────────────────
        // For pump.fun bonding-curve tokens only: reject (a) tokens in the
        // late-curve dump zone (completion > max — depth discontinuity at
        // graduation makes pre-graduation dumping always more profitable) and
        // (b) tokens with slow, fragmented accumulation (velocity < min).
        // Research: "liquidity velocity is the single most informative
        // predictor of graduation" (arxiv 2602.14860). Fail-open on RPC
        // errors / non-pump tokens — the mirror + confirmation gates still
        // protect. Bypassed in the entry-confirmation re-decision (the
        // price-hold supplies the entry evidence instead).
        if !bypass_consensus_proven
            && self.config.token_velocity_gate_enabled
            && crate::token::is_pumpfun_token(&req.token_address)
        {
            let parser = self.token_parser.clone();
            match parser.get_bonding_curve_state(&req.token_address).await {
                Ok(Some(curve)) if !curve.complete => {
                    let completion = curve.completion_pct();
                    if completion > self.config.token_max_curve_completion {
                        let reason = format!(
                            "Token in late bonding-curve dump zone ({:.1}% complete > {:.0}%) — depth discontinuity at graduation",
                            completion * 100.0,
                            self.config.token_max_curve_completion * 100.0
                        );
                        tracing::info!(
                            ingress = ?req.ingress,
                            decision = "BUY",
                            token = %req.token_address,
                            rejection_code = "BONDING_CURVE_DUMP_ZONE",
                            reason = %reason,
                            completion_pct = completion,
                            "selection: BUY rejected by gate"
                        );
                        return BuyDecision::rejected(
                            req,
                            &self.config_hash,
                            "BONDING_CURVE_DUMP_ZONE",
                            reason,
                        );
                    }
                    if let Ok(swap_count) = parser
                        .get_bonding_curve_swap_count(&req.token_address, 1000)
                        .await
                    {
                        let velocity = curve.liquidity_velocity(swap_count);
                        if velocity < self.config.token_min_liquidity_velocity {
                            let reason = format!(
                                "Slow fragmented accumulation: {:.3} SOL/trade over {swap_count} swaps (min {:.2})",
                                velocity, self.config.token_min_liquidity_velocity
                            );
                            tracing::info!(
                                ingress = ?req.ingress,
                                decision = "BUY",
                                token = %req.token_address,
                                rejection_code = "LOW_LIQUIDITY_VELOCITY",
                                reason = %reason,
                                velocity_sol_per_trade = velocity,
                                swap_count,
                                "selection: BUY rejected by gate"
                            );
                            return BuyDecision::rejected(
                                req,
                                &self.config_hash,
                                "LOW_LIQUIDITY_VELOCITY",
                                reason,
                            );
                        }
                    }
                }
                Ok(_) => {
                    // Graduated or non-curve token — velocity gate doesn't apply.
                }
                Err(e) => {
                    tracing::debug!(
                        token = %req.token_address,
                        error = %e,
                        "Velocity gate: curve fetch failed — skipping (fail-open)"
                    );
                }
            }
        }

        // ── 7e. Stop-loss re-entry cooldown (2026-08-08) ───────────────────
        // After losing >= stop_loss_cooldown_loss_pct on a token, block new
        // BUYs for the cooldown window. Re-buying the next pump cycle of a
        // dying token was the pattern behind 3 of 9 losses on 2026-08-08
        // (9p84TE2Z: -3.7%, -12.2% re-entries; shadow closed at -83%).
        // Applies in every path including the entry-confirmation re-decision.
        if self.config.stop_loss_cooldown_enabled {
            match self
                .db
                .has_recent_net_loss(
                    &req.token_address,
                    self.config.stop_loss_cooldown_hours,
                    self.config.stop_loss_cooldown_loss_pct,
                )
                .await
            {
                Ok(true) => {
                    let reason = format!(
                        "Token lost >= {:.0}% net within the last {}h — re-entry cooldown active",
                        self.config.stop_loss_cooldown_loss_pct,
                        self.config.stop_loss_cooldown_hours
                    );
                    tracing::info!(
                        ingress = ?req.ingress,
                        decision = "BUY",
                        token = %req.token_address,
                        wallet = %req.wallet_address,
                        rejection_code = "STOP_LOSS_COOLDOWN",
                        reason = %reason,
                        cooldown_hours = self.config.stop_loss_cooldown_hours,
                        loss_threshold_pct = %self.config.stop_loss_cooldown_loss_pct,
                        "selection: BUY rejected by gate"
                    );
                    return BuyDecision::rejected(
                        req,
                        &self.config_hash,
                        "STOP_LOSS_COOLDOWN",
                        reason,
                    );
                }
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(
                        token = %req.token_address,
                        error = %e,
                        "Re-entry cooldown check failed — failing open"
                    );
                }
            }
        }

        // ── 7f. Whale averaging-down gate (2026-08-08) ─────────────────────
        // A wallet whose buys on a token are each lower than the previous is
        // averaging into a falling knife. Verified: the -9.4% and -12.2%
        // losers were entered after the whale's 3rd-4th buy into tokens that
        // shadow-closed at -83%..-99%. Pyramiding-up whales (each buy higher)
        // are strong signals and pass. Applies in every path.
        if self.config.averaging_down_enabled {
            let whale_buys = match self
                .db
                .get_whale_buy_prices(
                    &req.wallet_address,
                    &req.token_address,
                    self.config.averaging_down_window_hours,
                )
                .await
            {
                Ok(buys) => buys,
                Err(e) => {
                    tracing::warn!(
                        wallet = %req.wallet_address,
                        token = %req.token_address,
                        error = %e,
                        "Averaging-down check failed — failing open"
                    );
                    Vec::new()
                }
            };
            if is_averaging_down(
                &whale_buys,
                self.config.averaging_down_min_buys,
                self.config.averaging_down_min_drop_pct,
            ) {
                let first = whale_buys.first().copied().unwrap_or(Decimal::ZERO);
                let last = whale_buys.last().copied().unwrap_or(Decimal::ZERO);
                let drop_pct = if first > Decimal::ZERO {
                    ((first - last) / first) * Decimal::from(100)
                } else {
                    Decimal::ZERO
                };
                let reason = format!(
                    "Whale averaging down: {} prior buys on this token in {}h, latest buy {:.2}% below first — catching a falling knife",
                    whale_buys.len(),
                    self.config.averaging_down_window_hours,
                    drop_pct
                );
                tracing::info!(
                    ingress = ?req.ingress,
                    decision = "BUY",
                    token = %req.token_address,
                    wallet = %req.wallet_address,
                    rejection_code = "WHALE_AVERAGING_DOWN",
                    reason = %reason,
                    prior_buys = whale_buys.len(),
                    first_buy_price = %first,
                    latest_buy_price = %last,
                    drop_pct = %drop_pct,
                    "selection: BUY rejected by gate"
                );
                return BuyDecision::rejected(
                    req,
                    &self.config_hash,
                    "WHALE_AVERAGING_DOWN",
                    reason,
                );
            }
        }

        // ── 7g. Pump-chase gate (2026-08-08) ───────────────────────────────
        // Reject BUYs on tokens already up more than the threshold over 15m —
        // buying the top of a fresh pump. Consensus / smart-money cluster
        // signals are exempt (crowd support can carry the move). Fail-open
        // when no price history exists (young tokens). Verified: DcdNm2UX was
        // entered at +15.6%/15m — the top — and stopped out minutes later.
        if self.config.pump_chase_enabled && !is_consensus && !is_smart_money_cluster {
            if let Some(ref price_cache) = self.price_cache {
                let history = price_cache.price_history_read();
                if let Some(token_history) = history.get(&req.token_address) {
                    let fifteen_min_ago = chrono::Utc::now() - chrono::Duration::minutes(15);
                    let mut price_15m_ago: Option<Decimal> = None;
                    let mut current_token_price: Option<Decimal> = None;
                    for (timestamp, price) in token_history.iter().rev() {
                        if current_token_price.is_none() {
                            current_token_price = Some(*price);
                        }
                        if *timestamp <= fifteen_min_ago && price_15m_ago.is_none() {
                            price_15m_ago = Some(*price);
                            break;
                        }
                    }
                    if let (Some(old_price), Some(new_price)) = (price_15m_ago, current_token_price)
                    {
                        if old_price > Decimal::ZERO {
                            let delta_pct =
                                ((new_price - old_price) / old_price) * Decimal::from(100);
                            if delta_pct > self.config.pump_chase_max_delta_pct {
                                let reason = format!(
                                    "Token up {:.1}% in 15m — buying the top of a fresh pump (max {:.0}%)",
                                    delta_pct, self.config.pump_chase_max_delta_pct
                                );
                                tracing::info!(
                                    ingress = ?req.ingress,
                                    decision = "BUY",
                                    token = %req.token_address,
                                    wallet = %req.wallet_address,
                                    rejection_code = "PUMP_CHASE",
                                    reason = %reason,
                                    delta_15m_pct = %delta_pct,
                                    max_delta_pct = %self.config.pump_chase_max_delta_pct,
                                    "selection: BUY rejected by gate"
                                );
                                return BuyDecision::rejected(
                                    req,
                                    &self.config_hash,
                                    "PUMP_CHASE",
                                    reason,
                                );
                            }
                        }
                    }
                }
            }
        }

        // ── 7h. Entry-price guard (2026-08-11) ───────────────────────────────
        // Reject BUYs when the token already pumped significantly since the
        // whale's entry. The whale's swap gives an exact entry price (amount_in
        // / amount_out in SOL per raw token unit); comparing it to the current
        // price-cache price tells us how much the copier would overpay. Fails
        // open when either price is unavailable (young tokens without cache data).
        //
        // 2026-08-17: two fail-open corrections after production forensics.
        // The gate computed +185,451,383% and +52,449,789% "pumps" (minutes
        // after whale entry) for tokens the entry-confirmation path had just
        // verified as price-holding, and rejected both — the unit conversion
        // was untrustworthy, not the market:
        //   1. decimals: the price entry's decimals are often None for fresh
        //      tokens and the old `unwrap_or(6)` guess was wrong by 10^3-10^6
        //      for 9-decimal/unknown tokens. Now resolves via the dedicated
        //      decimals cache and skips the gate when unknown.
        //   2. plausibility: a computed pump >10,000% (100x) within the
        //      confirmation window indicates inconsistent inputs (decimals
        //      mismatch, Jupiter quoting an illiquid fresh pair), not a real
        //      move. Skip the gate and let the remaining gates decide.
        let mut pump_pct_computed: Option<(Decimal, u8)> = None;
        if self.config.pump_since_whale_guard_enabled || self.config.entry_drift_guard_enabled {
            if let Some(ref price_cache) = self.price_cache {
                if let Some(whale_price) = req.whale_entry_price {
                    if whale_price > Decimal::ZERO {
                        if let Some(current_entry) = price_cache.get_price(&req.token_address) {
                            let sol_usd = price_cache.get_sol_price_usd();
                            let decimals = current_entry
                                .decimals
                                .or_else(|| price_cache.get_decimals(&req.token_address))
                                .filter(|d| (2..=12).contains(d));
                            if decimals.is_none() {
                                tracing::debug!(
                                    token = %req.token_address,
                                    "Pump-since-whale guard skipped: token decimals unknown (fail-open)"
                                );
                            }
                            if let (Some(decimals), Some(sol_usd)) = (decimals, sol_usd) {
                                if sol_usd > Decimal::ZERO
                                    && current_entry.price_usd > Decimal::ZERO
                                {
                                    let mult = match 10u64.checked_pow(decimals as u32) {
                                        Some(m) => Decimal::from(m),
                                        None => Decimal::from(1_000_000),
                                    };
                                    let current_sol_per_raw =
                                        current_entry.price_usd / (sol_usd * mult);
                                    let pump_pct = ((current_sol_per_raw - whale_price)
                                        / whale_price)
                                        * Decimal::from(100);
                                    if pump_pct > Decimal::from(10_000) {
                                        tracing::warn!(
                                            token = %req.token_address,
                                            wallet = %req.wallet_address,
                                            pump_pct = %pump_pct,
                                            decimals = decimals,
                                            "Pump-since-whale guard: implausible pump (>10000%) — unit inconsistency, failing open"
                                        );
                                    } else {
                                        pump_pct_computed = Some((pump_pct, decimals));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some((pump_pct, _)) = pump_pct_computed {
            if pump_pct > self.config.max_pump_since_whale_pct {
                let reason = format!(
                    "Token already up {:.1}% since whale entry (max {:.0}%) — buying the top",
                    pump_pct, self.config.max_pump_since_whale_pct
                );
                tracing::info!(
                    ingress = ?req.ingress,
                    decision = "BUY",
                    token = %req.token_address,
                    wallet = %req.wallet_address,
                    rejection_code = "ALREADY_PUMPED",
                    reason = %reason,
                    pump_since_whale_pct = %pump_pct,
                    "selection: BUY rejected by gate"
                );
                return BuyDecision::rejected(req, &self.config_hash, "ALREADY_PUMPED", reason);
            }
        }

        // ── 7h'. Entry drift guard (2026-08-22) ────────────────────────────
        // Reject when the price has moved more than `max_entry_drift_pct` in
        // either direction since the whale's entry — the signal-time
        // reference. Closes the measured execution gap: re-admitted signals
        // (consensus wait, entry-confirmation hold, queue) filled -1.98%
        // gross / -2.99% net worse than the decision-time shadow mark because
        // they copied into matured pumps 33-310s after the signal. Uses the
        // same unit-verified comparison as the guard above; fails open with
        // it (no prices / unknown decimals / implausible conversion).
        if self.config.entry_drift_guard_enabled {
            if let Some((pump_pct, _)) = pump_pct_computed {
                let drift_pct = pump_pct.abs();
                if drift_pct > self.config.max_entry_drift_pct {
                    let reason = format!(
                        "Entry drift {:.1}% since whale entry exceeds max {:.1}% — late fill into a matured move",
                        drift_pct, self.config.max_entry_drift_pct
                    );
                    tracing::info!(
                        ingress = ?req.ingress,
                        decision = "BUY",
                        token = %req.token_address,
                        wallet = %req.wallet_address,
                        rejection_code = "ENTRY_DRIFT_EXCEEDED",
                        reason = %reason,
                        entry_drift_pct = %drift_pct,
                        max_entry_drift_pct = %self.config.max_entry_drift_pct,
                        "selection: BUY rejected by gate"
                    );
                    return BuyDecision::rejected(
                        req,
                        &self.config_hash,
                        "ENTRY_DRIFT_EXCEEDED",
                        reason,
                    );
                }
            }
        }

        // ── 7i. Repeat-signal gate (2026-08-11) ───────────────────────────────
        // One-shot tokens (single BUY signal, never traded again) have an 8%
        // win rate and generate 59% of all losses (-0.44 SOL). Repeat tokens
        // (2+ signals) have 18% win rate and +18.4% avg win move. Require at
        // least `repeat_signal_min_prior` prior shadow signals on the token.
        // Fails OPEN on DB errors (don't block trading on infra issues).
        if self.config.repeat_signal_gate_enabled && !is_consensus && !is_smart_money_cluster {
            match self
                .db
                .count_shadow_positions_by_token(&req.token_address)
                .await
            {
                Ok(prior) if prior < self.config.repeat_signal_min_prior => {
                    let reason = format!(
                        "First signal on this token ({} prior shadow signals, need {}) — shadow-trading only; wait for repeat signal to confirm genuine interest",
                        prior, self.config.repeat_signal_min_prior
                    );
                    tracing::info!(
                        ingress = ?req.ingress,
                        decision = "BUY",
                        token = %req.token_address,
                        wallet = %req.wallet_address,
                        rejection_code = "FIRST_SIGNAL_SHADOW_ONLY",
                        reason = %reason,
                        prior_shadow_count = prior,
                        "selection: BUY rejected by gate"
                    );
                    return BuyDecision::rejected(
                        req,
                        &self.config_hash,
                        "FIRST_SIGNAL_SHADOW_ONLY",
                        reason,
                    );
                }
                Ok(_) => {
                    tracing::debug!(
                        token = %req.token_address,
                        "Repeat-signal gate passed — token has prior shadow signals"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        token = %req.token_address,
                        error = %e,
                        "Repeat-signal gate DB query failed — failing open"
                    );
                }
            }
        }

        // ── 8. Signal-quality score ─────────────────────────────────────────
        let quality = SignalQuality::calculate(
            wallet_wqs,
            consensus_wallet_count,
            liquidity_usd,
            token_age_hours,
        );
        // Smart-money cluster bonus: a ≥N-wallet profitable cluster is a
        // research-backed conviction signal — small quality boost so cluster
        // signals clear tighter quality thresholds than lone signals.
        let quality = if is_smart_money_cluster {
            SignalQuality {
                score: (quality.score + 0.05).min(1.0),
                ..quality
            }
        } else {
            quality
        };
        let quality_threshold = match strategy {
            Strategy::Shield => self.config.shield_signal_quality_threshold,
            Strategy::Spear => self.config.spear_signal_quality_threshold,
            Strategy::Exit => 0.0,
        };
        if !quality.should_enter(quality_threshold) {
            let reason = format!(
                "Quality score {:.2} below threshold {:.2}",
                quality.score, quality_threshold
            );
            tracing::info!(
                ingress = ?req.ingress,
                decision = "BUY",
                token = %req.token_address,
                wallet = %req.wallet_address,
                rejection_code = "SIGNAL_QUALITY_TOO_LOW",
                reason = %reason,
                quality_score = quality.score,
                quality_threshold = quality_threshold,
                strategy = ?strategy,
                is_pumpfun = is_pumpfun,
                consensus_wallet_count = ?consensus_wallet_count,
                "selection: BUY rejected by gate"
            );
            return BuyDecision::rejected(req, &self.config_hash, "SIGNAL_QUALITY_TOO_LOW", reason);
        }

        // ── 9. Market-regime multiplier ─────────────────────────────────────
        let regime_multiplier = if let Some(ref regime) = self.market_regime {
            regime.get_regime_multiplier(&req.token_address)
        } else {
            Decimal::ONE
        };

        // ── 10. Position size via PositionSizer ─────────────────────────────
        // Per-wallet copy-performance boost: if this wallet qualifies as BOOSTED
        // (proven recent copy profitability), seed the size from the boost
        // target; otherwise None and the floor applies.
        let boost_target_sol = match self.wallet_performance {
            Some(ref tracker) => tracker.boost_target_for(&req.wallet_address).await,
            None => None,
        };
        if let Some(bt) = boost_target_sol {
            tracing::info!(
                wallet = %req.wallet_address,
                boost_target_sol = %bt,
                "Wallet qualified for copy-performance size boost"
            );
        }
        let size_sol = if let Some(ref sizer) = self.position_sizer {
            // Resolve proven-ness (2026-08-18): reuse the waiver's result
            // when already computed; otherwise query the oracle only when
            // the sizer's proven boost is enabled (avoids a DB round trip
            // per signal when the feature is off).
            let is_proven = match wallet_proven {
                Some(v) => v,
                None => {
                    if sizer.proven_boost_enabled() {
                        self.wallet_is_proven(&req.wallet_address).await
                    } else {
                        false
                    }
                }
            };
            let factors = SizingFactors {
                is_consensus,
                wallet_wqs,
                wqs_confidence,
                wallet_success_rate,
                token_age_hours,
                estimated_slippage: Decimal::ZERO,
                signal_quality: Decimal::from_f64_retain(quality.score),
                token_volatility_24h: None,
                wallet_address: req.wallet_address.clone(),
                total_capital_sol: self.config.total_capital_sol,
                strategy,
                consensus_wallet_count,
                regime_multiplier,
                // Micro-position caps: the WQS spear-lite cap for sub-threshold
                // wallets, and the token-age trial cap for trial-admitted
                // sub-floor tokens. Both may apply — take the most
                // conservative (lower) of whichever are present.
                wqs_capped_max_size: {
                    let wqs_cap = if wallet_wqs < self.config.spear_lite_wqs_threshold {
                        Some(self.config.spear_lite_max_size_sol)
                    } else {
                        None
                    };
                    match (wqs_cap, age_trial_cap) {
                        (Some(a), Some(b)) => Some(a.min(b)),
                        (a, b) => a.or(b),
                    }
                },
                boost_target_sol,
                token_address: Some(req.token_address.clone()),
                is_proven,
            };
            let size = match sizer.calculate_size(factors).await {
                Ok(s) => s,
                Err(e) => {
                    let reason =
                        format!("Position sizer failed (DB error — fail-safe reject): {}", e);
                    tracing::warn!(
                        ingress = ?req.ingress,
                        decision = "BUY",
                        token = %req.token_address,
                        wallet = %req.wallet_address,
                        rejection_code = "POSITION_SIZER_ERROR",
                        error = %e,
                        "selection: BUY rejected by gate"
                    );
                    return BuyDecision::rejected(
                        req,
                        &self.config_hash,
                        "POSITION_SIZER_ERROR",
                        reason,
                    );
                }
            };
            if size.is_zero() {
                let reason =
                    "Position sizer returned zero (strategy_max below min_size_sol)".to_string();
                tracing::info!(
                    ingress = ?req.ingress,
                    decision = "BUY",
                    token = %req.token_address,
                    wallet = %req.wallet_address,
                    rejection_code = "POSITION_SIZE_ZERO",
                    reason = %reason,
                    strategy = ?strategy,
                    quality_score = quality.score,
                    is_consensus = is_consensus,
                    regime_multiplier = %regime_multiplier,
                    "selection: BUY rejected by gate"
                );
                return BuyDecision::rejected(req, &self.config_hash, "POSITION_SIZE_ZERO", reason);
            }
            size
        } else {
            // No sizer configured — fall back to the source amount clamped to
            // the configured capital ceiling. This should not happen in
            // production; logged loudly.
            tracing::warn!(
                "PositionSizer unavailable; using source amount clamped to max_position_sol"
            );
            req.source_amount_sol.min(self.config.max_position_sol)
        };

        // ── 11. Portfolio heat + strategy-allocation heat admission ─────────
        if let Some(ref heat) = self.portfolio_heat {
            match heat.can_open_position(size_sol).await {
                Ok(false) => {
                    let reason = "Portfolio heat limit reached".to_string();
                    tracing::info!(
                        ingress = ?req.ingress,
                        decision = "BUY",
                        token = %req.token_address,
                        wallet = %req.wallet_address,
                        rejection_code = "PORTFOLIO_HEAT_LIMIT",
                        reason = %reason,
                        size_sol = %size_sol,
                        strategy = ?strategy,
                        "selection: BUY rejected by gate"
                    );
                    return BuyDecision::rejected(
                        req,
                        &self.config_hash,
                        "PORTFOLIO_HEAT_LIMIT",
                        reason,
                    );
                }
                Ok(true) => {
                    match heat
                        .can_open_strategy_position(
                            strategy,
                            size_sol,
                            self.config.shield_percent,
                            self.config.spear_percent,
                        )
                        .await
                    {
                        Ok(false) => {
                            let reason =
                                format!("Strategy allocation limit reached for {:?}", strategy);
                            tracing::info!(
                                ingress = ?req.ingress,
                                decision = "BUY",
                                token = %req.token_address,
                                wallet = %req.wallet_address,
                                rejection_code = "STRATEGY_HEAT_LIMIT",
                                reason = %reason,
                                size_sol = %size_sol,
                                strategy = ?strategy,
                                "selection: BUY rejected by gate"
                            );
                            return BuyDecision::rejected(
                                req,
                                &self.config_hash,
                                "STRATEGY_HEAT_LIMIT",
                                reason,
                            );
                        }
                        Ok(true) => {}
                        Err(e) => {
                            let reason = format!(
                                "Strategy allocation check failed — rejecting signal (fail-safe): {}",
                                e
                            );
                            tracing::warn!(
                                ingress = ?req.ingress,
                                decision = "BUY",
                                token = %req.token_address,
                                wallet = %req.wallet_address,
                                rejection_code = "STRATEGY_HEAT_ERROR",
                                reason = %reason,
                                error = %e,
                                "selection: BUY rejected by gate"
                            );
                            return BuyDecision::rejected(
                                req,
                                &self.config_hash,
                                "STRATEGY_HEAT_ERROR",
                                reason,
                            );
                        }
                    }
                }
                Err(e) => {
                    let reason = format!(
                        "Portfolio heat check failed — rejecting signal (fail-safe): {}",
                        e
                    );
                    tracing::warn!(
                        ingress = ?req.ingress,
                        decision = "BUY",
                        token = %req.token_address,
                        wallet = %req.wallet_address,
                        rejection_code = "PORTFOLIO_HEAT_ERROR",
                        reason = %reason,
                        error = %e,
                        "selection: BUY rejected by gate"
                    );
                    return BuyDecision::rejected(
                        req,
                        &self.config_hash,
                        "PORTFOLIO_HEAT_ERROR",
                        reason,
                    );
                }
            }
        }

        // ── 12. Record the admitted signal for consensus tracking ───────────
        // Only admitted signals enter the aggregator window / signal_aggregation
        // table, so rejected noise cannot drive false consensus downstream.
        if let Some(ref aggregator) = self.signal_aggregator {
            let _ = aggregator
                .add_signal(
                    &req.wallet_address,
                    &req.token_address,
                    "BUY",
                    req.source_amount_sol,
                )
                .await;
        }
        if let Err(e) = self.persist_signal_aggregation(req, is_consensus).await {
            tracing::warn!(
                error = %e,
                "Failed to record signal aggregation — consensus detection may be degraded"
            );
        }

        tracing::debug!(
            ingress = ?req.ingress,
            decision = "BUY",
            token = %req.token_address,
            wallet = %req.wallet_address,
            admitted = true,
            wallet_wqs = wallet_wqs,
            wqs_confidence = ?wqs_confidence,
            strategy = ?strategy,
            size_sol = %size_sol,
            quality_score = quality.score,
            liquidity_usd = %liquidity_usd,
            token_age_hours = ?token_age_hours,
            volume_24h_usd = ?volume_24h_usd,
            consensus_wallet_count = ?consensus_wallet_count,
            regime_multiplier = %regime_multiplier,
            is_pumpfun = is_pumpfun,
            is_consensus = is_consensus,
            "selection: BUY admitted"
        );

        BuyDecision {
            decision_id: uuid::Uuid::new_v4().to_string(),
            admitted: true,
            rejection_reason: None,
            rejection_code: None,
            strategy: Some(strategy),
            size_sol: Some(size_sol),
            source_amount_sol: req.source_amount_sol,
            wqs: Some(wallet_wqs),
            wqs_confidence,
            quality_score: Some(quality.score),
            consensus_wallet_count,
            regime_multiplier: Some(regime_multiplier),
            token_age_hours,
            liquidity_usd: Some(liquidity_usd),
            volume_24h_usd, // B3: DexScreener feed
            price_impact_pct: None,
            config_hash: self.config_hash.clone(),
            ingress: req.ingress,
            is_consensus,
            fast_check_errored,
        }
    }

    /// A wallet is "proven" when its copy-trade ledger shows >=
    /// `min_proven_trades` closed trades and (when `require_proven_positive_pnl`)
    /// positive realized net PnL, per the live `trades` table. Fails closed on
    /// any error — an unverifiable wallet must not bypass the gate.
    async fn wallet_is_proven(&self, wallet_address: &str) -> bool {
        let proven = self.wallet_is_proven_base(wallet_address).await;
        if !proven {
            return false;
        }

        // Recency-weighted overlay (2026-08-24): every proven path above
        // (shadow-total, t-stat, ledger) aggregates over long windows that go
        // stale silently — ArcebCcX kept solo-admitting on a positive
        // all-time ledger while its trailing week ran −0.24 SOL. A wallet is
        // only *currently* proven if its most recent `proven_recency_trades`
        // closed copy-trades are not net-negative. Thin histories (fewer
        // trades than the window) can't be judged and pass through; zero
        // disables; errors fail-open.
        if self.config.proven_recency_trades > 0 {
            match self
                .db
                .get_wallet_recency_stats(wallet_address, self.config.proven_recency_trades)
                .await
            {
                Ok((recent_n, net_recent))
                    if recent_n >= self.config.proven_recency_trades
                        && net_recent < Decimal::ZERO =>
                {
                    // Shadow escape hatch (2026-08-24): a live-ledger block
                    // is otherwise unrecoverable — the wallet cannot trade
                    // to rebuild its live form, and blocking it also stops
                    // the very copy flow the shadow book keeps scoring
                    // (shadow evaluates every signal regardless of live
                    // blocks). When the wallet's trailing-7d deduped shadow
                    // mirror_main form is positive with enough samples,
                    // trust that evidence and waive the overlay. Prod
                    // evidence at decision time: 8MPy8CXZ +6.58%/116 exits,
                    // ArcebCcX +2.65%/29.
                    const SHADOW_ESCAPE_WINDOW_DAYS: i32 = 7;
                    const SHADOW_ESCAPE_MIN_SAMPLES: i64 = 3;
                    match self
                        .db
                        .get_wallet_pnl_statistics(wallet_address, SHADOW_ESCAPE_WINDOW_DAYS)
                        .await
                    {
                        Ok(Some((n, mean, _)))
                            if n >= SHADOW_ESCAPE_MIN_SAMPLES && mean > Decimal::ZERO =>
                        {
                            tracing::info!(
                                wallet = wallet_address,
                                recent_n,
                                net_recent = %net_recent,
                                shadow_n = n,
                                shadow_mean_pct = %mean,
                                "Recency overlay waived: live form negative but trailing shadow mirror_main positive"
                            );
                        }
                        _ => {
                            tracing::debug!(
                                wallet = wallet_address,
                                recent_n,
                                proven_recency_trades =
                                    self.config.proven_recency_trades,
                                net_recent = %net_recent,
                                "Proven-wallet check: recent form negative and no positive shadow evidence — treating as unproven"
                            );
                            return false;
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(
                        wallet = wallet_address,
                        error = %e,
                        "Recency check failed — keeping proven status (fail-open)"
                    );
                }
            }
        }
        true
    }

    /// Base proven evaluation without the recency overlay (see
    /// `wallet_is_proven`): shadow-total-PnL OR shadow-t-stat OR the
    /// closed-trade ledger fallback.
    async fn wallet_is_proven_base(&self, wallet_address: &str) -> bool {
        // Shadow total-PnL proven (2026-08-13): admits high-variance "moonshot"
        // wallets (big total shadow PnL, low t-stat) the paths below reject.
        // OR'd first so it short-circuits; no-op when shadow_proven_enabled=false.
        if self.wallet_is_shadow_total_proven(wallet_address).await {
            return true;
        }
        // Research-backed criterion (arxiv 2601.08641): wallet selection is
        // the dominant factor in copier profitability. Only wallets whose
        // shadow mirror_main PnL is STATISTICALLY significant (t > threshold)
        // are proven. Falls back to the live-trades ledger check when the
        // t-stat gate is disabled (e.g. during shadow A/B calibration).
        if self.config.wallet_tstat_enabled {
            return self.wallet_has_significant_pnl(wallet_address).await;
        }
        let (closed_trades, net_pnl_sol) = match self.db.get_wallet_copy_stats(wallet_address).await
        {
            Ok(stats) => stats,
            Err(e) => {
                tracing::warn!(
                    wallet = wallet_address,
                    error = %e,
                    "Proven-wallet check failed — treating as unproven (fail-closed)"
                );
                return false;
            }
        };
        if closed_trades < self.config.min_proven_trades as i64 {
            tracing::debug!(
                wallet = wallet_address,
                closed_trades,
                min_proven_trades = self.config.min_proven_trades,
                "Proven-wallet check: too few closed copy-trades"
            );
            return false;
        }
        if self.config.require_proven_positive_pnl && net_pnl_sol <= Decimal::ZERO {
            tracing::debug!(
                wallet = wallet_address,
                closed_trades,
                net_pnl_sol = %net_pnl_sol,
                "Proven-wallet check: realized copy PnL not positive"
            );
            return false;
        }
        tracing::debug!(
            wallet = wallet_address,
            closed_trades,
            net_pnl_sol = %net_pnl_sol,
            "Proven-wallet check passed"
        );
        true
    }

    /// T-statistic significance test on a wallet's shadow mirror_main PnL:
    /// t = mean / (stddev / sqrt(n)). Requires t > threshold with at least
    /// `wallet_tstat_min_samples` exits in the window. Fail-closed on any
    /// error, missing data, non-positive mean, or insufficient samples.
    async fn wallet_has_significant_pnl(&self, wallet_address: &str) -> bool {
        let stats = match self
            .db
            .get_wallet_pnl_statistics(wallet_address, self.config.wallet_tstat_window_days)
            .await
        {
            Ok(Some(s)) => s,
            Ok(None) => {
                tracing::debug!(
                    wallet = wallet_address,
                    "T-stat check: no shadow mirror_main data in window — unproven"
                );
                return false;
            }
            Err(e) => {
                tracing::warn!(
                    wallet = wallet_address,
                    error = %e,
                    "T-stat check failed — treating as unproven (fail-closed)"
                );
                return false;
            }
        };
        let (n, mean, stddev) = stats;
        if n < self.config.wallet_tstat_min_samples as i64 {
            tracing::debug!(
                wallet = wallet_address,
                n,
                min_samples = self.config.wallet_tstat_min_samples,
                "T-stat check: insufficient shadow samples"
            );
            return false;
        }
        if mean <= Decimal::ZERO {
            tracing::debug!(
                wallet = wallet_address,
                n,
                mean = %mean,
                "T-stat check: mean PnL not positive"
            );
            return false;
        }
        let t = if stddev > Decimal::ZERO {
            let se = stddev / Decimal::from_f64((n as f64).sqrt()).unwrap_or(Decimal::ONE);
            (mean / se).to_f64().unwrap_or(0.0)
        } else {
            // Zero variance with positive mean = perfectly consistent.
            f64::INFINITY
        };
        let passes = t > self.config.wallet_tstat_threshold;
        tracing::debug!(
            wallet = wallet_address,
            n,
            mean = %mean,
            stddev = %stddev,
            t_statistic = t,
            threshold = self.config.wallet_tstat_threshold,
            passes,
            "T-stat check"
        );
        passes
    }

    /// Shadow total-PnL proven check (2026-08-13): a wallet counts as proven if
    /// its `mirror_main` shadow exits in the window total >=
    /// `shadow_proven_min_total_pnl_sol` over >= `shadow_proven_min_samples`
    /// exits. Unlike the t-stat gate, this captures high-variance "moonshot"
    /// wallets whose edge is real in total PnL but not statistically significant
    /// by t (huge std from rare large winners). Reuses `get_wallet_pnl_statistics`
    /// (total PnL ≈ mean × n). Fail-closed on any error or missing data.
    async fn wallet_is_shadow_total_proven(&self, wallet_address: &str) -> bool {
        if !self.config.shadow_proven_enabled {
            return false;
        }
        let stats = match self
            .db
            .get_wallet_pnl_statistics(wallet_address, self.config.wallet_tstat_window_days)
            .await
        {
            Ok(Some(s)) => s,
            Ok(None) => return false,
            Err(e) => {
                tracing::warn!(
                    wallet = wallet_address,
                    error = %e,
                    "Shadow total-PnL proven check failed — treating as unproven (fail-closed)"
                );
                return false;
            }
        };
        let (n, mean, _stddev) = stats;
        if n < self.config.shadow_proven_min_samples as i64 || mean <= Decimal::ZERO {
            return false;
        }
        let total = mean * Decimal::from(n);
        let min_total =
            Decimal::from_f64(self.config.shadow_proven_min_total_pnl_sol).unwrap_or(Decimal::ZERO);
        let passes = total >= min_total;
        if passes {
            tracing::info!(
                wallet = wallet_address,
                n,
                mean = %mean,
                total_pnl = %total,
                min_total_pnl = %min_total,
                "Shadow total-PnL proven check passed — moonshot wallet admitted"
            );
        }
        passes
    }

    /// Persist a BUY signal to the signal_aggregation table so the stop-loss
    /// manager's consensus detection works uniformly for both ingress paths.
    async fn persist_signal_aggregation(
        &self,
        req: &SelectionRequest,
        is_consensus: bool,
    ) -> Result<(), crate::error::AppError> {
        use crate::db_abstraction::DbPool;
        let DbPool::PostgreSQL(pool) = self.db.pool();
        let amount_f64 = req.source_amount_sol.to_f64().unwrap_or(0.0);
        sqlx::query(
            r#"
            INSERT INTO signal_aggregation
                (token_address, wallet_address, direction, amount_sol, is_consensus)
            VALUES ($1, $2, 'BUY', $3, $4)
            "#,
        )
        .bind(&req.token_address)
        .bind(&req.wallet_address)
        .bind(amount_f64)
        .bind(is_consensus)
        .execute(&pool)
        .await
        .map_err(crate::error::AppError::Database)?;
        Ok(())
    }
}

/// True when the whale's buy sequence shows the averaging-down signature:
/// at least `min_buys` prior buys, and the LATEST buy at least
/// `min_drop_pct` below the FIRST — the whale keeps buying into a falling
/// price. Pyramiding-up sequences (each buy higher) are strong signals and
/// never rejected. `prices` must be in ascending time order (as returned by
/// `get_whale_buy_prices`).
pub fn is_averaging_down(prices: &[Decimal], min_buys: usize, min_drop_pct: Decimal) -> bool {
    if prices.len() < min_buys || min_buys == 0 {
        return false;
    }
    let Some(first) = prices.first() else {
        return false;
    };
    let Some(latest) = prices.last() else {
        return false;
    };
    if *first <= Decimal::ZERO || *latest <= Decimal::ZERO || min_drop_pct < Decimal::ZERO {
        return false;
    }
    let drop_pct = ((*first - *latest) / *first) * Decimal::from(100);
    drop_pct >= min_drop_pct
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn averaging_down_rejects_falling_knife() {
        // Whale bought at 1.0, 0.9, 0.8 — each buy lower: catching a falling knife.
        let prices = vec![
            Decimal::from_str("1.0").unwrap(),
            Decimal::from_str("0.9").unwrap(),
            Decimal::from_str("0.8").unwrap(),
        ];
        assert!(
            is_averaging_down(&prices, 2, Decimal::from(3)),
            "latest 20% below first with >=2 buys must be flagged"
        );
    }

    #[test]
    fn averaging_down_requires_min_buys() {
        let prices = vec![
            Decimal::from_str("1.0").unwrap(),
            Decimal::from_str("0.5").unwrap(),
        ];
        assert!(
            !is_averaging_down(&prices, 3, Decimal::from(3)),
            "fewer buys than min_buys must not be flagged"
        );
        assert!(
            is_averaging_down(&prices, 2, Decimal::from(3)),
            "2 buys with a 50% drop must be flagged"
        );
    }

    #[test]
    fn averaging_down_allows_pyramiding_up() {
        // Each buy HIGHER — accumulation into strength, never flagged.
        let prices = vec![
            Decimal::from_str("1.0").unwrap(),
            Decimal::from_str("1.1").unwrap(),
            Decimal::from_str("1.2").unwrap(),
        ];
        assert!(
            !is_averaging_down(&prices, 2, Decimal::from(3)),
            "pyramiding up is a strong signal, must pass"
        );
    }

    #[test]
    fn averaging_down_ignores_small_drawdowns() {
        let prices = vec![
            Decimal::from_str("1.0").unwrap(),
            Decimal::from_str("0.98").unwrap(),
        ];
        assert!(
            !is_averaging_down(&prices, 2, Decimal::from(3)),
            "2% drop below the 3% threshold must pass"
        );
    }

    #[test]
    fn averaging_down_guards_bad_input() {
        assert!(!is_averaging_down(&[], 2, Decimal::from(3)));
        assert!(!is_averaging_down(&[Decimal::ZERO], 1, Decimal::from(3)));
        assert!(!is_averaging_down(
            &[Decimal::ONE, Decimal::from(2)],
            2,
            Decimal::from(-3)
        ));
    }

    #[test]
    fn selection_config_hash_changes_with_pumpfun_fields() {
        let mut config1 = SelectionConfig {
            total_capital_sol: Decimal::from(10),
            max_position_sol: Decimal::from(5),
            shield_signal_quality_threshold: 0.55,
            spear_signal_quality_threshold: 0.30,
            shield_percent: 60,
            spear_percent: 40,
            min_liquidity_shield_usd: Decimal::from(10000),
            min_liquidity_spear_usd: Decimal::from(5000),
            min_liquidity_pumpfun_usd: Decimal::from(25000),
            allow_graduated_pumpfun: true,
            min_token_age_hours: 1.0,
            min_token_age_pumpfun_hours: 1.0,
            min_token_age_proven_hours: 0.1,
            min_wqs_score: 70.0,
            spear_lite_max_size_sol: Decimal::new(10, 2), // 0.10 SOL
            spear_lite_wqs_threshold: 40.0,
            require_consensus_or_proven: true,
            min_proven_trades: 10,
            require_proven_positive_pnl: true,
            mirror_gate_enabled: true,
            mirror_gate_min_avg_pct: Decimal::new(15, 1), // 1.5%
            mirror_gate_min_samples: 10,
            mirror_gate_window_hours: 48,
            mirror_gate_trial_min_samples: 3,
            wallet_tstat_enabled: true,
            wallet_tstat_threshold: 1.645,
            wallet_tstat_min_samples: 10,
            wallet_tstat_window_days: 30,
            shadow_proven_enabled: true,
            shadow_proven_min_samples: 20,
            shadow_proven_min_total_pnl_sol: 2.0,
            token_velocity_gate_enabled: false,
            token_min_liquidity_velocity: 0.10,
            token_max_curve_completion: 0.85,
            cluster_gate_enabled: true,
            cluster_min_profitable_wallets: 3,
            averaging_down_enabled: true,
            averaging_down_window_hours: 12,
            averaging_down_min_buys: 2,
            averaging_down_min_drop_pct: Decimal::new(3, 0),
            pump_chase_enabled: true,
            pump_chase_max_delta_pct: Decimal::new(10, 0),
            stop_loss_cooldown_enabled: true,
            stop_loss_cooldown_hours: 12,
            stop_loss_cooldown_loss_pct: Decimal::new(5, 0),
            pump_since_whale_guard_enabled: true,
            max_pump_since_whale_pct: rust_decimal::Decimal::new(15, 0),
            repeat_signal_gate_enabled: true,
            repeat_signal_min_prior: 1,
            entry_drift_guard_enabled: true,
            max_entry_drift_pct: rust_decimal::Decimal::new(30, 1),
            wqs_trial_enabled: false,
            wqs_trial_min_score: 10.0,
            proven_recency_trades: 0,
            token_age_trial_enabled: false,
            token_age_trial_max_size_sol: Decimal::new(25, 2), // 0.25 SOL
            wallet_loss_pause_enabled: true,
            wallet_loss_pause_max_loss_sol: Decimal::new(15, 2), // 0.15 SOL
            wallet_loss_pause_window_hours: 24,
            momentum_bypass_min_pct: rust_decimal::Decimal::new(3, 0),
            momentum_bypass_enabled: false,
            wqs_proven_waiver_enabled: true,
        };

        let mut config2 = config1.clone();
        assert_eq!(
            config1.hash(),
            config2.hash(),
            "identical configs should hash identically"
        );

        config2.allow_graduated_pumpfun = false;
        assert_ne!(
            config1.hash(),
            config2.hash(),
            "changing allow_graduated_pumpfun should change hash"
        );

        config2 = config1.clone();
        config2.min_liquidity_pumpfun_usd = Decimal::from(50000);
        assert_ne!(
            config1.hash(),
            config2.hash(),
            "changing min_liquidity_pumpfun_usd should change hash"
        );

        config2 = config1.clone();
        config2.require_consensus_or_proven = false;
        assert_ne!(
            config1.hash(),
            config2.hash(),
            "changing require_consensus_or_proven should change hash"
        );
    }
}
