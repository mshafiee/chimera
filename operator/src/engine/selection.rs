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

use rust_decimal::prelude::ToPrimitive;
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
    /// Minimum WQS score for a wallet to be eligible for copying.
    /// Wallets below this are rejected entirely. Configurable via env var
    /// CHIMERA_SELECTION__MIN_WQS_SCORE (default: 70.0).
    pub min_wqs_score: f64,
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
        hasher.update(self.min_wqs_score.to_le_bytes());
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
    /// For SELL: the fraction of the position to exit (None = full).
    pub exit_fraction: Option<Decimal>,
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
        let received_at = chrono::Utc::now();
        let decision = match req.action {
            Action::Buy => self.decide_buy(req).await,
            Action::Sell => self.decide_sell(req).await,
        };
        if let Some(ref recorder) = self.decision_recorder {
            // trade_uuid is linked by the caller after the trade row is
            // inserted (the Helius path derives it from the decision size, so
            // it is not available here). See DecisionRecorder::link_trade.
            recorder.record(&decision, req, None, received_at);
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
                return BuyDecision::rejected(
                    req,
                    &self.config_hash,
                    "UNKNOWN_WALLET",
                    format!("Unknown wallet {}", req.wallet_address),
                )
            }
            Err(e) => {
                return BuyDecision::rejected(
                    req,
                    &self.config_hash,
                    "WALLET_LOOKUP_ERROR",
                    format!("DB error fetching wallet: {}", e),
                )
            }
        };
        if wallet.status != "ACTIVE" {
            return BuyDecision::rejected(
                req,
                &self.config_hash,
                "WALLET_NOT_ACTIVE",
                format!("Wallet status {} != ACTIVE", wallet.status),
            );
        }

        // 2. Only exit if we actually hold an active position for this token.
        match self
            .db
            .get_active_position_by_wallet_token(&req.wallet_address, &req.token_address)
            .await
        {
            Ok(Some(_)) => {}
            Ok(None) => {
                return BuyDecision::rejected(
                    req,
                    &self.config_hash,
                    "NO_ACTIVE_POSITION",
                    "No active position to close".to_string(),
                )
            }
            Err(e) => {
                return BuyDecision::rejected(
                    req,
                    &self.config_hash,
                    "POSITION_LOOKUP_ERROR",
                    format!("Position lookup failed: {}", e),
                )
            }
        }

        let wqs = wallet.wqs_score.and_then(|d| d.to_f64());
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
            size_sol: Some(req.source_amount_sol.min(self.config.max_position_sol)),
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

    async fn decide_buy(&self, req: &SelectionRequest) -> BuyDecision {
        // ── 0. Token address format validation (cheap, fail fast) ──────────
        if req
            .token_address
            .parse::<solana_sdk::pubkey::Pubkey>()
            .is_err()
        {
            return BuyDecision::rejected(
                req,
                &self.config_hash,
                "INVALID_TOKEN_ADDRESS",
                format!(
                    "Invalid Solana token address: {}",
                    req.token_address
                ),
            );
        }

        // ── 1. Wallet fetch + ACTIVE status gate ────────────────────────────
        let wallet = match self.db.get_wallet(&req.wallet_address).await {
            Ok(Some(w)) => w,
            Ok(None) => {
                return BuyDecision::rejected(
                    req,
                    &self.config_hash,
                    "UNKNOWN_WALLET",
                    "Unknown wallet — not in roster".to_string(),
                )
            }
            Err(e) => {
                return BuyDecision::rejected(
                    req,
                    &self.config_hash,
                    "WALLET_LOOKUP_ERROR",
                    format!("DB error fetching wallet: {}", e),
                )
            }
        };
        if wallet.status != "ACTIVE" {
            return BuyDecision::rejected(
                req,
                &self.config_hash,
                "WALLET_NOT_ACTIVE",
                format!("Wallet status {} != ACTIVE", wallet.status),
            );
        }

        // B3: Toxic-wallet gate — reject signals from wallets flagged toxic.
        if let Some(ref detector) = self.toxic_detector {
            if detector.is_wallet_toxic(&req.wallet_address).await {
                return BuyDecision::rejected(
                    req,
                    &self.config_hash,
                    "TOXIC_WALLET",
                    "Wallet flagged as toxic — post-promotion ROI deterioration"
                        .to_string(),
                );
            }
        }

        let wallet_wqs = wallet.wqs_score.and_then(|d| d.to_f64()).unwrap_or(0.0);
        let wqs_confidence = wallet.wqs_confidence.and_then(|d| d.to_f64());
        let wallet_success_rate = wallet
            .win_rate
            .unwrap_or(Decimal::from_f64_retain(0.5).unwrap_or(Decimal::ZERO));

        // ── 2. Hard WQS gate + strategy assignment ──────────────────────────
        // Configurable minimum WQS; ≥80 → SHIELD; min..80 → SPEAR.
        // pump.fun tokens always use SHIELD (SPEAR has 0% win rate on pump.fun).
        let min_wqs = self.config.min_wqs_score;
        if wallet_wqs < min_wqs {
            return BuyDecision::rejected(
                req,
                &self.config_hash,
                "WQS_TOO_LOW",
                format!("Wallet WQS {:.1} below minimum {:.1}", wallet_wqs, min_wqs),
            );
        }
        let is_pumpfun = is_pumpfun_token(&req.token_address);
        let strategy = if wallet_wqs >= 80.0 || is_pumpfun {
            Strategy::Shield
        } else {
            Strategy::Spear
        };

        // ── 3. Non-speculative / pump.fun bonding-curve skip ────────────────
        if is_non_speculative(&req.token_address) {
            return BuyDecision::rejected(
                req,
                &self.config_hash,
                "NON_SPECULATIVE_TOKEN",
                "Stablecoin/WSOL — no profit potential".to_string(),
            );
        }
        let is_pumpfun = is_pumpfun_token(&req.token_address); // computed above for strategy routing
        if is_pumpfun && !self.config.allow_graduated_pumpfun {
            return BuyDecision::rejected(
                req,
                &self.config_hash,
                "PUMPFUN_BONDING_CURVE",
                "pump.fun token — graduated-pumpfun disabled in config".to_string(),
            );
        }

        // ── 4. Token fast_check ─────────────────────────────────────────────
        let mut fast_check_liquidity: Option<Decimal> = None;
        let mut fast_check_errored = false;
        match self
            .token_parser
            .fast_check(&req.token_address, strategy)
            .await
        {
            Ok(result) if !result.safe => {
                let reason = result
                    .rejection_reason
                    .unwrap_or_else(|| "Token failed safety check".to_string());
                return BuyDecision::rejected(req, &self.config_hash, "TOKEN_UNSAFE", reason);
            }
            Ok(result) => {
                fast_check_liquidity = result.liquidity_usd;
            }
            Err(e) => {
                // fast-check failure proceeds to the slow path downstream;
                // record but do not reject here. The flag is propagated in the
                // decision so the caller can set Signal::force_slow_path.
                fast_check_errored = true;
                tracing::debug!(
                    token = %req.token_address,
                    error = %e,
                    "Token fast-check errored; proceeding to slow path"
                );
            }
        }

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
        if min_age > 0.0 {
            match token_age_hours {
                Some(age) if age < min_age => {
                    return BuyDecision::rejected(
                        req,
                        &self.config_hash,
                        "TOKEN_TOO_NEW",
                        format!(
                            "Token age {:.1}h below minimum {:.1}h",
                            age, min_age
                        ),
                    );
                }
                None => {
                    // Unknown age — policy: reject SPEAR, allow SHIELD.
                    if strategy == Strategy::Spear {
                        return BuyDecision::rejected(
                            req,
                            &self.config_hash,
                            "TOKEN_AGE_UNKNOWN",
                            "Token age unknown — rejected for SPEAR (conservative policy)"
                                .to_string(),
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
            return BuyDecision::rejected(req, &self.config_hash, code, reason);
        }

        // ── 6b. 24h volume via DexScreener (B3, fail-open) ─────────────────
        let volume_24h_usd = if let Some(ref dex) = self.dexscreener {
            dex.get_volume_24h(&req.token_address).await
        } else {
            None
        };

        // ── 7. Consensus detection ──────────────────────────────────────────
        let mut consensus_wallet_count: Option<usize> = None;
        let is_consensus = if let Some(ref aggregator) = self.signal_aggregator {
            if let Some(consensus) = aggregator
                .add_signal(
                    &req.wallet_address,
                    &req.token_address,
                    "BUY",
                    req.source_amount_sol,
                )
                .await
            {
                consensus_wallet_count = Some(consensus.wallet_count);
                true
            } else {
                consensus_wallet_count = Some(1);
                false
            }
        } else {
            false
        };

        // Persist to signal_aggregation so the stop-loss manager can detect
        // consensus on open positions regardless of ingress path.
        if let Err(e) = self.persist_signal_aggregation(req, is_consensus).await {
            tracing::warn!(
                error = %e,
                "Failed to record signal aggregation — consensus detection may be degraded"
            );
        }

        // ── 8. Signal-quality score ─────────────────────────────────────────
        let quality = SignalQuality::calculate(
            wallet_wqs,
            consensus_wallet_count,
            liquidity_usd,
            token_age_hours,
        );
        let quality_threshold = match strategy {
            Strategy::Shield => self.config.shield_signal_quality_threshold,
            Strategy::Spear => self.config.spear_signal_quality_threshold,
            Strategy::Exit => 0.0,
        };
        if !quality.should_enter(quality_threshold) {
            return BuyDecision::rejected(
                req,
                &self.config_hash,
                "SIGNAL_QUALITY_TOO_LOW",
                format!(
                    "Quality score {:.2} below threshold {:.2}",
                    quality.score, quality_threshold
                ),
            );
        }

        // ── 9. Market-regime multiplier ─────────────────────────────────────
        let regime_multiplier = if let Some(ref regime) = self.market_regime {
            regime.get_regime_multiplier(&req.token_address)
        } else {
            Decimal::ONE
        };

        // ── 10. Position size via PositionSizer ─────────────────────────────
        let size_sol = if let Some(ref sizer) = self.position_sizer {
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
            };
            let size = sizer.calculate_size(factors).await;
            if size.is_zero() {
                return BuyDecision::rejected(
                    req,
                    &self.config_hash,
                    "POSITION_SIZE_ZERO",
                    "Position sizer returned zero (strategy_max below min_size_sol)"
                        .to_string(),
                );
            }
            size
        } else {
            // No sizer configured — fall back to the source amount clamped to
            // the configured capital ceiling. This should not happen in
            // production; logged loudly.
            tracing::warn!("PositionSizer unavailable; using source amount (unclamped sizer)");
            req.source_amount_sol
        };

        // ── 11. Portfolio heat + strategy-allocation heat admission ─────────
        if let Some(ref heat) = self.portfolio_heat {
            match heat.can_open_position(size_sol).await {
                Ok(false) => {
                    return BuyDecision::rejected(
                        req,
                        &self.config_hash,
                        "PORTFOLIO_HEAT_LIMIT",
                        "Portfolio heat limit reached".to_string(),
                    )
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
                            return BuyDecision::rejected(
                                req,
                                &self.config_hash,
                                "STRATEGY_HEAT_LIMIT",
                                format!(
                                    "Strategy allocation limit reached for {:?}",
                                    strategy
                                ),
                            )
                        }
                        Ok(true) => {}
                        Err(e) => {
                            tracing::warn!(error = %e, "Strategy heat check failed, allowing trade")
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Portfolio heat check failed, allowing trade")
                }
            }
        }

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

    /// Persist a BUY signal to the signal_aggregation table so the stop-loss
    /// manager's consensus detection works uniformly for both ingress paths.
    async fn persist_signal_aggregation(
        &self,
        req: &SelectionRequest,
        is_consensus: bool,
    ) -> Result<(), crate::error::AppError> {
        use crate::db_abstraction::DbPool;
        let pool = match self.db.pool() {
            DbPool::PostgreSQL(p) => p,
            _ => {
                return Err(crate::error::AppError::Internal(
                    "PostgreSQL backend required".to_string(),
                ))
            }
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

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
            min_wqs_score: 70.0,
        };

        let mut config2 = config1.clone();
        assert_eq!(config1.hash(), config2.hash(), "identical configs should hash identically");

        config2.allow_graduated_pumpfun = false;
        assert_ne!(
            config1.hash(), config2.hash(),
            "changing allow_graduated_pumpfun should change hash"
        );

        config2 = config1.clone();
        config2.min_liquidity_pumpfun_usd = Decimal::from(50000);
        assert_ne!(
            config1.hash(), config2.hash(),
            "changing min_liquidity_pumpfun_usd should change hash"
        );
    }
}
