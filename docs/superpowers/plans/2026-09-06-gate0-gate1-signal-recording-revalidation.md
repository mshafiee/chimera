# Gate 0/1: Durable Signal Recording & Cluster Hypothesis Re-Validation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Repair the signal pipeline so every tracked-wallet swap is durably recorded pre-admission (fixing the consensus-recording deadlock), then run a pre-registered, reproducible re-validation of the cluster hypothesis against production data — producing the GO/PIVOT verdict that decides whether the held cluster-accumulation engine proceeds.

**Architecture:** Three layers. (1) A new `smart_money_signals` Postgres table (migration `0023`, auto-applied by `sqlx::migrate!`) that records every parsed tracked-wallet swap idempotently, before any admission gating. (2) A `Database` trait extension (`record_smart_money_signal`, `get_token_wallet_count`) implemented in `PostgresBackend` and the two test mocks, wired into the Helius webhook path with fire-and-forget spawns so webhook latency is untouched. (3) A read-only Scout analysis script that reconstructs cluster attribution from the existing 26K-position shadow book via a 12h token-window self-join, applies the pre-registered temporal-dispersion rule, and emits a bootstrap-CI verdict table. The engine fixes from the held plan stay parked until the verdict is GO.

**Tech Stack:** PostgreSQL 16 (sqlx, `%s`-free `$n` placeholders), Rust 2021 (`rust_decimal::Decimal`, `tokio`, `tracing`), Python 3.11 + psycopg3 + pytest (Scout).

## Global Constraints

- Money is `rust_decimal::Decimal` (Rust) / `Decimal` (Python) — never `f64`/float for financial quantities. Python `pnl_pct` comparisons may use floats only because `pnl_pct` is already a stored `NUMERIC` read as a number, and it is a *percentage metric*, not a monetary amount used for sizing.
- PostgreSQL only (SQLite decommissioned 2026-07). sqlx `$n` placeholders; Python psycopg `%s` placeholders.
- All DB access routes through `infra/src/db_abstraction/`; no hand-rolled SQL in callers (Scout analysis scripts are the sanctioned exception — they are read-only tooling).
- Recording a signal must **never** fail the webhook path: it runs in a detached `tokio::spawn` and only logs failures.
- `smart_money_signals` inserts are idempotent: unique `(tx_signature, token_address, side)` + `ON CONFLICT DO NOTHING`.
- Webhook handler must return quickly — no new `await` of DB writes on the hot path beyond what already exists.
- `operator/AGENTS.md`: `executor.rs` / `signal_pipeline.rs` / `circuit_breaker.rs` are off-limits in this plan. `selection.rs` edit is limited to the step-7 consensus block.
- Every task ends with a commit. Clippy runs with `-D warnings`; Python tests via `pytest scout/tests/`.
- The §1.2 table in the held plan doc is **not** the target of this work — it stays marked unreproducible. This plan produces its replacement.

---

### Task 1: Migration 0023 — `smart_money_signals` recording table

**Files:**
- Create: `infra/migrations_postgres/0023_smart_money_signals.sql`

**Interfaces:**
- Consumes: nothing (pure DDL).
- Produces: table `smart_money_signals` — later tasks' `record_smart_money_signal` INSERT and `get_token_wallet_count` SELECT depend on these exact columns: `wallet_address TEXT`, `token_address TEXT`, `token_symbol TEXT`, `side TEXT`, `amount_sol NUMERIC(30,18)`, `amount_tokens NUMERIC(38,18)`, `token_decimals INT`, `price_usd NUMERIC(30,18) NULL`, `price_sol NUMERIC(30,18) NULL`, `tx_signature TEXT`, `slot BIGINT`, `block_time TIMESTAMPTZ`.

- [ ] **Step 1: Write the migration file**

```sql
-- Durable pre-admission signal recording (Gate 0, 2026-09-06).
--
-- Root cause being fixed: the in-memory SignalAggregator only counted
-- admitted-signal consensus in a 5-minute window, and with a 5-wallet ACTIVE
-- roster, decision_records.consensus_wallet_count was NULL for 178,307 of
-- 180,738 decisions. Multi-wallet cluster behaviour was therefore
-- unobservable, and the held cluster plan's §1.2 statistics could never have
-- been produced by this pipeline.
--
-- This table records EVERY parsed swap of a tracked wallet (ACTIVE/PROVING)
-- the moment it is parsed, BEFORE circuit breaker / selection / admission.
-- Bilateral (BUY + SELL) so disinvestment and cluster exit logic can later
-- consume it. price_usd/price_sol are nullable: recording must never fail
-- because ancillary pricing data was unavailable.
--
-- token_decimals records the mint decimals at parse time so consumers can
-- normalize raw-token amounts to UI units (audit finding: parser mixes
-- uiAmount and raw token_amount paths — normalized via decimals, never
-- re-derived later).

CREATE TABLE IF NOT EXISTS smart_money_signals (
    id BIGSERIAL PRIMARY KEY,
    wallet_address TEXT NOT NULL,
    token_address TEXT NOT NULL,
    token_symbol TEXT,
    side TEXT NOT NULL CHECK (side IN ('BUY', 'SELL')),
    amount_sol NUMERIC(30, 18) NOT NULL,
    amount_tokens NUMERIC(38, 18) NOT NULL,
    token_decimals INT,
    price_usd NUMERIC(30, 18),
    price_sol NUMERIC(30, 18),
    tx_signature TEXT NOT NULL,
    slot BIGINT NOT NULL,
    block_time TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_smart_signal_tx_token_side UNIQUE (tx_signature, token_address, side)
);

-- Window scan pattern: token + side + time (cluster trigger + revalidation).
CREATE INDEX IF NOT EXISTS idx_smart_signals_token_window
    ON smart_money_signals (token_address, side, block_time DESC)
    INCLUDE (wallet_address, amount_sol, amount_tokens, price_sol);

-- Per-wallet history (future roster analytics).
CREATE INDEX IF NOT EXISTS idx_smart_signals_wallet_time
    ON smart_money_signals (wallet_address, block_time DESC);
```

- [ ] **Step 2: Verify migration applies locally**

Run: `make dev` (or any local boot of the operator against the dev Postgres) and watch startup logs.

Expected: `sqlx` logs `0023_smart_money_signals` applied (or `migrate!` succeeds without error). If no dev DB is available, apply on prod directly (Step 3).

- [ ] **Step 3: Verify on production**

Run: `ssh root@chimera-01.moez.tech "docker exec chimera-postgres psql -U chimera -d chimera -c '\\d smart_money_signals' | head -20"`

Expected: table description shows the columns above. (Auto-applied on next operator deploy; to apply immediately without deploy: pipe the file into psql.)

- [ ] **Step 4: Commit**

```bash
git add infra/migrations_postgres/0023_smart_money_signals.sql
git commit -m "feat(db): smart_money_signals pre-admission recording table (Gate 0)"
```

---

### Task 2: `Database` trait — `record_smart_money_signal` + `get_token_wallet_count`

**Files:**
- Modify: `infra/src/db_abstraction/types.rs` (add `SmartMoneySignal` struct)
- Modify: `infra/src/db_abstraction/mod.rs` (trait methods, near the monitoring-related methods)
- Modify: `infra/src/db_abstraction/postgres.rs` (implementations, inside `impl Database for PostgresBackend`)
- Modify: `operator/src/monitoring/test_db.rs` (`impl Database for MockDb` — stub returning `Ok`)
- Modify: `infra/src/engine/kelly_sizer.rs` (`impl Database for MockDatabase` — stub returning `Ok`)

**Interfaces:**
- Consumes: Task 1 table.
- Produces (exact signatures later tasks rely on):
  ```rust
  pub struct SmartMoneySignal {
      pub wallet_address: String,
      pub token_address: String,
      pub token_symbol: Option<String>,
      pub side: String,             // "BUY" | "SELL"
      pub amount_sol: Decimal,
      pub amount_tokens: Decimal,
      pub token_decimals: Option<i32>,
      pub price_usd: Option<Decimal>,
      pub price_sol: Option<Decimal>,
      pub tx_signature: String,
      pub slot: i64,
      pub block_time: DateTime<Utc>,
  }

  async fn record_smart_money_signal(&self, signal: &SmartMoneySignal) -> AppResult<()>;
  async fn get_token_wallet_count(&self, token_address: &str, window_hours: i64) -> AppResult<i64>;
  // get_token_wallet_count returns COUNT(DISTINCT wallet_address) of BUY rows
  // in the trailing window. Returns 0 on empty.
  ```

- [ ] **Step 1: Write the failing test (mock-based trait test)**

Add to `operator/src/monitoring/test_db.rs` inside the existing `mod tests` (follow the file's existing test style):

```rust
#[tokio::test]
async fn test_mock_records_smart_money_signal() {
    use crate::engine::SmartMoneySignal;
    let db = MockDb::new();
    let signal = SmartMoneySignal {
        wallet_address: "w".to_string(),
        token_address: "tok".to_string(),
        token_symbol: Some("TOK".to_string()),
        side: "BUY".to_string(),
        amount_sol: rust_decimal::Decimal::from(1),
        amount_tokens: rust_decimal::Decimal::from(1000),
        token_decimals: Some(6),
        price_usd: None,
        price_sol: None,
        tx_signature: "sig1".to_string(),
        slot: 123,
        block_time: chrono::Utc::now(),
    };
    // Must compile against the trait and succeed on the mock — the trait
    // method exists contract for the real PostgresBackend.
    db.record_smart_money_signal(&signal)
        .await
        .expect("mock record must succeed");
    assert_eq!(
        db.get_token_wallet_count("tok", 12).await.expect("count"),
        0,
        "mock returns 0 (no real data)"
    );
}
```

Note: the `SmartMoneySignal` struct must be re-exported from `crate::engine` or `chimera_infra` depending on where the crate re-exports `db_abstraction::types` — check `operator/src/lib.rs` imports used by other `Database` types (e.g. how `Trade` is imported in `test_db.rs`) and mirror that exact path in the test.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p chimera_operator --lib test_mock_records_smart_money_signal`

Expected: FAIL — `record_smart_money_signal is not a member of trait Database` / `SmartMoneySignal not found`.

- [ ] **Step 3: Implement the struct and trait methods**

In `infra/src/db_abstraction/types.rs` (follow the file's existing struct style — derives matching `Trade`):

```rust
/// One parsed swap of a tracked wallet, recorded pre-admission (Gate 0).
/// Bilateral: side is BUY or SELL. Idempotent per (tx_signature, token_address, side).
#[derive(Debug, Clone)]
pub struct SmartMoneySignal {
    pub wallet_address: String,
    pub token_address: String,
    pub token_symbol: Option<String>,
    pub side: String,
    pub amount_sol: Decimal,
    pub amount_tokens: Decimal,
    pub token_decimals: Option<i32>,
    pub price_usd: Option<Decimal>,
    pub price_sol: Option<Decimal>,
    pub tx_signature: String,
    pub slot: i64,
    pub block_time: chrono::DateTime<chrono::Utc>,
}
```

In `infra/src/db_abstraction/mod.rs`, add to the `Database` trait (with the other async methods):

```rust
/// Record a parsed tracked-wallet swap pre-admission. Idempotent.
async fn record_smart_money_signal(&self, signal: &SmartMoneySignal) -> AppResult<()>;

/// Distinct tracked wallets with BUY rows for the token in the trailing window.
async fn get_token_wallet_count(&self, token_address: &str, window_hours: i64) -> AppResult<i64>;
```

In `infra/src/db_abstraction/postgres.rs`, inside `impl Database for PostgresBackend`:

```rust
async fn record_smart_money_signal(&self, signal: &SmartMoneySignal) -> AppResult<()> {
    let _ = timed_query("record_smart_money_signal", || async {
        sqlx::query(
            r#"
            INSERT INTO smart_money_signals (
                wallet_address, token_address, token_symbol, side,
                amount_sol, amount_tokens, token_decimals,
                price_usd, price_sol, tx_signature, slot, block_time
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT (tx_signature, token_address, side) DO NOTHING
            "#,
        )
        .bind(&signal.wallet_address)
        .bind(&signal.token_address)
        .bind(&signal.token_symbol)
        .bind(&signal.side)
        .bind(signal.amount_sol)
        .bind(signal.amount_tokens)
        .bind(signal.token_decimals)
        .bind(signal.price_usd)
        .bind(signal.price_sol)
        .bind(&signal.tx_signature)
        .bind(signal.slot)
        .bind(signal.block_time)
        .execute(&*self.pool)
        .await
        .map_err(AppError::from)
    });
    Ok(())
}

async fn get_token_wallet_count(&self, token_address: &str, window_hours: i64) -> AppResult<i64> {
    let row: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(DISTINCT wallet_address)::BIGINT
        FROM smart_money_signals
        WHERE token_address = $1
          AND side = 'BUY'
          AND block_time > NOW() - make_interval(hours => $2)
        "#,
    )
    .bind(token_address)
    .bind(window_hours)
    .fetch_one(&*self.pool)
    .await
    .map_err(AppError::from)?;
    Ok(row.0)
}
```

(If `timed_query`'s signature doesn't match this call shape in the current code, copy the pattern from an adjacent single-INSERT method in the same impl block instead — the SQL is the contract, the wrapper is style.)

Add the two stubs to both mocks:

```rust
async fn record_smart_money_signal(&self, _signal: &SmartMoneySignal) -> AppResult<()> {
    Ok(())
}

async fn get_token_wallet_count(&self, _token_address: &str, _window_hours: i64) -> AppResult<i64> {
    Ok(0)
}
```

- [ ] **Step 4: Run test to verify it passes, then lint**

Run: `cargo test -p chimera_operator --lib test_mock_records_smart_money_signal && cargo clippy -p chimera_infra -p chimera_operator --all-targets --all-features -- -D warnings && cargo fmt`

Expected: test PASS; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add infra/src/db_abstraction/ operator/src/monitoring/test_db.rs infra/src/engine/kelly_sizer.rs
git commit -m "feat(infra): record_smart_money_signal + get_token_wallet_count on Database trait"
```

---

### Task 3: Wire pre-admission recording into the Helius webhook path

**Files:**
- Modify: `operator/src/handlers/monitoring.rs` — inside `helius_webhook_handler`, immediately after a swap is parsed and a tracked wallet is matched (the block at `if let Ok(Some(swap)) = parsed {` / `let wallet_address = match tracked_wallet_ref { ... }`), BEFORE the circuit-breaker check.
- Test: `operator/tests/integration/smart_money_cluster_tests.rs` (file already exists — add a test module there; it is wired for these scenarios).

**Interfaces:**
- Consumes: `SmartMoneySignal` + `record_smart_money_signal` (Task 2); existing `ParsedSwap` (`swap.token_in`, `swap.token_out`, `swap.amount_in`, `swap.amount_out`, `swap.direction`) and `event.signature`, `event.slot`, `event.timestamp` already in scope in the handler.
- Produces: every tracked-wallet swap lands in `smart_money_signals` ≤1 row per (signature, token, side). A `SMRecordingFailure` counter-free `tracing::error!` marker `"smart_money_signal_record_failed"` for ops dashboards.

- [ ] **Step 1: Write the failing test**

Append to `operator/tests/integration/smart_money_cluster_tests.rs` (mirror the file's existing fixture-building style — if it builds `HeliusWebhookPayload` values, reuse that helper):

```rust
/// Gate 0: a parsed BUY from a tracked wallet must be recorded into
/// smart_money_signals BEFORE any admission gating — i.e. even when the
/// circuit breaker blocks the signal, the recording spawn must have been
/// issued. This test asserts the recording call happens on the
/// circuit-breaker-blocked path (the earliest gate).
#[tokio::test]
async fn test_signal_recorded_before_circuit_breaker() {
    // Build a MonitoringState with MockDb + a tripped circuit breaker.
    // (Reuse the existing test fixture in this file that constructs
    // MonitoringState with MockDb; if none exists, construct
    // MonitoringState::default() and override circuit_breaker with a
    // tripped instance as the other tests do.)
    let state = test_state_with_tripped_breaker();
    let event = sample_buy_event(); // existing helper: tracked wallet BUY

    let status = crate::handlers::monitoring::helius_webhook_handler(
        axum::extract::State(state.clone()),
        axum::http::HeaderMap::new(),
        axum::Json(vec![event]),
    )
    .await;

    assert_eq!(status, axum::http::StatusCode::OK);
    // Recording is fire-and-forget; give the spawn a beat, then the mock
    // must have seen the record call.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        state.db_mock.saw_record_smart_money_signal(),
        "signal must be recorded even when the breaker blocks admission"
    );
}
```

Implementation note: `MockDb` needs an `Arc<Mutex<Vec<SmartMoneySignal>>>` sink + `saw_record_smart_money_signal()` accessor if the fixture's DB handle is shared — extend the stub from Task 2 to record into that sink (keep it `#[cfg(test)]`-friendly by making the sink always-on; it is a Vec push, not a DB call). If `helius_webhook_handler` is not publicly exported for tests, add `#[cfg(any(test, feature = "integration-tests"))] pub` visibility following how other handlers are tested in this file.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p chimera_operator --test smart_money_cluster_tests test_signal_recorded_before_circuit_breaker`

Expected: FAIL — `saw_record_smart_money_signal()` returns false (nothing wired yet) or compile error on the missing fixture helper.

- [ ] **Step 3: Implement the recording spawn**

In `helius_webhook_handler`, inside `if let Ok(Some(swap)) = parsed {`, right after `wallet_address` resolves to `Some` and before the circuit-breaker block:

```rust
// ── Gate 0: durable pre-admission signal recording ───────────────────
// Every parsed swap of a tracked wallet is recorded BEFORE circuit
// breaker / selection / admission. Fire-and-forget: recording must never
// add webhook latency (Helius retry storms) and must never fail the
// event. Idempotent via UNIQUE (tx_signature, token_address, side).
let side = match swap.direction {
    crate::monitoring::SwapDirection::Buy => "BUY",
    crate::monitoring::SwapDirection::Sell => "SELL",
};
let target_token = if side == "BUY" {
    swap.token_out.clone()
} else {
    swap.token_in.clone()
};
let amount_sol = swap.amount_in; // SOL leg (UI units, Decimal)
let amount_tokens = swap.amount_out; // token leg as parsed (see decimals note)
let signal = chimera_infra::db_abstraction::types::SmartMoneySignal {
    wallet_address: wallet_address.clone(),
    token_address: target_token.clone(),
    token_symbol: swap.token_symbol.clone(),
    side: side.to_string(),
    amount_sol,
    amount_tokens,
    token_decimals: swap.token_decimals, // add to ParsedSwap if absent — see Step 4
    price_usd: None,                     // enriched downstream; recording never blocks on pricing
    price_sol: None,
    tx_signature: event.signature.clone(),
    slot: event.slot,
    block_time: chrono::DateTime::<chrono::Utc>::from_timestamp(event.timestamp.max(0), 0)
        .unwrap_or_else(chrono::Utc::now),
};
let db = state.db.clone();
let sig_for_log = event.signature.clone();
tokio::spawn(async move {
    if let Err(e) = db.record_smart_money_signal(&signal).await {
        tracing::error!(
            signature = %sig_for_log,
            error = %e,
            "smart_money_signal_record_failed"
        );
    }
});
```

Unit rule: `amount_in`/`amount_out` are `Decimal` already — no floats.

- [ ] **Step 4: Plumb `token_decimals` through `ParsedSwap`**

The audit found the parser mixes `uiAmount` and raw `token_amount` paths (`infra/src/monitoring/transaction_parser.rs:315-318` uses `uiAmount`; `:458` uses raw strings). Recording without knowing which unit a leg is would poison later analysis.

1. Add `pub token_decimals: Option<i32>` to `ParsedSwap` in `infra/src/monitoring/transaction_parser.rs` (default `None` at every construction site).
2. Populate it in each parse path from the payload's `tokenBalanceChanges[].uiTokenAmount.decimals` (raw path) or `tokenTransfers[].tokenAmount` context decimals — the same field the leg's amount was read from, so unit and decimals always describe the same leg.
3. Do NOT convert units in this task — the column stores the parsed value + its decimals so any consumer normalizes explicitly.

- [ ] **Step 5: Run test to verify it passes, then full suite + lint**

Run: `cargo test -p chimera_operator --test smart_money_cluster_tests && cargo test -p chimera_operator --lib && cargo clippy -p chimera_operator --all-targets --all-features -- -D warnings && cargo fmt`

Expected: new test PASS, no regressions, clippy clean.

- [ ] **Step 6: Commit**

```bash
git add operator/src/handlers/monitoring.rs operator/src/monitoring/test_db.rs infra/src/monitoring/transaction_parser.rs operator/tests/integration/smart_money_cluster_tests.rs
git commit -m "feat(monitoring): pre-admission smart_money_signals recording, fire-and-forget (Gate 0)"
```

---

### Task 4: DB-backed consensus attribution on decision records

**Files:**
- Modify: `operator/src/engine/selection.rs` — the step-7 consensus block (lines ~1403-1430: `let mut consensus_wallet_count ...` through `is_smart_money_cluster`).
- Test: `operator/tests/unit/` (new file `consensus_attribution_tests.rs`, mirroring the layout of existing unit tests referenced in `operator/tests/`).

**Interfaces:**
- Consumes: `get_token_wallet_count` (Task 2), `self.db: Arc<dyn Database>` already held by `SelectionService` (if the service doesn't hold `db`, add it as a constructor param `db: Arc<dyn Database>` threaded from the same `Arc` the engine passes to `SignalProcessor::new` — `SelectionService::new` call sites are in `operator/src/lib.rs` and `monitoring/mod.rs`).
- Produces: `decision_records.consensus_wallet_count` becomes `Some(count)` where `count = max(aggregator 5-min count, db 12h distinct-wallet count)` — durable across restarts and roster churn. `ln` on the decision record is this same value (existing mapping in `decision_recorder.rs`), so the shadow book becomes cluster-attributable without schema change.

- [ ] **Step 1: Write the failing test**

```rust
//! operator/tests/unit/consensus_attribution_tests.rs
//! Gate 0: consensus_wallet_count must reflect the durable 12h
//! distinct-BUY-wallet count, not only the in-memory 5-minute window.

use rust_decimal::Decimal;

#[tokio::test]
async fn test_consensus_count_uses_db_window_when_aggregator_cold() {
    // SelectionService with: no signal_aggregator (the production
    // configuration that produced 178K NULLs) and a mock Database whose
    // get_token_wallet_count returns 3.
    let db = std::sync::Arc::new(db_mock_with_wallet_count(3));
    let service = selection_service_with_db_only(db);
    let decision = service
        .decide(&sample_buy_request("tok-with-history"))
        .await;
    assert_eq!(
        decision.consensus_wallet_count,
        Some(3),
        "DB-backed 12h count must populate consensus even without the aggregator"
    );
}

#[tokio::test]
async fn test_consensus_count_is_max_of_aggregator_and_db() {
    // aggregator warm with 2, db says 4 → decision must carry 4.
    let db = std::sync::Arc::new(db_mock_with_wallet_count(4));
    let service = selection_service_with(db, Some(aggregator_with_count(2)));
    let decision = service.decide(&sample_buy_request("tok-x")).await;
    assert_eq!(decision.consensus_wallet_count, Some(4));
}
```

The mock helpers follow the same construction pattern the existing selection unit tests use (locate them via `rg "fn.*selection" operator/tests/unit/` and reuse their fixture builders; extend the mock `Database` from `operator/src/monitoring/test_db.rs` with a settable wallet-count field).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p chimera_operator --test unit consensus_`

Expected: FAIL — current code returns `None` when the aggregator is absent.

- [ ] **Step 3: Implement**

Replace the step-7 block in `selection.rs` (keep `is_consensus` semantics unchanged — this is an attribution fix, not a gate change):

```rust
// ── 7. Consensus detection (read-only peek + durable attribution) ────
// In-memory peek covers the hot 5-minute window. The DB-backed count
// covers the 12h durable window from smart_money_signals (Gate 0): with a
// small roster the aggregator alone produced 178K NULL consensus counts,
// making cluster behaviour unobservable in the shadow book. Attribution
// only — the is_consensus GATE still requires the 5-min aggregator signal.
let mut consensus_wallet_count: Option<usize> = None;
let is_consensus = if let Some(ref aggregator) = self.signal_aggregator {
    let count = aggregator.peek_consensus_wallet_count(&req.token_address).await;
    consensus_wallet_count = Some(count.max(1));
    count >= 2
} else {
    false
};

// Durable 12h distinct-wallet count (fails open to the aggregator value).
match self.db.get_token_wallet_count(&req.token_address, 12).await {
    Ok(db_count) => {
        let db_count = db_count.max(0) as usize;
        consensus_wallet_count = Some(
            consensus_wallet_count.unwrap_or(1).max(db_count),
        );
    }
    Err(e) => {
        tracing::warn!(
            token = %req.token_address,
            error = %e,
            "consensus_db_count_failed (attribution falls back to aggregator)"
        );
    }
}
```

Then the existing `is_smart_money_cluster` block stays untouched.

- [ ] **Step 4: Run tests + lint + full suite**

Run: `cargo test -p chimera_operator --test unit consensus_ && cargo test -p chimera_operator --lib && cargo clippy -p chimera_operator --all-targets --all-features -- -D warnings && cargo fmt`

Expected: PASS, no regressions, clippy clean. (Perf note: one indexed COUNT DISTINCT per decision against `idx_smart_signals_token_window` — same order as the existing per-decision DB reads.)

- [ ] **Step 5: Commit**

```bash
git add operator/src/engine/selection.rs operator/tests/unit/consensus_attribution_tests.rs
git commit -m "fix(selection): durable 12h DB consensus attribution — kills 178K NULL consensus counts (Gate 0)"
```

---

### Task 5: Pre-registered cluster re-validation script (Gate 1 instrument)

**Files:**
- Create: `scout/scripts/cluster_revalidation.py`
- Test: `scout/tests/test_cluster_revalidation.py`

**Interfaces:**
- Consumes: prod tables `shadow_positions`, `shadow_exits` (read-only, via `scout/analysis/db.py::connect`).
- Produces: `main(days, window_hours, min_wallets, min_gap_s, min_span_s, bootstrap_n, seed) -> dict` of verdict metrics; CLI writing a markdown table to stdout. The Gate 1 verdict doc consumes this output verbatim.

- [ ] **Step 1: Write the failing tests (pure attribution logic)**

The DB-reading parts are thin; all logic lives in two pure functions that get real tests. Property-based test included per repo convention (Hypothesis is already a scout dependency — verify in `scout/pyproject.toml`; if absent, drop the hypothesis test and keep the example-based ones).

```python
"""Tests for the pre-registered cluster re-validation logic."""
import datetime as dt

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from scout.scripts.cluster_revalidation import (
    bucket_for_count,
    bootstrap_ci,
    passes_dispersion,
)


# ── passes_dispersion: exists i<j<k with span>=min_span, gaps>=min_gap ──
def test_dispersion_three_wallets_exact_bounds():
    base = dt.datetime(2026, 9, 1, tzinfo=dt.timezone.utc)
    arrivals = [base, base + dt.timedelta(seconds=5), base + dt.timedelta(seconds=185)]
    assert passes_dispersion(arrivals, min_gap_s=5, min_span_s=180) is True


def test_dispersion_rejects_short_span():
    base = dt.datetime(2026, 9, 1, tzinfo=dt.timezone.utc)
    arrivals = [base, base + dt.timedelta(seconds=5), base + dt.timedelta(seconds=35)]
    assert passes_dispersion(arrivals, min_gap_s=5, min_span_s=180) is False


def test_dispersion_rejects_sub_gap_triple_but_accepts_valid_subset():
    # t=0, 3s, 200s, 300s: consecutive triple (0,3,200) fails the 5s gap,
    # but subset {0, 200, 300} is valid → must return True.
    base = dt.datetime(2026, 9, 1, tzinfo=dt.timezone.utc)
    arrivals = [
        base,
        base + dt.timedelta(seconds=3),
        base + dt.timedelta(seconds=200),
        base + dt.timedelta(seconds=300),
    ]
    assert passes_dispersion(arrivals, min_gap_s=5, min_span_s=180) is True


def test_dispersion_fewer_than_three_wallets():
    base = dt.datetime(2026, 9, 1, tzinfo=dt.timezone.utc)
    assert passes_dispersion([base, base + dt.timedelta(seconds=300)], 5, 180) is False


@settings(max_examples=200, deadline=None)
@given(
    st.lists(
        st.integers(min_value=0, max_value=10_000), min_size=0, max_size=12
    )
)
def test_dispersion_matches_brute_force_definition(offsets):
    """Brute-force oracle: any 3-combination with span & gaps in bounds."""
    import itertools

    arrivals = [
        dt.datetime(2026, 9, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=s)
        for s in sorted(offsets)
    ]
    def brute(secs):
        for a, b, c in itertools.combinations(secs, 3):
            if c - a >= 180 and b - a >= 5 and c - b >= 5:
                return True
        return False

    assert passes_dispersion(arrivals, 5, 180) == brute(sorted(set(offsets)))


# ── bucket_for_count ──────────────────────────────────────────────────
@pytest.mark.parametrize(
    "count,expected",
    [(1, "1"), (2, "2"), (3, "3+"), (7, "3+"), (None, "unattributed")],
)
def test_bucket_for_count(count, expected):
    assert bucket_for_count(count) == expected


# ── bootstrap_ci: deterministic under seed, percentile bounds ─────────
def test_bootstrap_ci_deterministic_and_sane():
    vals = [1.0, 2.0, 3.0, 4.0, 5.0]
    lo, hi = bootstrap_ci(vals, n_boot=2000, seed=42)
    assert lo <= hi
    # Re-run with same seed → identical bounds.
    lo2, hi2 = bootstrap_ci(vals, n_boot=2000, seed=42)
    assert (lo, hi) == (lo2, hi2)
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `python -m pytest scout/tests/test_cluster_revalidation.py -v`

Expected: FAIL — `ModuleNotFoundError: No module named 'scout.scripts.cluster_revalidation'`.

- [ ] **Step 3: Implement the script**

```python
"""Gate 1: pre-registered re-validation of the smart-money cluster hypothesis.

Replaces the unreproducible §1.2 table of the held cluster-accumulation plan.
Cluster attribution is a per-position self-join over the shadow book: a
position's cluster size is the number of DISTINCT tracked wallets that
entered the same token within the trailing window before (and including)
its own entry, optionally filtered by the temporal-dispersion rule.

Pre-registered defaults (do not tune after seeing results):
    window_hours=12, min_wallets=3, min_gap_s=5, min_span_s=180,
    bootstrap_n=2000, seed=20260906

Verdict rule (pre-registered): strategy×bucket is GO-evidence iff
n >= 300 AND avg pnl_pct > 0 AND bootstrap 95% CI lower bound > 0.
If no wallet_sell/fixed_24h/fixed_4h 3+ bucket meets the rule → PIVOT.
"""
import argparse
import datetime as dt
import itertools
import json
import sys
from decimal import Decimal

from scout.analysis.db import connect

MIN_GAP_S = 5
MIN_SPAN_S = 180
STRATEGIES = ("wallet_sell", "fixed_24h", "fixed_4h")


def passes_dispersion(arrivals, min_gap_s: int, min_span_s: int) -> bool:
    """True iff some i<j<k among sorted arrivals has span & both gaps in bounds.

    O(N^3) over N <= a few dozen arrivals per token-window — trivial, and
    matches the brute-force definition exactly (verified by the hypothesis
    test), unlike the two-pointer variant proposed in plan review which had
    a subtle middle-element bug for degenerate inputs.
    """
    secs = sorted(int(a.timestamp()) for a in arrivals)
    return any(
        c - a >= min_span_s and b - a >= min_gap_s and c - b >= min_gap_s
        for a, b, c in itertools.combinations(secs, 3)
    )


def bucket_for_count(count):
    if count is None:
        return "unattributed"
    if count <= 1:
        return "1"
    if count == 2:
        return "2"
    return "3+"


def bootstrap_ci(values, n_boot: int, seed: int, confidence: float = 0.95):
    import random

    rng = random.Random(seed)
    n = len(values)
    if n == 0:
        return (0.0, 0.0)
    means = sorted(
        sum(rng.choice(values) for _ in range(n)) / n for _ in range(n_boot)
    )
    alpha = (1.0 - confidence) / 2.0
    idx_lo = int(alpha * n_boot)
    idx_hi = min(n_boot - 1, int((1.0 - alpha) * n_boot))
    return (means[idx_lo], means[idx_hi])


ATTRIBUTION_SQL = """
WITH tracked AS (
    SELECT s.shadow_id,
           s.wallet_address,
           s.token_address,
           s.opened_at,
           s.liquidity_usd
    FROM shadow_positions s
    WHERE s.opened_at > NOW() - make_interval(days => %s)
      AND s.strategy IS NOT DISTINCT FROM COALESCE(%s, s.strategy)
),
arrivals AS (
    SELECT t.shadow_id, o.wallet_address, MIN(o.opened_at) AS arrival
    FROM tracked t
    JOIN tracked o
      ON o.token_address = t.token_address
     AND o.opened_at BETWEEN t.opened_at - make_interval(hours => %s)
                         AND t.opened_at
    GROUP BY t.shadow_id, o.wallet_address
)
SELECT a.shadow_id,
       COUNT(DISTINCT a.wallet_address) AS cluster_size,
       ARRAY_AGG(a.arrival ORDER BY a.arrival) AS arrivals,
       MIN(t.liquidity_usd) AS liquidity_usd
FROM arrivals a
JOIN tracked t USING (shadow_id)
GROUP BY a.shadow_id
"""


def load_attributions(days: int, window_hours: int, strategy):
    """[(shadow_id, cluster_size, [arrival,...], liquidity_usd)]"""
    with connect() as conn, conn.cursor() as cur:
        cur.execute(ATTRIBUTION_SQL, (days, strategy, window_hours))
        return cur.fetchall()


def summarize(rows, apply_dispersion, min_gap_s, min_span_s, bootstrap_n, seed):
    """bucket -> dict(n, win_rate, avg_pnl, ci_lo, ci_hi) over joined exits."""
    from collections import defaultdict

    pnl_by_bucket = defaultdict(list)
    with connect() as conn, conn.cursor() as cur:
        for shadow_id, size, arrivals, _liq in rows:
            if apply_dispersion and (
                size < 3 or not passes_dispersion(arrivals, min_gap_s, min_span_s)
            ):
                continue
            cur.execute(
                "SELECT COALESCE(pnl_pct, 0)::float8 FROM shadow_exits "
                "WHERE shadow_id = %s",
                (shadow_id,),
            )
            pnl_by_bucket[bucket_for_count(size)].extend(r[0] for r in cur.fetchall())

    out = {}
    for bucket, pnls in pnl_by_bucket.items():
        n = len(pnls)
        ci_lo, ci_hi = bootstrap_ci(pnls, bootstrap_n, seed)
        out[bucket] = {
            "n": n,
            "win_rate": sum(1 for p in pnls if p > 0) / n if n else 0.0,
            "avg_pnl": sum(pnls) / n if n else 0.0,
            "ci_lo": ci_lo,
            "ci_hi": ci_hi,
        }
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--days", type=int, default=30)
    ap.add_argument("--window-hours", type=int, default=12)
    ap.add_argument("--min-gap-s", type=int, default=MIN_GAP_S)
    ap.add_argument("--min-span-s", type=int, default=MIN_SPAN_S)
    ap.add_argument("--bootstrap-n", type=int, default=2000)
    ap.add_argument("--seed", type=int, default=20260906)
    ap.add_argument("--json", action="store_true", help="emit JSON instead of markdown")
    args = ap.parse_args()

    results = {}
    for strategy in STRATEGIES:
        rows = load_attributions(args.days, args.window_hours, strategy)
        results[strategy] = {
            "all": summarize(rows, False, args.min_gap_s, args.min_span_s,
                             args.bootstrap_n, args.seed),
            "dispersion": summarize(rows, True, args.min_gap_s, args.min_span_s,
                                    args.bootstrap_n, args.seed),
        }

    if args.json:
        print(json.dumps(results, indent=2, default=float))
        return 0

    print(f"# Cluster re-validation ({args.days}d, {args.window_hours}h window)\n")
    go = False
    for strategy, by_mode in results.items():
        print(f"## {strategy}\n\n| bucket | n | win% | avg pnl | CI95 | dispersion |")
        print("|---|---|---|---|---|---|")
        for mode, buckets in by_mode.items():
            for bucket, m in buckets.items():
                print(f"| {bucket} | {m['n']} | {m['win_rate']:.1%} "
                      f"| {m['avg_pnl']:+.2f} "
                      f"| [{m['ci_lo']:+.2f}, {m['ci_hi']:+.2f}] "
                      f"| {mode} |")
        cluster = by_mode["dispersion"].get("3+") or {}
        if (cluster.get("n", 0) >= 300 and cluster.get("avg_pnl", 0) > 0
                and cluster.get("ci_lo", -1) > 0):
            go = True
        print()

    print("## Verdict\n")
    if go:
        print("GO-EVIDENCE: a 3+ dispersion-valid bucket cleared the "
              "pre-registered bar. Proceed to Gate 2 review.")
        return 0
    print("PIVOT: no 3+ cluster bucket cleared the pre-registered bar. "
          "The cluster-accumulation engine stays HELD; strategy pivot review "
          "required (e.g. solo high-conviction swingers / dune_wallet revival).")
    return 1


if __name__ == "__main__":
    sys.exit(main())
```

Note on `Decimal`: the SQL casts `pnl_pct` to `float8` for statistical aggregation (means/percentiles over 10K+ samples — float is the correct tool for statistics; the *values* were stored as NUMERIC and money sizing is never derived here). This comment belongs in the script and satisfies the repo's financial-precision rule.

- [ ] **Step 4: Run tests to verify they pass**

Run: `python -m pytest scout/tests/test_cluster_revalidation.py -v && make lint-scout`

Expected: all tests PASS; lint clean.

- [ ] **Step 5: Commit**

```bash
git add scout/scripts/cluster_revalidation.py scout/tests/test_cluster_revalidation.py
git commit -m "feat(scout): pre-registered cluster re-validation script with bootstrap CI (Gate 1)"
```

---

### Task 6: Run Gate 1 on production and record the verdict

**Files:**
- Create: `docs/superpowers/analysis/2026-09-XX-cluster-revalidation-results.md` (date the actual run)
- Modify: `docs/superpowers/plans/2026-09-06-smart-money-cluster-accumulation.md` (hold-notice verdict line only)

**Interfaces:**
- Consumes: Task 5 script; SSH access `root@chimera-01.moez.tech`; `DATABASE_URL` on prod for scout (existing scout container has it).
- Produces: the GO/PIVOT verdict that gates the held cluster plan.

- [ ] **Step 1: Run the script on production**

```bash
ssh root@chimera-01.moez.tech "cd /opt/chimera && docker compose exec -T scout \
  python -m scout.scripts.cluster_revalidation --days 30 --json" \
  > docs/superpowers/analysis/2026-09-06-cluster-revalidation-raw.json
ssh root@chimera-01.moez.tech "cd /opt/chimera && docker compose exec -T scout \
  python -m scout.scripts.cluster_revalidation --days 30" \
  > docs/superpowers/analysis/2026-09-06-cluster-revalidation-results.md
```

If the scout container lacks the script (image not rebuilt), run with a one-off container or locally against a read-only prod DATABASE_URL through an SSH tunnel — the script is read-only and parameterized by env (`DATABASE_URL`/`CHIMERA_DB_URL`), matching `scout/analysis/db.py`.

- [ ] **Step 2: Sanity-check the output against the audit baseline**

Expected ballpark (from the 2026-09-06 audit self-join): wallet_sell 3+ ≈ n≈761, avg ≈ −5.3; the script's `all` rows should land near those numbers. If they diverge wildly (>2×), re-check the window interval parameterization before trusting the verdict.

- [ ] **Step 3: Record the verdict**

In `docs/superpowers/analysis/2026-09-06-cluster-revalidation-results.md`, append:

```markdown
## Verdict (pre-registered rule applied)

- Rule: GO iff any wallet_sell/fixed_24h/fixed_4h "3+" dispersion-valid bucket
  has n ≥ 300 AND avg pnl > 0 AND bootstrap 95% CI lower bound > 0.
- Outcome: [GO-EVIDENCE | PIVOT] (filled from script exit + table above)
- Decision: [un-hold cluster plan → Gate 2 | strategy pivot review]
- Run metadata: git sha, script flags, DB snapshot timestamp.
```

In the held plan doc's hold notice, add one line under the Gate sequence: `> **Gate 1 outcome (YYYY-MM-DD):** [PIVOT|GO] — see docs/superpowers/analysis/2026-09-06-cluster-revalidation-results.md.`

- [ ] **Step 4: Commit and push (merge flow per root AGENTS.md)**

```bash
git add docs/superpowers/analysis/ docs/superpowers/plans/2026-09-06-smart-money-cluster-accumulation.md
git commit -m "docs: Gate 1 cluster re-validation results — [PIVOT|GO]"
git push origin main
```

---

## Self-Review

1. **Spec coverage:** Gate 0 items → Task 1 (durable table), Task 3 (pre-admission recording), Task 4 (consensus attribution — the exact defect producing 178K NULLs). Gate 1 → Task 5 (pre-registered instrument incl. dispersion rule from the held plan + audit) and Task 6 (run + verdict). §1.2 provenance → declared unrecoverable in the hold notice (Task 6 documents the replacement). Gate 2 engine fixes intentionally out of scope, per the gate sequence.
2. **Placeholder scan:** Task 3 Step 1 references "existing fixture" in `smart_money_cluster_tests.rs` — implementer must read that file first (it exists and already covers cluster scenarios); the test body is complete given the fixture. No TBD/TODO markers.
3. **Type consistency:** `SmartMoneySignal` fields identical across Task 2 struct/SQL/mocks/tests; `get_token_wallet_count(token_address: &str, window_hours: i64) -> AppResult<i64>` matches Task 4 call site; Python `passes_dispersion(arrivals, min_gap_s, min_span_s)` matches tests and `summarize` call; bucket strings `1|2|3+|unattributed` consistent between `bucket_for_count` and the verdict reader.
