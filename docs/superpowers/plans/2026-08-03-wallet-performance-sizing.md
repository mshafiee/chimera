# Wallet Performance Sizing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Size admitted trades at 0.50 SOL for wallets with proven recent copy profitability (else the 0.25 floor), gated on sample/recency/WR/net to avoid small-sample overfitting.

**Architecture:** A `CopyTier { Base, Boosted }` is derived per wallet by `WalletPerformanceTracker` from its recent CLOSED copy trades. `SelectionEngine` (newly wired to the tracker) reads the tier and passes a `boost_target_sol` into `SizingFactors`; `PositionSizer::calculate_size` uses it (else the floor). Default OFF; all knobs in config.

**Tech Stack:** Rust (operator), sqlx/PostgreSQL, rust_decimal, chrono, existing `WalletPerformanceTracker` + `get_trades_filtered`.

## Global Constraints

- Paper trading only. No live-execution behavior changes.
- Financial values use `rust_decimal::Decimal` (never float). Type alias `AppResult<T> = Result<T, AppError>`.
- Feature default OFF (`wallet_boost_enabled: false`); zero behavior change unless enabled.
- Hard cap on the boost size is 0.50 SOL (`wallet_boost_size_sol`). The 0.25 floor (`position_sizing.min_size_sol`) is never breached.
- Sizing only moves UP (boost) — never below the floor. Bad wallets are removed by existing demotion, not sized-down.
- Lint gate: `cargo clippy -- -D warnings` must pass (operator). Tests: `cargo test --lib` and `cargo test --bin chimera_operator`.

## Spec reference

`docs/superpowers/specs/2026-08-03-wallet-performance-sizing-design.md`.

### Note on the cost-gate fallback (deferred)

The spec's cost-gate floor-fallback is **deferred from this plan**. Reasoning: the executor cost gate (`check_execution_costs`) validates a pre-computed quote; a fallback requires re-quoting at the floor (a second Jupiter call inside `execute_paper`), which is disproportionate for v1. The 5% cost gate accepts a 0.50 SOL size for tokens at/above the $8K liquidity floor (~0.85% round-trip cost at $8K), so the fallback is rarely triggered in practice. **If proven wallets are observed being skipped on borderline-liquidity tokens after this ships, add the fallback as a follow-up.** This is flagged explicitly rather than silently dropped.

---

## Task 1: Config — add `wallet_boost_*` fields to MonitoringConfig

**Files:**
- Modify: `operator/src/config.rs` (MonitoringConfig struct ~line 1423, its `Default` impl, and the default-fn block near `default_auto_promote_*`)
- Modify: `config/config.yaml` (monitoring section; added but OFF)

**Interfaces:**
- Produces: `MonitoringConfig` fields `wallet_boost_enabled: bool`, `wallet_boost_min_sample: u32`, `wallet_boost_window_trades: u32`, `wallet_boost_window_days: i64`, `wallet_boost_min_net_sol: Decimal`, `wallet_boost_min_winrate: f64`, `wallet_boost_recency_days: i64`, `wallet_boost_size_sol: Decimal`. Default OFF.

- [ ] **Step 1: Write the failing test**

Add to `operator/src/main.rs` test module (next to `test_auto_promote_config_defaults`):

```rust
#[test]
fn test_wallet_boost_config_defaults() {
    let m = chimera_operator::config::MonitoringConfig::default();
    assert!(!m.wallet_boost_enabled, "wallet_boost must default to false");
    assert_eq!(m.wallet_boost_min_sample, 15);
    assert_eq!(m.wallet_boost_window_trades, 20);
    assert_eq!(m.wallet_boost_window_days, 30);
    assert_eq!(m.wallet_boost_min_winrate, 0.40);
    assert_eq!(m.wallet_boost_recency_days, 7);
    assert_eq!(m.wallet_boost_size_sol, rust_decimal::Decimal::new(50, 2)); // 0.50
    assert_eq!(m.wallet_boost_min_net_sol, rust_decimal::Decimal::new(1, 2)); // 0.01
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin chimera_operator test_wallet_boost_config_defaults`
Expected: FAIL — `no field wallet_boost_enabled` on MonitoringConfig.

- [ ] **Step 3: Add the fields + defaults to config.rs**

In `MonitoringConfig` struct (after the `auto_promote_max_age_days` field):

```rust
    /// Enable tiered per-wallet copy-performance sizing: proven wallets (sample
    /// + recency + WR + net gates) get a larger allocation; everyone else stays
    /// at the floor. Default OFF (opt-in).
    #[serde(default = "default_false")]
    pub wallet_boost_enabled: bool,
    /// Minimum CLOSED copy trades in the window to be eligible for a boost.
    #[serde(default = "default_wallet_boost_min_sample")]
    pub wallet_boost_min_sample: u32,
    /// Window: consider the last N copy trades.
    #[serde(default = "default_wallet_boost_window_trades")]
    pub wallet_boost_window_trades: u32,
    /// Window: ignore copy trades older than this many days.
    #[serde(default = "default_wallet_boost_window_days")]
    pub wallet_boost_window_days: i64,
    /// Minimum net PnL (SOL) over the window to qualify (excludes trivially-small-positive).
    #[serde(default = "default_wallet_boost_min_net_sol")]
    pub wallet_boost_min_net_sol: rust_decimal::Decimal,
    /// Minimum win rate over the window (0.0–1.0).
    #[serde(default = "default_wallet_boost_min_winrate")]
    pub wallet_boost_min_winrate: f64,
    /// A wallet whose last copy trade is older than this many days loses its boost.
    #[serde(default = "default_wallet_boost_recency_days")]
    pub wallet_boost_recency_days: i64,
    /// The BOOSTED target size (SOL). Hard cap; the floor still applies below it.
    #[serde(default = "default_wallet_boost_size_sol")]
    pub wallet_boost_size_sol: rust_decimal::Decimal,
```

Add default fns next to `default_auto_promote_max_age_days`:

```rust
fn default_wallet_boost_min_sample() -> u32 { 15 }
fn default_wallet_boost_window_trades() -> u32 { 20 }
fn default_wallet_boost_window_days() -> i64 { 30 }
fn default_wallet_boost_min_net_sol() -> rust_decimal::Decimal { rust_decimal::Decimal::new(1, 2) } // 0.01
fn default_wallet_boost_min_winrate() -> f64 { 0.40 }
fn default_wallet_boost_recency_days() -> i64 { 7 }
fn default_wallet_boost_size_sol() -> rust_decimal::Decimal { rust_decimal::Decimal::new(50, 2) } // 0.50
```

Add to the `Default` impl (after `auto_promote_max_age_days: default_auto_promote_max_age_days(),`):

```rust
            wallet_boost_enabled: default_false(),
            wallet_boost_min_sample: default_wallet_boost_min_sample(),
            wallet_boost_window_trades: default_wallet_boost_window_trades(),
            wallet_boost_window_days: default_wallet_boost_window_days(),
            wallet_boost_min_net_sol: default_wallet_boost_min_net_sol(),
            wallet_boost_min_winrate: default_wallet_boost_min_winrate(),
            wallet_boost_recency_days: default_wallet_boost_recency_days(),
            wallet_boost_size_sol: default_wallet_boost_size_sol(),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --bin chimera_operator test_wallet_boost_config_defaults`
Expected: PASS.

- [ ] **Step 5: Add (OFF) keys to config/config.yaml**

In the `monitoring:` section (after the `auto_promote_*` block):

```yaml
  wallet_boost_enabled: false  # Tiered per-wallet copy-performance sizing (opt-in; see design spec)
  wallet_boost_min_sample: 15
  wallet_boost_window_trades: 20
  wallet_boost_window_days: 30
  wallet_boost_min_net_sol: 0.01
  wallet_boost_min_winrate: 0.40
  wallet_boost_recency_days: 7
  wallet_boost_size_sol: 0.50
```

- [ ] **Step 6: Lint + commit**

Run: `cargo clippy -- -D warnings` (operator)
```bash
git add operator/src/config.rs operator/src/main.rs config/config.yaml
git commit -m "feat(config): add wallet_boost_* sizing config (default off)"
```

---

## Task 2: CopyTier type + pure classify_copy_tier + compute_copy_tier

**Files:**
- Modify: `operator/src/monitoring/wallet_performance.rs`

**Interfaces:**
- Consumes: `MonitoringConfig.wallet_boost_*` (Task 1), `Database::get_trades_filtered(from, to, status, strategy, wallet, limit, offset)`.
- Produces: `pub enum CopyTier { Base, Boosted }`; `pub fn classify_copy_tier(...) -> CopyTier` (pure, testable); `impl WalletPerformanceTracker { pub async fn compute_copy_tier(&self, wallet: &str) -> CopyTier }`.

- [ ] **Step 1: Write failing tests for the pure classifier**

Add a `#[cfg(test)] mod tier_tests` (or extend the existing test module) in `wallet_performance.rs`:

```rust
#[cfg(test)]
mod tier_tests {
    use super::*;
    use rust_decimal::Decimal;

    fn cfg() -> crate::config::MonitoringConfig {
        crate::config::MonitoringConfig::default()
    }

    #[test]
    fn boosted_when_all_gates_pass() {
        let c = cfg();
        assert_eq!(
            classify_copy_tier(20, Decimal::new(5, 1), 0.50, 1, &c),
            CopyTier::Boosted
        ); // count 20>=15, net 0.5>0.01, wr 0.5>=0.4, age 1<=7
    }

    #[test]
    fn base_when_sample_too_small() {
        assert_eq!(classify_copy_tier(14, Decimal::new(5, 1), 0.50, 1, &cfg()), CopyTier::Base);
    }

    #[test]
    fn base_when_net_not_positive_enough() {
        assert_eq!(classify_copy_tier(20, Decimal::new(1, 2), 0.50, 1, &cfg()), CopyTier::Base); // net 0.01 not > 0.01
    }

    #[test]
    fn base_when_winrate_below_threshold() {
        assert_eq!(classify_copy_tier(20, Decimal::new(5, 1), 0.39, 1, &cfg()), CopyTier::Base);
    }

    #[test]
    fn base_when_dormant() {
        assert_eq!(classify_copy_tier(20, Decimal::new(5, 1), 0.50, 8, &cfg()), CopyTier::Base); // 8 > 7
    }

    #[test]
    fn base_when_disabled() {
        let mut c = cfg();
        c.wallet_boost_enabled = false;
        assert_eq!(classify_copy_tier(20, Decimal::new(5, 1), 0.50, 1, &c), CopyTier::Base);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib wallet_performance::tier_tests`
Expected: FAIL — `classify_copy_tier` / `CopyTier` not found.

- [ ] **Step 3: Add CopyTier + classify_copy_tier + compute_copy_tier**

Near the top of `wallet_performance.rs` (after the `use` block), add:

```rust
/// Per-wallet copy-performance tier driving position sizing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyTier {
    /// Default — size at the position floor.
    Base,
    /// Proven recent copy profitability — size at the boost target.
    Boosted,
}

/// Pure classification of a wallet's recent copy performance into a sizing tier.
/// All gates (sample, net PnL, win rate, recency) AND the enabled flag must pass
/// for Boosted; otherwise Base. Pure so it is unit-testable without a DB.
pub fn classify_copy_tier(
    sample_count: u32,
    net_sol: rust_decimal::Decimal,
    winrate: f64,
    days_since_last_trade: i64,
    cfg: &crate::config::MonitoringConfig,
) -> CopyTier {
    if !cfg.wallet_boost_enabled {
        return CopyTier::Base;
    }
    if sample_count < cfg.wallet_boost_min_sample {
        return CopyTier::Base;
    }
    if net_sol <= cfg.wallet_boost_min_net_sol {
        return CopyTier::Base;
    }
    if winrate < cfg.wallet_boost_min_winrate {
        return CopyTier::Base;
    }
    if days_since_last_trade > cfg.wallet_boost_recency_days {
        return CopyTier::Base;
    }
    CopyTier::Boosted
}
```

Add the DB-driven method inside `impl WalletPerformanceTracker` (it reuses the same `get_trades_filtered` query pattern already used in `get_metrics`):

```rust
    /// Derive this wallet's copy-performance sizing tier from its recent CLOSED
    /// copy trades. Queries the last `window_trades` within `window_days`,
    /// summarizes (count / net / wins / last-trade), and classifies.
    pub async fn compute_copy_tier(&self, wallet: &str) -> CopyTier {
        let cfg = match self.config.monitoring.as_ref() {
            Some(m) => m,
            None => return CopyTier::Base,
        };
        if !cfg.wallet_boost_enabled {
            return CopyTier::Base;
        }
        let window_days = cfg.wallet_boost_window_days;
        let window_trades = cfg.wallet_boost_window_trades;
        let from = chrono::Utc::now() - chrono::Duration::days(window_days);
        let from_str = from.format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let trades = match self
            .db
            .get_trades_filtered(
                Some(&from_str),
                None,
                Some("CLOSED"),
                None,
                Some(wallet),
                window_trades as i64,
                0,
            )
            .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, wallet = %wallet, "compute_copy_tier: query failed -> Base");
                return CopyTier::Base;
            }
        };

        // Most-recent first, then take the window.
        let mut sorted: Vec<_> = trades.into_iter().collect();
        sorted.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        let recent: Vec<_> = sorted.into_iter().take(window_trades as usize).collect();

        let count = recent.len() as u32;
        let net: rust_decimal::Decimal = recent
            .iter()
            .filter_map(|t| t.net_pnl_sol)
            .sum();
        let wins = recent
            .iter()
            .filter(|t| t.net_pnl_sol.map(|p| p > rust_decimal::Decimal::ZERO).unwrap_or(false))
            .count();
        let winrate = if count > 0 { wins as f64 / count as f64 } else { 0.0 };
        let last_trade = recent.iter().map(|t| t.created_at).max();
        let days_since = last_trade
            .map(|t| (chrono::Utc::now().signed_duration_since(t).num_days()).max(0))
            .unwrap_or(i64::MAX);

        classify_copy_tier(count, net, winrate, days_since, cfg)
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib wallet_performance::tier_tests`
Expected: PASS (6 tests).

- [ ] **Step 5: Lint + commit**

Run: `cargo clippy -- -D warnings`
```bash
git add operator/src/monitoring/wallet_performance.rs
git commit -m "feat(wallet_performance): CopyTier + classify/compute_copy_tier (sample/recency/WR/net gated)"
```

---

## Task 3: Wire WalletPerformanceTracker into SelectionEngine

**Files:**
- Modify: `operator/src/engine/selection.rs` (SelectionEngine struct ~line 220, add field + setter mirroring `latency_tracker`)
- Modify: `operator/src/main.rs` (call the setter where SelectionEngine is constructed)

**Interfaces:**
- Consumes: `WalletPerformanceTracker::compute_copy_tier` (Task 2).
- Produces: `SelectionEngine::with_wallet_performance(tracker)` and a private reader used by Task 5.

- [ ] **Step 1: Add the field + setter to SelectionEngine**

In `selection.rs`, add the field to the `SelectionEngine` struct (next to `latency_tracker`):

```rust
    latency_tracker: Option<Arc<crate::engine::LatencyTracker>>,
    /// Optional wallet-performance tracker for per-wallet copy-performance sizing.
    wallet_performance: Option<Arc<crate::monitoring::WalletPerformanceTracker>>,
```

Initialize it `None` in the `new` constructor (next to `latency_tracker: None,`):

```rust
            latency_tracker: None,
            wallet_performance: None,
```

Add a setter mirroring the existing `latency_tracker` setter (find the `pub fn ... (mut self, latency_tracker: Arc<...>)` pattern and add):

```rust
    /// Attach the wallet-performance tracker for tiered copy-performance sizing.
    pub fn with_wallet_performance(
        mut self,
        tracker: Arc<crate::monitoring::WalletPerformanceTracker>,
    ) -> Self {
        self.wallet_performance = Some(tracker);
        self
    }
```

- [ ] **Step 2: Wire it in main.rs**

Find where `SelectionEngine::new(...)` (or its builder) is constructed in `main.rs` and chain `.with_wallet_performance(wallet_performance_tracker.clone())` — `wallet_performance_tracker` is the `Arc` created at main.rs ~line 862. If the construction is a builder, add the setter call right after creation.

- [ ] **Step 3: Build + lint**

Run: `cargo clippy -- -D warnings`
Expected: clean (no test yet; the reader is added in Task 5).

- [ ] **Step 4: Commit**

```bash
git add operator/src/engine/selection.rs operator/src/main.rs
git commit -m "feat(selection): wire WalletPerformanceTracker into SelectionEngine"
```

---

## Task 4: `boost_target_sol` in SizingFactors + apply in calculate_size

**Files:**
- Modify: `operator/src/engine/position_sizer.rs` (SizingFactors struct ~line 27, `calculate_size`)

**Interfaces:**
- Consumes: none new.
- Produces: `SizingFactors.boost_target_sol: Option<Decimal>`; `calculate_size` returns the boost target (capped by strategy_max, floored by min) when set, else the floor.

- [ ] **Step 1: Write failing test for the sizer**

Add to `position_sizer.rs` test module (mirror an existing `calculate_size` test's setup; create a `PositionSizer` with `min_size_sol=0.25`):

```rust
#[test]
fn test_calculate_size_uses_boost_target_when_set() {
    let sizer = PositionSizer::new(/* db */, std::sync::Arc::new(test_position_sizing_config()));
    let mut factors = base_sizing_factors(); // helper used by other tests
    factors.boost_target_sol = Some(rust_decimal::Decimal::new(50, 2)); // 0.50
    let size = sizer.calculate_size(&factors);
    assert_eq!(size, rust_decimal::Decimal::new(50, 2)); // 0.50
}

#[test]
fn test_calculate_size_floor_when_no_boost() {
    let sizer = PositionSizer::new(/* db */, std::sync::Arc::new(test_position_sizing_config()));
    let factors = base_sizing_factors(); // boost_target_sol = None
    let size = sizer.calculate_size(&factors);
    assert_eq!(size, rust_decimal::Decimal::new(25, 2)); // 0.25 floor
}
```

(Use the existing test helpers `test_position_sizing_config()` / `base_sizing_factors()` already present in the file's test module; if a helper name differs, use whatever the existing `calculate_size` tests use.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib position_sizer::tests`
Expected: FAIL — `boost_target_sol` field missing.

- [ ] **Step 3: Add the field to SizingFactors**

In `SizingFactors` (position_sizer.rs:27), after `wqs_capped_max_size`:

```rust
    /// Optional per-wallet copy-performance boost target (SOL). When set, the
    /// final size starts from this value (still capped by strategy_max and
    /// floored by min_size_sol). Set by selection for wallets whose recent copy
    /// trades qualify them as BOOSTED.
    pub boost_target_sol: Option<rust_decimal::Decimal>,
```

- [ ] **Step 4: Apply the boost in calculate_size**

In `calculate_size`, near the top where the base size is established (before the off-hours/floor logic — find the line that sets the initial `size` from base_size_sol), replace the base-size derivation so a boost target seeds the size:

```rust
        // Per-wallet copy-performance boost: proven wallets start from their
        // boost target instead of base_size_sol. Still subject to strategy_max
        // and the min_size_sol floor below.
        let mut size = factors.boost_target_sol.unwrap_or(self.config.base_size_sol);
```

(If the existing code derives `size` differently, seed `size` from `boost_target_sol.unwrap_or(<existing base>)` at the equivalent point. The existing final clamp `size = size.max(self.config.min_size_sol)` and `strategy_max` clamp already bound it correctly: a 0.50 boost stays 0.50 when ≤ strategy_max and ≥ floor; the floor still protects the non-boosted path.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib position_sizer::tests`
Expected: PASS (including the two new tests).

- [ ] **Step 6: Lint + commit**

Run: `cargo clippy -- -D warnings`
```bash
git add operator/src/engine/position_sizer.rs
git commit -m "feat(position_sizer): apply boost_target_sol in calculate_size (floor still enforced)"
```

---

## Task 5: selection sets `boost_target_sol` from the wallet tier

**Files:**
- Modify: `operator/src/engine/selection.rs` (where `SizingFactors` is constructed ~line 978-999)

**Interfaces:**
- Consumes: `SelectionEngine.wallet_performance` (Task 3), `compute_copy_tier` (Task 2), `SizingFactors.boost_target_sol` (Task 4), `config.monitoring.wallet_boost_size_sol` (Task 1).
- Produces: BOOSTED wallets' signals carry `boost_target_sol = Some(0.50)`.

- [ ] **Step 1: Set the boost when building SizingFactors**

At the SizingFactors construction site (~line 978-999), after `wqs_capped_max_size` is computed, derive the tier and set `boost_target_sol`. The construction is synchronous, but `compute_copy_tier` is async — so resolve the tier just before building the factors (the surrounding code is already async):

```rust
        // Per-wallet copy-performance boost: if this wallet qualifies as
        // BOOSTED (proven recent copy profitability), seed the size from the
        // boost target; otherwise None (floor applies).
        let boost_target_sol = if let Some(ref tracker) = self.wallet_performance {
            match tracker.compute_copy_tier(&wallet_address).await {
                crate::monitoring::CopyTier::Boosted => {
                    let target = self
                        .config
                        .monitoring
                        .as_ref()
                        .map(|m| m.wallet_boost_size_sol)
                        .unwrap_or(rust_decimal::Decimal::new(50, 2));
                    tracing::info!(
                        wallet = %wallet_address,
                        boost_target_sol = %target,
                        "Wallet qualified for copy-performance size boost"
                    );
                    Some(target)
                }
                crate::monitoring::CopyTier::Base => None,
            }
        } else {
            None
        };
```

Then add `boost_target_sol,` to the `SizingFactors { ... }` literal. (`wallet_address` is the variable already in scope at the construction site; if it's named differently there, use the existing local.)

- [ ] **Step 2: Build + lint**

Run: `cargo clippy -- -D warnings`

- [ ] **Step 3: Commit**

```bash
git add operator/src/engine/selection.rs
git commit -m "feat(selection): set boost_target_sol for BOOSTED-tier wallets"
```

---

## Task 6: Enable + end-to-end verification

**Files:**
- Modify: `config/config.yaml` (flip `wallet_boost_enabled: true` when ready)
- Verify: operator log + DB

**Note:** Keep `wallet_boost_enabled: false` until you've confirmed tier classification on real data; flip to `true` only when ready to size-up.

- [ ] **Step 1: Build release**

Run: `make build-operator`

- [ ] **Step 2: Deploy (gitops)**

Commit/push, then on the server:
```bash
cd /opt/chimera && git pull origin main
COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml build operator
COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml up -d --force-recreate operator
```

- [ ] **Step 3: Verify tier classification with the feature still OFF**

Confirm the operator is healthy and that no sizing change occurs (all opens still 0.25):
```bash
docker compose ... exec -T operator grep -i "copy-performance size boost" /app/data/logs/operator.log.$(date -u +%F) | head
docker compose ... exec -T postgres psql -U chimera -d chimera -c \
  "SELECT opened_at, entry_amount_sol FROM positions WHERE opened_at > NOW() - INTERVAL '1 hour' ORDER BY opened_at DESC LIMIT 10;"
```
Expected: no boost logs yet (feature off); entries at 0.25.

- [ ] **Step 4: Flip ON and verify a BOOSTED wallet sizes 0.50**

Set `wallet_boost_enabled: true` in `config/config.yaml`, commit, push, pull, recreate operator. Then:
```bash
docker compose ... exec -T operator grep "qualified for copy-performance size boost" /app/data/logs/operator.log.$(date -u +%F) | head
docker compose ... exec -T postgres psql -U chimera -d chimera -c \
  "SELECT opened_at, strategy, entry_amount_sol FROM positions WHERE entry_amount_sol > 0.30 ORDER BY opened_at DESC LIMIT 10;"
```
Expected: boost log lines for qualifying wallets; new positions at 0.50 from BOOSTED wallets.

- [ ] **Step 5: Confirm auto-revoke**

After a BOOSTED wallet has a bad stretch or goes dormant (>7d), confirm it reverts to BASE (its new signals size 0.25, no boost log). Re-run the tier query from Task 2's logic via the boost log absence + entry_amount_sol = 0.25.

- [ ] **Step 6: Commit the enable flip**

```bash
git add config/config.yaml
git commit -m "feat(config): enable wallet_boost sizing"
```

---

## Self-review notes (completed)

- **Spec coverage:** config (T1), tier classification + proven gates (T2), data flow wiring (T3), sizer boost (T4), selection sets boost (T5), enable + verify (T6). The cost-gate fallback is explicitly deferred with rationale above.
- **Type consistency:** `CopyTier { Base, Boosted }`, `classify_copy_tier(...)`, `compute_copy_tier(&self, wallet) -> CopyTier`, `SizingFactors.boost_target_sol: Option<Decimal>`, `SelectionEngine::with_wallet_performance(...)` — names used consistently across tasks.
- **No placeholders:** every code step shows exact code; the two `/* db */` / helper references in Task 4 tests point to existing test helpers in the file (named explicitly).
