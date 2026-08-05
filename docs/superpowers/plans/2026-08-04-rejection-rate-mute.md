# Rejection-Rate Wallet Mute Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a rejection-rate-based wallet mute layer that suppresses signal intake from wallets whose BUY signals are overwhelmingly rejected for hard, structural reasons (non-speculative tokens, unsafe tokens, illiquid pump.fun tokens), eliminating ~95% of wasted decision processing with zero opportunity cost.

**Architecture:** A new `RejectionMuteDetector` (mirroring the `ToxicFlowDetector` pattern) maintains an in-memory rolling window of the last N BUY-decision outcomes per wallet. When the hard-rejection rate exceeds a configurable threshold, the wallet is time-boxed muted (default 6h). The mute gate short-circuits `decide_buy()` immediately after the existing toxic gate, before any expensive token-safety/liquidity checks. State is persisted to a new `muted_wallets` table and reloaded on startup.

**Tech Stack:** Rust (tokio, sqlx, chrono), PostgreSQL, existing `SelectionService` builder pattern.

## Global Constraints

- PostgreSQL only — use `%s`-style `$N` numbered placeholders (never `?`).
- All financial values use `rust_decimal::Decimal` — this feature has no financial values.
- Error handling: return `AppResult<T>` (alias for `Result<T, AppError>`), use `?` operator.
- Logging: structured `tracing` macros (`warn!`, `info!`, `debug!`).
- Tests: inline `#[cfg(test)] mod tests` in the detector module. Run with `cargo test --lib rejection_mute`.
- Clippy must pass: `cargo clippy -- -D warnings`.
- The config file (`config/config.yaml`) is the source of truth; env overrides are unreliable (config crate 0.15). New config uses `#[serde(default)]` so it works with defaults even when absent from the YAML.

## Hard vs Soft Rejection Classification

Only **hard** rejections count toward the mute threshold — these mean the wallet fundamentally trades assets we can never copy:

| Hard (counts toward mute) | Soft (does NOT count) |
|---|---|
| `NON_SPECULATIVE_TOKEN` | `LIQUIDITY_BELOW_MINIMUM` |
| `TOKEN_UNSAFE` | `SIGNAL_QUALITY_TOO_LOW` |
| `PUMPFUN_INSUFFICIENT_LIQUIDITY` | `WQS_TOO_LOW` |
| `PUMPFUN_BONDING_CURVE` | `TOKEN_TOO_NEW` |
| `INVALID_TOKEN_ADDRESS` | `TOKEN_AGE_UNKNOWN` |
| `TOKEN_FAST_CHECK_ERRORED` | `PORTFOLIO_HEAT_LIMIT`, `STRATEGY_HEAT_LIMIT` |
| | `POSITION_SIZE_ZERO`, `POSITION_SIZER_ERROR` |
| | `TOXIC_WALLET`, `WALLET_MUTED` (self), `WALLET_NOT_ACTIVE` |

Rationale: Hard rejections are structural (the token type is inherently untradeable). Soft rejections are situational (quality/liquidity/heat — can recover).

---

### Task 1: Database Migration — `muted_wallets` Table

**Files:**
- Create: `operator/migrations_postgres/0016_rejection_mute.sql`

- [ ] **Step 1: Create the migration file**

Create `operator/migrations_postgres/0016_rejection_mute.sql`:

```sql
-- Rejection-rate wallet mute: tracks wallets whose BUY signals are
-- overwhelmingly rejected for hard, structural reasons (non-speculative,
-- unsafe, illiquid pump.fun). Mirrors toxic_wallets persistence pattern.

CREATE TABLE IF NOT EXISTS muted_wallets (
    wallet_address      TEXT PRIMARY KEY,
    is_muted            BOOLEAN      NOT NULL DEFAULT FALSE,
    muted_at            TIMESTAMPTZ,
    muted_until         TIMESTAMPTZ,
    window_size         INTEGER      NOT NULL DEFAULT 0,
    run_id              TEXT,
    created_at          TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_muted_wallets_is_muted
    ON muted_wallets (is_muted) WHERE is_muted;
```

- [ ] **Step 2: Apply the migration to the local/dev database**

Run:
```bash
cd operator && cargo run -- apply-migrations
```
Expected: migration 0016 applies without error. (If the binary doesn't have this subcommand, apply manually via `psql` or check the Makefile target.)

- [ ] **Step 3: Commit**

```bash
git add operator/migrations_postgres/0016_rejection_mute.sql
git commit -m "feat(rejection_mute): add muted_wallets migration (0016)"
```

---

### Task 2: Config Struct — `RejectionMuteConfig`

**Files:**
- Modify: `operator/src/config.rs` (add struct ~line 134 after `profitability_gate`, add field to `AppConfig` ~line 134)

**Interfaces:**
- Produces: `pub struct RejectionMuteConfig` with fields `enabled`, `window_size`, `min_window_samples`, `hard_rejection_threshold`, `mute_duration_hours`; plus standalone `fn default_*()` functions and a `Default` impl. Consumed by Task 3 and Task 5.

- [ ] **Step 1: Add the `RejectionMuteConfig` struct and defaults**

In `operator/src/config.rs`, add the following **immediately after** the `ExperimentConfig` `impl Default` block (search for `fn default_local_top_decline_pct` and place after the `ExperimentConfig` Default impl, before the next struct):

```rust
/// ── Rejection-rate wallet mute ──────────────────────────────────────────

/// Mutes wallets whose BUY signals are overwhelmingly rejected for hard,
/// structural reasons (non-speculative / unsafe / illiquid pump.fun tokens).
/// Prevents wasted decision processing on wallets that can never produce an
/// actionable trade.
#[derive(Debug, Clone, Deserialize)]
pub struct RejectionMuteConfig {
    /// Master switch. When false, the mute gate and recording are no-ops.
    #[serde(default = "default_rejection_mute_enabled")]
    pub enabled: bool,
    /// Rolling window size: number of most-recent BUY decisions tracked.
    #[serde(default = "default_rejection_mute_window_size")]
    pub window_size: u32,
    /// Minimum samples in the window before a wallet can be muted
    /// (avoids muting on tiny sample sizes).
    #[serde(default = "default_rejection_mute_min_samples")]
    pub min_window_samples: u32,
    /// Hard-rejection rate threshold (0.0–1.0) that triggers a mute.
    #[serde(default = "default_rejection_mute_threshold")]
    pub hard_rejection_threshold: f64,
    /// How long (in hours) a wallet stays muted before re-evaluation.
    #[serde(default = "default_rejection_mute_duration_hours")]
    pub mute_duration_hours: u32,
}

fn default_rejection_mute_enabled() -> bool { true }
fn default_rejection_mute_window_size() -> u32 { 50 }
fn default_rejection_mute_min_samples() -> u32 { 20 }
fn default_rejection_mute_threshold() -> f64 { 0.90 }
fn default_rejection_mute_duration_hours() -> u32 { 6 }

impl Default for RejectionMuteConfig {
    fn default() -> Self {
        Self {
            enabled: default_rejection_mute_enabled(),
            window_size: default_rejection_mute_window_size(),
            min_window_samples: default_rejection_mute_min_samples(),
            hard_rejection_threshold: default_rejection_mute_threshold(),
            mute_duration_hours: default_rejection_mute_duration_hours(),
        }
    }
}
```

- [ ] **Step 2: Add the field to `AppConfig`**

In `operator/src/config.rs`, find the `AppConfig` struct (around line 74–135). Add this field **after** the `profitability_gate` field (line 134) and before the closing `}` of `AppConfig`:

```rust
    /// Rejection-rate wallet mute configuration
    #[serde(default)]
    pub rejection_mute: RejectionMuteConfig,
```

- [ ] **Step 3: Verify it compiles**

Run:
```bash
cd operator && cargo check 2>&1 | tail -5
```
Expected: compiles with no errors (warnings about unused fields are OK for now).

- [ ] **Step 4: Commit**

```bash
git add operator/src/config.rs
git commit -m "feat(rejection_mute): add RejectionMuteConfig to AppConfig"
```

---

### Task 3: RejectionMuteDetector — Core Module with Unit Tests

**Files:**
- Create: `operator/src/engine/rejection_mute.rs`
- Modify: `operator/src/engine/mod.rs` (add `pub mod rejection_mute;`)

**Interfaces:**
- Consumes: `crate::config::RejectionMuteConfig` (from Task 2), `crate::error::AppResult`
- Produces: `pub struct RejectionMuteDetector` with methods:
  - `pub fn new(config: RejectionMuteConfig) -> Self`
  - `pub fn is_hard_rejection(code: &str) -> bool` — classifies a rejection code
  - `pub async fn record_decision(&self, wallet: &str, admitted: bool, rejection_code: Option<&str>) -> AppResult<()>`
  - `pub async fn is_wallet_muted(&self, wallet: &str) -> bool`
  - `pub async fn get_muted_wallets(&self) -> Vec<String>`
  - `pub async fn persist_to_database(&self, pool: &sqlx::Pool<sqlx::Postgres>, run_id: &str) -> AppResult<()>`
  - `pub async fn load_from_database(&self, pool: &sqlx::Pool<sqlx::Postgres>) -> AppResult<()>`
- Consumed by: Task 4 (selection.rs gate + recording), Task 5 (main.rs wiring)

- [ ] **Step 1: Register the module**

In `operator/src/engine/mod.rs`, add (alongside the other `pub mod` declarations):

```rust
pub mod rejection_mute;
```

- [ ] **Step 2: Write the failing test for hard-rejection classification**

Create `operator/src/engine/rejection_mute.rs` with **only** the test module first:

```rust
//! Rejection-rate wallet mute.
//!
//! Tracks per-wallet BUY-decision rejection rates over a rolling window.
//! When a wallet's *hard* rejection rate exceeds a threshold, the wallet is
//! time-boxed muted — its signals are short-circuited before any expensive
//! token-safety or liquidity checks, eliminating wasted processing.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::config::RejectionMuteConfig;
use crate::error::AppResult;

/// Rejection codes that indicate the wallet fundamentally trades untradeable
/// assets. These count toward the mute threshold.
const HARD_REJECTION_CODES: &[&str] = &[
    "NON_SPECULATIVE_TOKEN",
    "TOKEN_UNSAFE",
    "PUMPFUN_INSUFFICIENT_LIQUIDITY",
    "PUMPFUN_BONDING_CURVE",
    "INVALID_TOKEN_ADDRESS",
    "TOKEN_FAST_CHECK_ERRORED",
];

#[derive(Debug, Clone)]
struct MutedWallet {
    address: String,
    window: VecDeque<bool>,
    is_muted: bool,
    muted_at: Option<DateTime<Utc>>,
    muted_until: Option<DateTime<Utc>>,
}

impl MutedWallet {
    fn new(address: String) -> Self {
        Self {
            address,
            window: VecDeque::new(),
            is_muted: false,
            muted_at: None,
            muted_until: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RejectionMuteDetector {
    wallets: Arc<RwLock<HashMap<String, MutedWallet>>>,
    config: RejectionMuteConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> RejectionMuteConfig {
        RejectionMuteConfig {
            enabled: true,
            window_size: 10,
            min_window_samples: 5,
            hard_rejection_threshold: 0.80,
            mute_duration_hours: 6,
        }
    }

    #[tokio::test]
    async fn test_hard_rejection_classification() {
        assert!(RejectionMuteDetector::is_hard_rejection("NON_SPECULATIVE_TOKEN"));
        assert!(RejectionMuteDetector::is_hard_rejection("TOKEN_UNSAFE"));
        assert!(RejectionMuteDetector::is_hard_rejection("PUMPFUN_INSUFFICIENT_LIQUIDITY"));
        assert!(!RejectionMuteDetector::is_hard_rejection("SIGNAL_QUALITY_TOO_LOW"));
        assert!(!RejectionMuteDetector::is_hard_rejection("WQS_TOO_LOW"));
        assert!(!RejectionMuteDetector::is_hard_rejection("WALLET_MUTED"));
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run:
```bash
cd operator && cargo test --lib rejection_mute::tests::test_hard_rejection_classification 2>&1 | tail -10
```
Expected: FAIL — `is_hard_rejection` method does not exist yet (compile error).

- [ ] **Step 4: Add `is_hard_rejection` + `new()` to make it compile**

Add to the `impl RejectionMuteDetector` block in `operator/src/engine/rejection_mute.rs` (insert after the struct definition):

```rust
impl RejectionMuteDetector {
    pub fn new(config: RejectionMuteConfig) -> Self {
        Self {
            wallets: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Classify a rejection code as "hard" (structural, untradeable asset).
    pub fn is_hard_rejection(code: &str) -> bool {
        HARD_REJECTION_CODES.contains(&code)
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run:
```bash
cd operator && cargo test --lib rejection_mute::tests::test_hard_rejection_classification 2>&1 | tail -5
```
Expected: PASS.

- [ ] **Step 6: Write failing tests for muting logic**

Append to the `tests` module in `operator/src/engine/rejection_mute.rs`:

```rust
    #[tokio::test]
    async fn test_mute_after_threshold_hard_rejections() {
        let det = RejectionMuteDetector::new(test_config());
        // 9 hard rejections, 1 admitted = 90% hard rate, ≥ min_samples(5), ≥ threshold(80%)
        for _ in 0..9 {
            det.record_decision("walletA", false, Some("TOKEN_UNSAFE")).await.unwrap();
        }
        det.record_decision("walletA", true, None).await.unwrap();
        assert!(det.is_wallet_muted("walletA").await, "wallet should be muted at 90% hard rate");
    }

    #[tokio::test]
    async fn test_no_mute_below_min_samples() {
        let det = RejectionMuteDetector::new(test_config()); // min_samples = 5
        // Only 4 hard rejections — below min_samples(5)
        for _ in 0..4 {
            det.record_decision("walletB", false, Some("NON_SPECULATIVE_TOKEN")).await.unwrap();
        }
        assert!(!det.is_wallet_muted("walletB").await, "should NOT mute below min_samples");
    }

    #[tokio::test]
    async fn test_no_mute_when_rate_below_threshold() {
        let det = RejectionMuteDetector::new(test_config()); // threshold = 80%
        // 3 hard, 7 soft = 30% — well below threshold
        for _ in 0..3 {
            det.record_decision("walletC", false, Some("TOKEN_UNSAFE")).await.unwrap();
        }
        for _ in 0..7 {
            det.record_decision("walletC", false, Some("SIGNAL_QUALITY_TOO_LOW")).await.unwrap();
        }
        assert!(!det.is_wallet_muted("walletC").await, "should NOT mute at 30% hard rate");
    }

    #[tokio::test]
    async fn test_admitted_signals_dilute_rate() {
        let det = RejectionMuteDetector::new(test_config()); // threshold = 80%, window = 10
        // 7 hard + 3 admitted = 70% — below threshold
        for _ in 0..7 {
            det.record_decision("walletD", false, Some("TOKEN_UNSAFE")).await.unwrap();
        }
        for _ in 0..3 {
            det.record_decision("walletD", true, None).await.unwrap();
        }
        assert!(!det.is_wallet_muted("walletD").await, "3 admits should dilute to 70% — not muted");
    }

    #[tokio::test]
    async fn test_disabled_never_mutes() {
        let mut cfg = test_config();
        cfg.enabled = false;
        let det = RejectionMuteDetector::new(cfg);
        for _ in 0..10 {
            det.record_decision("walletE", false, Some("TOKEN_UNSAFE")).await.unwrap();
        }
        assert!(!det.is_wallet_muted("walletE").await, "disabled detector should never mute");
    }

    #[tokio::test]
    async fn test_muted_wallet_freezes_window() {
        let det = RejectionMuteDetector::new(test_config()); // window=10, threshold=80%, min=5
        // Mute the wallet with 5 hard rejections (100%)
        for _ in 0..5 {
            det.record_decision("walletF", false, Some("TOKEN_UNSAFE")).await.unwrap();
        }
        assert!(det.is_wallet_muted("walletF").await);
        // Send 5 more signals while muted — should NOT change anything
        for _ in 0..5 {
            det.record_decision("walletF", true, None).await.unwrap();
        }
        assert!(det.is_wallet_muted("walletF").await, "still muted");
    }
```

- [ ] **Step 7: Run tests to verify they fail**

Run:
```bash
cd operator && cargo test --lib rejection_mute 2>&1 | tail -15
```
Expected: FAIL — `record_decision` and `is_wallet_muted` methods don't exist yet.

- [ ] **Step 8: Implement `record_decision` and `is_wallet_muted`**

Add these methods to the `impl RejectionMuteDetector` block:

```rust
    /// Record a BUY decision outcome for a wallet.
    ///
    /// Called from `SelectionService::decide()` after the decision is finalized.
    /// Maintains a rolling window of the last `window_size` decisions. If the
    /// hard-rejection rate exceeds the threshold, the wallet is muted.
    pub async fn record_decision(
        &self,
        wallet: &str,
        admitted: bool,
        rejection_code: Option<&str>,
    ) -> AppResult<()> {
        if !self.config.enabled {
            return Ok(());
        }

        let mut wallets = self.wallets.write().await;
        let entry = wallets
            .entry(wallet.to_string())
            .or_insert_with(|| MutedWallet::new(wallet.to_string()));

        // If currently muted, check for expiry.
        if entry.is_muted {
            if let Some(until) = entry.muted_until {
                if Utc::now() >= until {
                    // Mute expired — reset for a fresh evaluation window.
                    entry.is_muted = false;
                    entry.muted_at = None;
                    entry.muted_until = None;
                    entry.window.clear();
                    // Fall through to record this signal in the fresh window.
                } else {
                    // Still muted — do not pollute the frozen window.
                    return Ok(());
                }
            }
        }

        let is_hard =
            !admitted && rejection_code.map(Self::is_hard_rejection).unwrap_or(false);

        // Push to rolling window, evicting oldest if over capacity.
        entry.window.push_back(is_hard);
        if entry.window.len() > self.config.window_size as usize {
            entry.window.pop_front();
        }

        // Evaluate mute condition.
        if !entry.is_muted && entry.window.len() >= self.config.min_window_samples as usize {
            let hard = entry.window.iter().filter(|&&h| h).count();
            let rate = hard as f64 / entry.window.len() as f64;
            if rate >= self.config.hard_rejection_threshold {
                let now = Utc::now();
                let until = now + ChronoDuration::hours(self.config.mute_duration_hours as i64);
                entry.is_muted = true;
                entry.muted_at = Some(now);
                entry.muted_until = Some(until);
                warn!(
                    wallet = %wallet,
                    hard_rate = format!("{:.0}%", rate * 100.0),
                    window = entry.window.len(),
                    muted_until = %until,
                    "RejectionMuteDetector: wallet muted (high hard-rejection rate)"
                );
            }
        }

        Ok(())
    }

    /// Gate check: is this wallet currently muted?
    ///
    /// Read-only; called on the hot path in `decide_buy()`. Expiry is handled
    /// lazily — a muted wallet whose `muted_until` has passed returns `false`
    /// here and its window resets on the next `record_decision` call.
    pub async fn is_wallet_muted(&self, wallet: &str) -> bool {
        if !self.config.enabled {
            return false;
        }
        let wallets = self.wallets.read().await;
        if let Some(w) = wallets.get(wallet) {
            if w.is_muted {
                if let Some(until) = w.muted_until {
                    return Utc::now() < until;
                }
                return true;
            }
        }
        false
    }
```

- [ ] **Step 9: Run tests to verify they pass**

Run:
```bash
cd operator && cargo test --lib rejection_mute 2>&1 | tail -15
```
Expected: all 7 tests PASS.

- [ ] **Step 10: Add `get_muted_wallets` query helper**

Add to the `impl RejectionMuteDetector` block:

```rust
    /// List all currently-muted wallet addresses (for diagnostics / API).
    pub async fn get_muted_wallets(&self) -> Vec<String> {
        let wallets = self.wallets.read().await;
        wallets
            .values()
            .filter(|w| {
                w.is_muted
                    && w.muted_until
                        .map(|until| Utc::now() < until)
                        .unwrap_or(true)
            })
            .map(|w| w.address.clone())
            .collect()
    }
```

- [ ] **Step 11: Write the failing test for expiry (time-based)**

Append to the `tests` module:

```rust
    #[tokio::test]
    async fn test_mute_expires_after_duration() {
        let mut cfg = test_config();
        cfg.mute_duration_hours = 0; // expires immediately (0 hours)
        let det = RejectionMuteDetector::new(cfg);
        for _ in 0..5 {
            det.record_decision("walletG", false, Some("TOKEN_UNSAFE")).await.unwrap();
        }
        // The mute was set; muted_until is ~now, so next check may be expired.
        // record_decision should reset on the next call after expiry.
        // Give it a signal to trigger the expiry reset path:
        det.record_decision("walletG", true, None).await.unwrap();
        assert!(!det.is_wallet_muted("walletG").await, "wallet should be unmuted after 0h expiry + new signal");
    }
```

- [ ] **Step 12: Run all tests**

Run:
```bash
cd operator && cargo test --lib rejection_mute 2>&1 | tail -15
```
Expected: all 8 tests PASS.

- [ ] **Step 13: Add DB persistence methods (`persist_to_database`, `load_from_database`)**

Add a private row-mapping struct and the two persistence methods. Place the struct at module level (after `MutedWallet`) and the methods in the `impl` block:

Struct (module level):
```rust
#[derive(Debug, sqlx::FromRow)]
struct MutedWalletRow {
    wallet_address: String,
    is_muted: bool,
    muted_at: Option<DateTime<Utc>>,
    muted_until: Option<DateTime<Utc>>,
}
```

Methods (in `impl RejectionMuteDetector`):
```rust
    /// Persist all tracked wallet state to the database (UPSERT).
    /// Snapshots under a short read lock, then does I/O lock-free.
    pub async fn persist_to_database(
        &self,
        pool: &sqlx::Pool<sqlx::Postgres>,
        run_id: &str,
    ) -> AppResult<()> {
        let snapshot: Vec<(String, bool, Option<DateTime<Utc>>, Option<DateTime<Utc>>, usize)> = {
            let wallets = self.wallets.read().await;
            wallets
                .values()
                .map(|w| {
                    (
                        w.address.clone(),
                        w.is_muted,
                        w.muted_at,
                        w.muted_until,
                        w.window.len(),
                    )
                })
                .collect()
        };

        for (address, is_muted, muted_at, muted_until, window_size) in &snapshot {
            sqlx::query(
                r#"
                INSERT INTO muted_wallets (
                    wallet_address, is_muted, muted_at, muted_until, window_size, run_id
                ) VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (wallet_address) DO UPDATE SET
                    is_muted     = excluded.is_muted,
                    muted_at     = excluded.muted_at,
                    muted_until  = excluded.muted_until,
                    window_size  = excluded.window_size,
                    run_id       = excluded.run_id,
                    updated_at   = CURRENT_TIMESTAMP
                "#,
            )
            .bind(address)
            .bind(is_muted)
            .bind(muted_at)
            .bind(muted_until)
            .bind(*window_size as i64)
            .bind(run_id)
            .execute(pool)
            .await?;
        }

        info!("Persisted {} muted-wallet records to database", snapshot.len());
        Ok(())
    }

    /// Load still-active mutes from the database on startup.
    /// Only restores wallets where `is_muted = TRUE AND muted_until > NOW()`.
    /// The rolling window starts empty (fresh evaluation after the mute lapses).
    pub async fn load_from_database(
        &self,
        pool: &sqlx::Pool<sqlx::Postgres>,
    ) -> AppResult<()> {
        let rows = sqlx::query_as::<_, MutedWalletRow>(
            r#"
            SELECT wallet_address, is_muted, muted_at, muted_until
            FROM muted_wallets
            WHERE is_muted = TRUE AND muted_until > NOW()
            "#,
        )
        .fetch_all(pool)
        .await?;

        let mut wallets = self.wallets.write().await;
        let count = rows.len();
        for row in rows {
            wallets.insert(
                row.wallet_address.clone(),
                MutedWallet {
                    address: row.wallet_address,
                    window: VecDeque::new(),
                    is_muted: true,
                    muted_at: row.muted_at,
                    muted_until: row.muted_until,
                },
            );
        }
        if count > 0 {
            warn!("Loaded {} active muted wallets from database on startup", count);
        }
        Ok(())
    }
```

- [ ] **Step 14: Verify full module compiles and clippy passes**

Run:
```bash
cd operator && cargo clippy --lib -- -D warnings 2>&1 | tail -10
```
Expected: no errors, no warnings.

- [ ] **Step 15: Commit**

```bash
git add operator/src/engine/rejection_mute.rs operator/src/engine/mod.rs
git commit -m "feat(rejection_mute): add RejectionMuteDetector with rolling-window muting"
```

---

### Task 4: SelectionService Integration — Gate + Recording

**Files:**
- Modify: `operator/src/engine/selection.rs`

**Interfaces:**
- Consumes: `crate::engine::rejection_mute::RejectionMuteDetector` (from Task 3)
- Produces: `SelectionService` gains a `mute_detector: Option<Arc<RejectionMuteDetector>>` field, a `.with_mute_detector()` builder, a `WALLET_MUTED` gate in `decide_buy()`, and a recording call in `decide()`.

- [ ] **Step 1: Add the field to `SelectionService`**

In `operator/src/engine/selection.rs`, find the `SelectionService` struct (line 208). Add this field after the `shadow_trader` field (line 224):

```rust
    /// Rejection-rate wallet mute detector.
    mute_detector: Option<Arc<crate::engine::rejection_mute::RejectionMuteDetector>>,
```

- [ ] **Step 2: Initialize the field in `new()`**

In the `SelectionService::new()` constructor (line 242–259), add to the struct literal (after `shadow_trader: None,`):

```rust
            mute_detector: None,
```

- [ ] **Step 3: Add the builder method**

Add this builder method after the `with_shadow_trader` builder (search for `with_shadow_trader` and place after its closing brace):

```rust
    /// Attach the RejectionMuteDetector for rejection-rate-based wallet muting.
    pub fn with_mute_detector(
        mut self,
        detector: Arc<crate::engine::rejection_mute::RejectionMuteDetector>,
    ) -> Self {
        self.mute_detector = Some(detector);
        self
    }
```

- [ ] **Step 4: Add the mute gate in `decide_buy()`**

In `operator/src/engine/selection.rs`, find the toxic-wallet gate (line 646–667). Add the mute gate **immediately after** the closing brace of the toxic gate block (after line 667), before `let wallet_wqs`:

```rust
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
                return BuyDecision::rejected(
                    req,
                    &self.config_hash,
                    "WALLET_MUTED",
                    reason,
                );
            }
        }
```

- [ ] **Step 5: Add the recording call in `decide()`**

In `operator/src/engine/selection.rs`, find the `decide()` method (line 350–406). After the shadow-trader block (after line 377, `shadow.on_signal(...)`), add:

```rust
        // Rejection-rate mute detector: record BUY decision outcomes for
        // rolling-window rejection-rate tracking. Only BUY decisions are
        // meaningful (SELL rejections have different semantics).
        if let Some(ref mute) = self.mute_detector {
            if matches!(req.action, Action::Buy) {
                mute.record_decision(
                    &req.wallet_address,
                    decision.admitted,
                    decision.rejection_code,
                )
                .await;
            }
        }
```

- [ ] **Step 6: Verify it compiles and clippy passes**

Run:
```bash
cd operator && cargo clippy --lib -- -D warnings 2>&1 | tail -10
```
Expected: no errors, no warnings.

- [ ] **Step 7: Run existing selection tests to verify no regressions**

Run:
```bash
cd operator && cargo test --lib selection 2>&1 | tail -10
```
Expected: all existing tests still PASS.

- [ ] **Step 8: Commit**

```bash
git add operator/src/engine/selection.rs
git commit -m "feat(rejection_mute): integrate WALLET_MUTED gate and decision recording in SelectionService"
```

---

### Task 5: Wiring in `main.rs` — Construct, Startup Load, Periodic Persist, Shutdown Persist

**Files:**
- Modify: `operator/src/main.rs`

**Interfaces:**
- Consumes: `crate::engine::rejection_mute::RejectionMuteDetector` (Task 3), `config.rejection_mute` (Task 2), `SelectionService::with_mute_detector()` (Task 4)
- Produces: a fully wired, persisted detector in the running operator.

- [ ] **Step 1: Construct the detector**

In `operator/src/main.rs`, find the toxic-flow-detector construction (around line 876–882, search for `ToxicFlowDetector::new`). Add **immediately after** the toxic detector construction:

```rust
    let rejection_mute_detector = Arc::new(
        crate::engine::rejection_mute::RejectionMuteDetector::new(config.rejection_mute.clone()),
    );
```

- [ ] **Step 2: Attach to SelectionService via builder**

In `operator/src/main.rs`, find the `selection_service` builder chain (around line 2918–2934, search for `.with_toxic_detector(`). Add `.with_mute_detector()` to the chain, right after `.with_toxic_detector(toxic_flow_detector.clone())`:

```rust
        .with_mute_detector(rejection_mute_detector.clone())
```

- [ ] **Step 3: Add startup load (after toxic startup load)**

In `operator/src/main.rs`, find the toxic-wallet startup load (search for `load_from_database`, around line 3177–3185). Add **immediately after** the toxic load block:

```rust
    // Rejection-rate mute: load active mutes from database on startup
    {
        use chimera_operator::db_abstraction::DbPool;
        if let DbPool::PostgreSQL(pg_pool) = db_pool.pool() {
            if let Err(e) = rejection_mute_detector.load_from_database(&pg_pool).await {
                tracing::warn!(error = %e, "Failed to load muted wallets on startup");
            }
        }
    }
```

- [ ] **Step 4: Add periodic persist (after toxic periodic persist)**

In `operator/src/main.rs`, find the toxic periodic-persist task (search for `Periodic toxic-wallet persistence`, around line 3187–3210). Add **immediately after** that block's closing brace:

```rust
    // Rejection-rate mute: periodic persistence (every 5 minutes)
    {
        let persist_mute = rejection_mute_detector.clone();
        let persist_db = db_pool.clone();
        let persist_token = cancel_token.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            interval.tick().await; // consume first immediate tick
            loop {
                tokio::select! {
                    _ = persist_token.cancelled() => break,
                    _ = interval.tick() => {
                        use chimera_operator::db_abstraction::DbPool;
                        if let DbPool::PostgreSQL(pg_pool) = persist_db.pool() {
                            let run_id = format!("v{}", env!("CARGO_PKG_VERSION"));
                            if let Err(e) = persist_mute.persist_to_database(&pg_pool, &run_id).await {
                                tracing::warn!(error = %e, "Periodic muted-wallet persist failed");
                            }
                        }
                    }
                }
            }
        });
    }
```

- [ ] **Step 5: Add shutdown persist (after toxic shutdown persist)**

In `operator/src/main.rs`, find the toxic final-persist on shutdown (search for `Final toxic-wallet persistence`, around line 3251–3262). Add **immediately after** that block:

```rust
    // Rejection-rate mute: final persistence on shutdown
    {
        use chimera_operator::db_abstraction::DbPool;
        if let DbPool::PostgreSQL(pg_pool) = db_pool.pool() {
            let run_id = format!("v{}", env!("CARGO_PKG_VERSION"));
            if let Err(e) = rejection_mute_detector.persist_to_database(&pg_pool, &run_id).await {
                tracing::warn!(error = %e, "Final muted-wallet persist failed on shutdown");
            }
        }
    }
```

- [ ] **Step 6: Verify full build compiles**

Run:
```bash
cd operator && cargo build 2>&1 | tail -5
```
Expected: builds successfully.

- [ ] **Step 7: Verify clippy on the full crate**

Run:
```bash
cd operator && cargo clippy -- -D warnings 2>&1 | tail -10
```
Expected: no errors, no warnings.

- [ ] **Step 8: Run full lib test suite**

Run:
```bash
cd operator && cargo test --lib 2>&1 | tail -10
```
Expected: all tests PASS (409+ existing + 8 new rejection_mute tests).

- [ ] **Step 9: Commit**

```bash
git add operator/src/main.rs
git commit -m "feat(rejection_mute): wire detector into main.rs (startup load, periodic/shutdown persist)"
```

---

### Task 6: Add Config to `config.yaml`

**Files:**
- Modify: `config/config.yaml`

**Interfaces:**
- None (configuration only).

- [ ] **Step 1: Add the rejection_mute section**

In `config/config.yaml`, add at the top level (e.g., after the `experiment:` section or near the end before any closing):

```yaml
# Rejection-rate wallet mute: suppresses signal intake from wallets whose
# BUY signals are overwhelmingly rejected for structural reasons.
rejection_mute:
  enabled: true
  window_size: 50           # track last 50 BUY decisions per wallet
  min_window_samples: 20    # need ≥20 decisions before muting
  hard_rejection_threshold: 0.90  # mute at ≥90% hard-rejection rate
  mute_duration_hours: 6    # mute for 6h, then re-evaluate
```

- [ ] **Step 2: Commit**

```bash
git add config/config.yaml
git commit -m "feat(rejection_mute): add config.yaml section with production defaults"
```

---

### Task 7: Production Deployment

**Files:**
- None (deployment only).

- [ ] **Step 1: Push to main**

Run:
```bash
git push origin main
```

- [ ] **Step 2: On the production server, pull and rebuild**

Run on `root@chimera-01.moez.tech`:
```bash
cd /opt/chimera
git pull origin main
COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml build operator
COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml up -d --force-recreate operator
```

- [ ] **Step 3: Verify migration applied and detector active**

Run on the production server:
```bash
docker exec chimera-postgres psql -U chimera -d chimera -c "\d muted_wallets"
docker logs chimera-operator 2>&1 | grep -i "muted\|RejectionMute" | tail -10
```
Expected: `muted_wallets` table exists; logs show detector loaded (or "0 active muted wallets" if none yet).

- [ ] **Step 4: Verify noise wallets get muted over time**

After ~1 hour of operation, run:
```bash
docker exec chimera-postgres psql -U chimera -d chimera -c \
  "SELECT wallet_address, is_muted, muted_until, window_size FROM muted_wallets WHERE is_muted ORDER BY muted_until;"
docker exec chimera-postgres psql -U chimera -d chimera -c \
  "SELECT rejection_code, count(*) FROM decision_records WHERE decided_at > NOW() - INTERVAL '1 hour' GROUP BY rejection_code ORDER BY count DESC LIMIT 10;"
```
Expected: `7wXtGay` (USDC market-maker) should appear muted after accumulating 50+ decisions. `WALLET_MUTED` rejection codes should start appearing in `decision_records`, and `NON_SPECULATIVE_TOKEN` counts should drop as the wallet is short-circuited.
