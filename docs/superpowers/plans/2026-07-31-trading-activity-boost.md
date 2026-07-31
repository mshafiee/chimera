# Trading Activity Boost Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Increase trading activity by lowering WQS thresholds, improving wallet discovery quality, and adding risk-adjusted position sizing tiers for lower-WQS wallets.

**Architecture:** Three-phase approach. Phase 1 is a config-only change that immediately unblocks trading by lowering WQS/confidence gates in both scout and operator. Phase 2 improves discovery quality by adding a wallet-age filter and fixing the discovery-hours default mismatch. Phase 3 adds a new SPEAR-LITE position-sizing tier so lower-WQS wallets trade with micro-positions while accumulating a track record.

**Tech Stack:** Python (scout), Rust (operator), Docker Compose (deployment config), PostgreSQL (wallet data)

## Global Constraints

- **Financial precision:** Never use float/double for money. Use `rust_decimal::Decimal` (Rust) or `Decimal` (Python).
- **Deployment:** All changes deployed via git workflow — commit, push, server pull, `docker compose build`, `docker compose up -d --force-recreate`.
- **Config precedence:** Env vars in `docker-compose.yml` override code defaults. Scout reads via `os.getenv()`, operator reads via `std::env::var()`.
- **Test commands:** Rust: `cd operator && cargo test <name> -- --test-threads=1`. Python: `cd scout && python -m pytest tests/test_file.py::test_name -v`.
- **Both components must agree:** Scout's `SCOUT_MIN_WQS_ACTIVE` and Operator's `CHIMERA_SELECTION__MIN_WQS_SCORE` must be set to the same value, otherwise wallets get promoted but never traded.

## Current State (Baseline)

| Config | Current Value | Location |
|--------|--------------|----------|
| `SCOUT_MIN_WQS_ACTIVE` | 25.0 | `docker-compose.yml:201` |
| `SCOUT_MIN_WQS_CANDIDATE` | 10.0 | `docker-compose.yml:202` |
| `SCOUT_MIN_CONFIDENCE_ACTIVE` | 0.30 | `docker-compose.yml:206` |
| `CHIMERA_SELECTION__MIN_WQS_SCORE` | 25.0 | `docker-compose.yml:140` |
| `SCOUT_DISCOVERY_HOURS` | 24 | `docker-compose.yml:191` |
| `SCOUT_MAX_WALLETS` | 300 | `docker-compose.yml:190` |
| Operator `min_wqs_score` default | 70.0 | `operator/src/engine/selection.rs:902` |
| SHIELD threshold (hardcoded) | WQS ≥ 80.0 | `operator/src/engine/selection.rs:524` |
| SPEAR max position | 0.5 SOL | `operator/src/config.rs` (shield_max=2.0, spear_max=0.5) |

**WQS distribution (5067 wallets analyzed):** 94.3% in 0-20, 1.9% in 20-40, 1.4% in 40-60, 0.5% in 60-80, 0.6% in 80-100. Only 19 wallets (0.38%) have WQS > 40.

---

## Phase 1: Lower Thresholds (Quick Fix)

**Rationale:** Currently only wallets with WQS ≥ 25 AND confidence ≥ 0.30 pass. Lowering to WQS ≥ 15 and confidence ≥ 0.20 expands the qualifying pool from ~19 wallets to ~40+. The operator's position sizer already scales size by `wqs_factor = wqs/100`, so WQS-15 wallets get ~15% of base size automatically. SPEAR strategy caps at 0.5 SOL, limiting downside.

**Risk mitigation:** All lower-WQS wallets route to SPEAR (aggressive, small positions, wider slippage, load-sheddable). This is by design — the barbell strategy protects capital.

### Task 1.1: Lower Scout Promotion Thresholds

**Files:**
- Modify: `docker-compose.yml:201,206`

**Interfaces:**
- Consumes: existing `SCOUT_MIN_WQS_ACTIVE`, `SCOUT_MIN_CONFIDENCE_ACTIVE` env vars
- Produces: more wallets promoted to ACTIVE status in the `wallets` table

- [ ] **Step 1: Lower SCOUT_MIN_WQS_ACTIVE and SCOUT_MIN_CONFIDENCE_ACTIVE**

In `docker-compose.yml`, change these two lines in the `scout` service environment block:

```yaml
# Line 201 — was 25.0
- SCOUT_MIN_WQS_ACTIVE=15.0
# Line 206 — was 0.30
- SCOUT_MIN_CONFIDENCE_ACTIVE=0.20
```

- [ ] **Step 2: Verify no other references to these specific values**

Run: `rg "SCOUT_MIN_WQS_ACTIVE|SCOUT_MIN_CONFIDENCE_ACTIVE" docker-compose*.yml`
Expected: Only the scout service block references these, values now 15.0 and 0.20.

- [ ] **Step 3: Commit**

```bash
git add docker-compose.yml
git commit -m "feat(scout): lower WQS/confidence thresholds for ACTIVE promotion

SCOUT_MIN_WQS_ACTIVE: 25.0 → 15.0
SCOUT_MIN_CONFIDENCE_ACTIVE: 0.30 → 0.20

Expands qualifying pool from ~19 to ~40 wallets. Lower-WQS wallets
route to SPEAR strategy (max 0.5 SOL) via operator selection service."
```

### Task 1.2: Lower Operator Selection WQS Gate

**Files:**
- Modify: `docker-compose.yml:140`

**Interfaces:**
- Consumes: `CHIMERA_SELECTION__MIN_WQS_SCORE` env var (read at `operator/src/main.rs:348`)
- Produces: operator admits wallets with WQS ≥ 15.0 instead of rejecting at 25.0

- [ ] **Step 1: Lower CHIMERA_SELECTION__MIN_WQS_SCORE**

In `docker-compose.yml`, change this line in the `operator` service environment block:

```yaml
# Line 140 — was 25.0
- CHIMERA_SELECTION__MIN_WQS_SCORE=15.0
```

- [ ] **Step 2: Verify alignment with scout threshold**

Run: `rg "MIN_WQS_SCORE|MIN_WQS_ACTIVE" docker-compose.yml`
Expected: Both `CHIMERA_SELECTION__MIN_WQS_SCORE` (operator) and `SCOUT_MIN_WQS_ACTIVE` (scout) show 15.0.

- [ ] **Step 3: Commit**

```bash
git add docker-compose.yml
git commit -m "feat(operator): lower selection WQS gate to match scout threshold

CHIMERA_SELECTION__MIN_WQS_SCORE: 25.0 → 15.0

Aligns operator admission gate with scout promotion threshold.
Without this, scout promotes wallets that operator immediately rejects."
```

### Task 1.3: Deploy Phase 1 and Verify

**Files:**
- No code changes — deploy only

- [ ] **Step 1: Push to remote**

```bash
git push origin main
```

- [ ] **Step 2: Pull on server and recreate both containers**

```bash
ssh root@chimera-01.moez.tech "cd /opt/chimera && git pull origin main && docker compose up -d --force-recreate scout operator"
```

- [ ] **Step 3: Wait 5 minutes for scout to run one analysis cycle, then check WQS distribution**

```bash
ssh root@chimera-01.moez.tech "curl -s 'http://localhost:8080/api/v1/scout/status' | python3 -m json.tool"
```

Expected: `wallets_analyzed` increases. WQS distribution buckets 20-40, 40-60 should grow.

- [ ] **Step 4: Check operator is admitting more wallets (not rejecting WQS_TOO_LOW)**

```bash
ssh root@chimera-01.moez.tech "docker exec chimera-operator tail -200 /app/data/logs/operator.log | grep -c 'WQS_TOO_LOW'"
```

Expected: Fewer `WQS_TOO_LOW` rejections than before.

- [ ] **Step 5: Check for new trades**

```bash
ssh root@chimera-01.moez.tech "curl -s 'http://localhost:8080/api/v1/health' | python3 -m json.tool"
```

Expected: `last_trade_at` timestamp updates to within the last hour.

---

## Phase 2: Improve Discovery Quality (Long-term)

**Rationale:** 94% of discovered wallets have WQS < 20 because discovery has no wallet-age filter — it surfaces brand-new wallets with insufficient track record. Adding a 7-day minimum account-age filter at discovery time will surface more established wallets with longer trade histories, yielding higher WQS scores and confidence. Also fixes the `SCOUT_DISCOVERY_HOURS` default mismatch (168 in config.py vs 24 in analyzer.py).

### Task 2.1: Add Wallet Age Filter to Discovery

**Files:**
- Modify: `scout/core/helius_client.py:2558-2610` (post-discovery validation pipeline)
- Test: `scout/tests/test_discovery_wallet_age_filter.py` (create)

**Interfaces:**
- Consumes: `SCOUT_MIN_WALLET_AGE_DAYS` env var (new, default 0 = disabled)
- Produces: `_filter_by_wallet_age()` method that removes wallets younger than threshold

- [ ] **Step 1: Write failing test for wallet age filter**

Create `scout/tests/test_discovery_wallet_age_filter.py`:

```python
"""Tests for wallet age filtering during discovery."""
import pytest
from unittest.mock import AsyncMock, MagicMock, patch
from datetime import datetime, timedelta, timezone
from scout.core.helius_client import HeliusClient


@pytest.mark.asyncio
async def test_filter_by_wallet_age_removes_young_wallets():
    """Wallets younger than min_age_days should be filtered out."""
    client = MagicMock(spec=HeliusClient)
    client._filter_by_wallet_age = HeliusClient._filter_by_wallet_age.__get__(client)

    now = datetime.now(timezone.utc)
    # Wallet created 3 days ago — should be filtered if min_age=7
    young_wallet = "YoungWal11111111111111111111111111111111111"
    # Wallet created 30 days ago — should pass
    old_wallet = "OldWall222222222222222222222222222222222222"

    creation_times = {
        young_wallet: (now - timedelta(days=3)).timestamp(),
        old_wallet: (now - timedelta(days=30)).timestamp(),
    }

    client._get_wallet_creation_timestamps_batch = AsyncMock(
        return_value=creation_times
    )

    result = await client._filter_by_wallet_age(
        [young_wallet, old_wallet], min_age_days=7
    )

    assert young_wallet not in result
    assert old_wallet in result


@pytest.mark.asyncio
async def test_filter_by_wallet_age_disabled_when_zero():
    """When min_age_days=0, all wallets pass (filter disabled)."""
    client = MagicMock(spec=HeliusClient)
    client._filter_by_wallet_age = HeliusClient._filter_by_wallet_age.__get__(client)

    wallets = ["WallA1111111111111111111111111111111111111", "WallB2222222222222222222222222222222222222"]

    result = await client._filter_by_wallet_age(wallets, min_age_days=0)

    assert len(result) == len(wallets)


@pytest.mark.asyncio
async def test_filter_by_wallet_age_handles_missing_creation_time():
    """Wallets with unknown creation time should pass (fail-open)."""
    client = MagicMock(spec=HeliusClient)
    client._filter_by_wallet_age = HeliusClient._filter_by_wallet_age.__get__(client)

    unknown_wallet = "Unknown111111111111111111111111111111111111"
    old_wallet = "OldWall222222222222222222222222222222222222"
    now = datetime.now(timezone.utc)

    client._get_wallet_creation_timestamps_batch = AsyncMock(
        return_value={
            # unknown_wallet missing — no creation time
            old_wallet: (now - timedelta(days=30)).timestamp(),
        }
    )

    result = await client._filter_by_wallet_age(
        [unknown_wallet, old_wallet], min_age_days=7
    )

    # Unknown wallet passes (fail-open to avoid blocking new legitimate wallets)
    assert unknown_wallet in result
    assert old_wallet in result
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd scout && python -m pytest tests/test_discovery_wallet_age_filter.py -v`
Expected: FAIL with `AttributeError: '_filter_by_wallet_age' not found` or similar.

- [ ] **Step 3: Implement `_filter_by_wallet_age` method**

Add this method to `HeliusClient` class in `scout/core/helius_client.py` (add after the existing `_filter_by_sol_balance` method, approximately line 2576):

```python
async def _filter_by_wallet_age(
    self, wallets: list[str], min_age_days: int = 0
) -> list[str]:
    """
    Filter out wallets younger than min_age_days.

    Uses cached wallet creation timestamps. Wallets with unknown creation
    time pass through (fail-open) to avoid blocking legitimate new wallets
    when the creation-time API is unavailable.

    Args:
        wallets: List of wallet addresses to filter.
        min_age_days: Minimum wallet age in days. 0 disables the filter.

    Returns:
        Filtered list of wallet addresses.
    """
    if min_age_days <= 0 or not wallets:
        return wallets

    import time as _time
    from datetime import datetime, timedelta, timezone

    cutoff_timestamp = (datetime.now(timezone.utc) - timedelta(days=min_age_days)).timestamp()

    # Fetch creation timestamps (uses existing cache infrastructure)
    creation_times = await self._get_wallet_creation_timestamps_batch(wallets)

    result = []
    for wallet in wallets:
        creation_ts = creation_times.get(wallet)
        # Fail-open: if we don't know the creation time, let it through
        if creation_ts is None or creation_ts is None:
            result.append(wallet)
        elif creation_ts <= cutoff_timestamp:
            result.append(wallet)
        # else: wallet is too young, filtered out

    filtered_count = len(wallets) - len(result)
    if filtered_count > 0:
        logger.info(
            f"[Discovery] Wallet age filter: removed {filtered_count}/{len(wallets)} "
            f"wallets younger than {min_age_days} days"
        )

    return result
```

Also add the batch timestamp fetcher (add as a new method):

```python
async def _get_wallet_creation_timestamps_batch(
    self, wallets: list[str], max_concurrent: int = 10
) -> dict[str, float | None]:
    """
    Fetch wallet creation timestamps for a batch of wallets.

    Uses the existing _get_wallet_first_transaction infrastructure with
    concurrency limiting. Returns a dict mapping address → unix timestamp
    (or None if unknown).
    """
    import asyncio

    sem = asyncio.Semaphore(max_concurrent)
    results: dict[str, float | None] = {}

    async def fetch_one(wallet: str) -> tuple[str, float | None]:
        async with sem:
            try:
                ts = await self.get_wallet_first_transaction(wallet)
                return wallet, float(ts) if ts else None
            except Exception:
                return wallet, None

    tasks = [fetch_one(w) for w in wallets]
    completed = await asyncio.gather(*tasks)
    for wallet, ts in completed:
        results[wallet] = ts

    return results
```

- [ ] **Step 4: Integrate filter into discovery pipeline**

In `scout/core/helius_client.py`, inside `discover_wallets_from_recent_swaps()`, after the SOL balance filter (approximately line 2576), add:

```python
# Stage B2 — Wallet age filter (remove brand-new wallets)
min_wallet_age_days = int(os.getenv("SCOUT_MIN_WALLET_AGE_DAYS", "0"))
if min_wallet_age_days > 0 and candidate_wallets:
    candidate_wallets = await self._filter_by_wallet_age(
        candidate_wallets, min_age_days=min_wallet_age_days
    )
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd scout && python -m pytest tests/test_discovery_wallet_age_filter.py -v`
Expected: All 3 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add scout/core/helius_client.py scout/tests/test_discovery_wallet_age_filter.py
git commit -m "feat(scout): add wallet age filter to discovery pipeline

Adds SCOUT_MIN_WALLET_AGE_DAYS env var (default 0 = disabled).
When set, removes wallets younger than the threshold from the
discovery pool, improving average WQS of analyzed wallets.

Fail-open design: wallets with unknown creation time pass through."
```

### Task 2.2: Enable Wallet Age Filter in Production Config

**Files:**
- Modify: `docker-compose.yml` (scout service environment block, near line 199)

- [ ] **Step 1: Add SCOUT_MIN_WALLET_AGE_DAYS to scout environment**

Add after `SCOUT_DISCOVERY_MIN_SOL` line in `docker-compose.yml`:

```yaml
- SCOUT_MIN_WALLET_AGE_DAYS=7                               # Filter out wallets < 7 days old
```

- [ ] **Step 2: Commit**

```bash
git add docker-compose.yml
git commit -m "feat(scout): enable 7-day wallet age filter in production"
```

### Task 2.3: Fix Discovery Hours Default Mismatch

**Files:**
- Modify: `scout/core/analyzer.py:771`

**Interfaces:**
- Consumes: `SCOUT_DISCOVERY_HOURS` env var
- Produces: consistent default (168h/7d) when env var is unset

- [ ] **Step 1: Fix the default value mismatch**

In `scout/core/analyzer.py` line 771, change:

```python
# Before:
hours_back = int(os.getenv("SCOUT_DISCOVERY_HOURS", "24"))

# After (match config.py canonical default of 168):
hours_back = int(os.getenv("SCOUT_DISCOVERY_HOURS", "168"))
```

- [ ] **Step 2: Verify no test depends on the 24 default**

Run: `cd scout && python -m pytest tests/ -k "discovery" -v 2>&1 | tail -20`
Expected: No test failures related to the default change.

- [ ] **Step 3: Commit**

```bash
git add scout/core/analyzer.py
git commit -m "fix(scout): align discovery hours default to 168 (config.py canonical value)

analyzer.py manual-impl fallback used 24h while config.py used 168h.
This caused inconsistent discovery windows when SCOUT_DISCOVERY_HOURS
was unset. Now both paths agree on 168h (7 days) default."
```

### Task 2.4: Deploy Phase 2 and Verify

- [ ] **Step 1: Push to remote**

```bash
git push origin main
```

- [ ] **Step 2: Pull and rebuild scout on server**

```bash
ssh root@chimera-01.moez.tech "cd /opt/chimera && git pull origin main && docker compose build scout && docker compose up -d --force-recreate scout"
```

- [ ] **Step 3: Wait for scout cycle, verify age filter is active**

```bash
ssh root@chimera-01.moez.tech "docker logs chimera-scout 2>&1 | grep 'Wallet age filter' | tail -5"
```

Expected: Log lines showing wallets removed by age filter.

- [ ] **Step 4: Check WQS distribution improvement**

```bash
ssh root@chimera-01.moez.tech "curl -s 'http://localhost:8080/api/v1/scout/status' | python3 -c \"import sys,json; d=json.load(sys.stdin); [print(f'{b[\\\"range\\\"]}: {b[\\\"count\\\"]} ({b[\\\"percentage\\\"]:.1f}%)') for b in d['wqs_distribution']]\""
```

Expected: Percentage in 0-20 range should decrease as young/unproven wallets are filtered out.

---

## Phase 3: Risk-Adjusted Position Sizing Tiers

**Rationale:** Phase 1 lets WQS-15 wallets through, but they all route to SPEAR (max 0.5 SOL). For very low WQS (15-40), even 0.5 SOL may be too aggressive. This phase adds a position-size cap proportional to WQS, so WQS-15 wallets get tiny positions (0.05-0.15 SOL) while accumulating track record. Once they prove themselves (20+ trades, WQS rises), they graduate to larger sizes automatically.

**Design:** Add a `spear_lite_max_size_sol` config field. WQS < 40 wallets route to a SPEAR variant with a much smaller max size. No new `Strategy` enum variant needed — the position sizer applies the cap based on WQS.

### Task 3.1: Add Spear-Lite Position Size Cap Config

**Files:**
- Modify: `operator/src/engine/selection.rs` (SelectionConfig struct, ~line 90)
- Modify: `operator/src/main.rs:348` (env var loading)
- Test: `operator/src/engine/selection.rs` (inline test module)

**Interfaces:**
- Consumes: `CHIMERA_SELECTION__SPEAR_LITE_MAX_SIZE_SOL` env var (new)
- Produces: `spear_lite_max_size_sol: Decimal` field on `SelectionConfig`
- Produces: `spear_lite_wqs_threshold: f64` field (WQS below this → cap applies, default 40.0)

- [ ] **Step 1: Add config fields to SelectionConfig**

In `operator/src/engine/selection.rs`, add to the `SelectionConfig` struct (after `min_wqs_score` field, ~line 90):

```rust
/// Maximum position size for low-WQS wallets (below spear_lite_wqs_threshold).
/// These wallets are admitted but with very small positions to limit risk
/// while accumulating a track record. Default: 0.1 SOL.
pub spear_lite_max_size_sol: Decimal,

/// WQS threshold below which spear_lite_max_size_sol applies.
/// Wallets with WQS < this value get micro-positions. Default: 40.0.
pub spear_lite_wqs_threshold: f64,
```

- [ ] **Step 2: Update the default SelectionConfig**

In `operator/src/engine/selection.rs`, in the `Default` impl (around line 902), add:

```rust
spear_lite_max_size_sol: Decimal::new(10, 2),  // 0.10 SOL
spear_lite_wqs_threshold: 40.0,
```

- [ ] **Step 3: Load from env vars in main.rs**

In `operator/src/main.rs`, after the `min_wqs_score` env loading (~line 348), add:

```rust
spear_lite_max_size_sol: std::env::var("CHIMERA_SELECTION__SPEAR_LITE_MAX_SIZE_SOL")
    .ok()
    .and_then(|s| s.parse::<Decimal>().ok())
    .unwrap_or(Decimal::new(10, 2)),  // 0.10 SOL
spear_lite_wqs_threshold: std::env::var("CHIMERA_SELECTION__SPEAR_LITE_WQS_THRESHOLD")
    .ok()
    .and_then(|s| s.parse::<f64>().ok())
    .unwrap_or(40.0),
```

- [ ] **Step 4: Verify compilation**

Run: `cd operator && cargo check`
Expected: No errors.

- [ ] **Step 5: Commit**

```bash
git add operator/src/engine/selection.rs operator/src/main.rs
git commit -m "feat(operator): add spear-lite config for low-WQS position caps

Adds spear_lite_max_size_sol (default 0.10 SOL) and
spear_lite_wqs_threshold (default 40.0) to SelectionConfig.

Wallets with WQS below threshold will get micro-positions to
limit risk while accumulating trading track record."
```

### Task 3.2: Apply Spear-Lite Cap in Position Sizing

**Files:**
- Modify: `operator/src/engine/position_sizer.rs` (add WQS-aware size clamp)
- Modify: `operator/src/engine/selection.rs` (pass spear-lite config to sizer)
- Test: `operator/src/engine/position_sizer.rs` (inline test)

**Interfaces:**
- Consumes: `spear_lite_max_size_sol`, `spear_lite_wqs_threshold` from SelectionConfig
- Produces: position sizes capped for low-WQS wallets

- [ ] **Step 1: Add WQS-based size cap to PositionSizingFactors**

In `operator/src/engine/position_sizer.rs`, add a field to the `PositionSizingFactors` struct (or equivalent input struct):

```rust
/// Optional WQS-based max size cap. When wallet WQS is below a threshold,
/// the position size is capped to this value. None = no cap.
pub wqs_capped_max_size: Option<Decimal>,
```

- [ ] **Step 2: Apply the cap in calculate_size()**

In `operator/src/engine/position_sizer.rs`, in `calculate_size()`, after the strategy max clamp (~line 340), add:

```rust
// WQS-based micro-position cap for low-conviction wallets
if let Some(wqs_cap) = factors.wqs_capped_max_size {
    if size > wqs_cap {
        tracing::debug!(
            wallet_wqs = %factors.wallet_wqs,
            original_size = %size,
            capped_size = %wqs_cap,
            "Applying WQS-based micro-position cap"
        );
        size = wqs_cap;
    }
}
```

- [ ] **Step 3: Wire up the cap from selection.rs**

In `operator/src/engine/selection.rs`, in `decide_buy()`, when building the position sizing factors (~line 743), set the cap:

```rust
// Apply spear-lite cap for low-WQS wallets
let wqs_capped_max_size = if wallet_wqs < self.config.spear_lite_wqs_threshold {
    Some(self.config.spear_lite_max_size_sol)
} else {
    None
};
```

Pass `wqs_capped_max_size` into the `PositionSizingFactors` when calling `calculate_size()`.

- [ ] **Step 4: Write test for WQS-based size cap**

In `operator/src/engine/position_sizer.rs`, add test:

```rust
#[cfg(test)]
mod wqs_cap_tests {
    use super::*;
    use rust_decimal::prelude::*;

    #[test]
    fn test_low_wqs_wallet_gets_micro_position() {
        // WQS 20.0 wallet with spear_lite cap of 0.10 SOL
        // Base size might be 0.3 SOL (0.5 * 20/100 * confidence)
        // Should be capped to 0.10 SOL
        let config = PositionSizingConfig {
            base_size_sol: Decimal::new(5, 1),  // 0.5
            spear_max_size_sol: Decimal::new(5, 1),  // 0.5
            ..Default::default()
        };
        let factors = PositionSizingFactors {
            wallet_wqs: 20.0,
            wqs_capped_max_size: Some(Decimal::new(10, 2)),  // 0.10
            ..Default::default()
        };
        let sizer = PositionSizer::new(config);
        let result = sizer.calculate_size(&factors).unwrap();
        assert!(result.size_sol <= Decimal::new(10, 2),
            "WQS-20 wallet should be capped at 0.10 SOL, got {}", result.size_sol);
    }

    #[test]
    fn test_high_wqs_wallet_not_capped() {
        // WQS 85.0 wallet — no spear-lite cap applies
        let config = PositionSizingConfig::default();
        let factors = PositionSizingFactors {
            wallet_wqs: 85.0,
            wqs_capped_max_size: None,
            ..Default::default()
        };
        let sizer = PositionSizer::new(config);
        let result = sizer.calculate_size(&factors).unwrap();
        // Should use normal sizing, not capped
        assert!(result.size_sol > Decimal::ZERO);
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cd operator && cargo test wqs_cap -- --test-threads=1`
Expected: Both tests PASS.

- [ ] **Step 6: Commit**

```bash
git add operator/src/engine/position_sizer.rs operator/src/engine/selection.rs
git commit -m "feat(operator): apply WQS-based micro-position cap for low-conviction wallets

Wallets with WQS < spear_lite_wqs_threshold (40.0) get positions
capped at spear_lite_max_size_sol (0.10 SOL). This limits risk on
unproven wallets while letting them accumulate a track record.

High-WQS wallets (≥40) are unaffected — normal SPEAR/SHIELD sizing applies."
```

### Task 3.3: Add Spear-Lite Config to docker-compose.yml

**Files:**
- Modify: `docker-compose.yml` (operator service environment block)

- [ ] **Step 1: Add env vars**

In `docker-compose.yml`, in the operator service environment block, add:

```yaml
- CHIMERA_SELECTION__SPEAR_LITE_MAX_SIZE_SOL=0.1             # Micro-positions for WQS < 40
- CHIMERA_SELECTION__SPEAR_LITE_WQS_THRESHOLD=40.0
```

- [ ] **Step 2: Commit**

```bash
git add docker-compose.yml
git commit -m "feat(operator): enable spear-lite micro-positions in production

WQS < 40 wallets capped at 0.1 SOL per trade.
WQS ≥ 40 wallets use normal SPEAR (0.5 SOL) / SHIELD (2.0 SOL) sizing."
```

### Task 3.4: Deploy Phase 3 and Verify

- [ ] **Step 1: Push to remote**

```bash
git push origin main
```

- [ ] **Step 2: Pull, rebuild operator, deploy**

```bash
ssh root@chimera-01.moez.tech "cd /opt/chimera && git pull origin main && docker compose build operator && docker compose up -d --force-recreate operator"
```

- [ ] **Step 3: Verify operator is applying the cap**

```bash
ssh root@chimera-01.moez.tech "docker exec chimera-operator tail -500 /app/data/logs/operator.log | grep 'WQS-based micro-position cap' | tail -5"
```

Expected: Log lines showing the cap being applied for low-WQS wallets.

- [ ] **Step 4: Check position sizes in API**

```bash
ssh root@chimera-01.moez.tech "curl -s 'http://localhost:8080/api/v1/positions?status=ACTIVE' | python3 -c \"import sys,json; d=json.load(sys.stdin); [print(f'{p[\\\"token_symbol\\\"]}: entry={p[\\\"entry_amount_sol\\\"]} SOL') for p in d.get('positions',[])]\""
```

Expected: New positions for low-WQS wallets show small entry amounts (≤ 0.1 SOL).

---

## Phase 4: Monitoring & Validation

### Task 4.1: Monitor Trading Activity Post-Deployment

- [ ] **Step 1: Check trade count after 1 hour**

```bash
ssh root@chimera-01.moez.tech "curl -s 'http://localhost:8080/api/v1/health' | python3 -c \"import sys,json; d=json.load(sys.stdin); print(f'Last trade: {d.get(\\\"last_trade_at\\\", \\\"never\\\")}')\""
```

Expected: `last_trade_at` within the last hour.

- [ ] **Step 2: Check PnL of new positions after 24 hours**

```bash
ssh root@chimera-01.moez.tech "curl -s 'http://localhost:8080/api/v1/metrics/performance' | python3 -m json.tool"
```

Expected: `pnl_24h` is populated with a non-null value.

- [ ] **Step 3: Check for excessive losses**

```bash
ssh root@chimera-01.moez.tech "curl -s 'http://localhost:8080/api/v1/positions?status=CLOSED' | python3 -c \"import sys,json; d=json.load(sys.stdin); trades=d.get('positions',[]); losses=[t for t in trades if float(t.get('realized_pnl_sol',0) or 0) < 0]; print(f'Closed: {len(trades)}, Losses: {len(losses)}')\""
```

Expected: Win rate should be > 40% for SPEAR strategy. If < 30%, consider raising thresholds back.

- [ ] **Step 4: If losing money, revert Phase 1 thresholds**

If win rate < 30% after 24 hours, revert:

```bash
# Restore original thresholds
git revert HEAD~N  # N = number of Phase 1 commits
git push origin main
ssh root@chimera-01.moez.tech "cd /opt/chimera && git pull && docker compose up -d --force-recreate scout operator"
```

---

## Self-Review Notes

### Spec Coverage
- ✅ Option 1 (Lower Thresholds): Tasks 1.1, 1.2, 1.3
- ✅ Option 2 (Discovery Quality): Tasks 2.1, 2.2, 2.3, 2.4
- ✅ Option 3 (Trading Strategy): Tasks 3.1, 3.2, 3.3, 3.4
- ✅ Monitoring: Task 4.1

### Key Risk: Operator/Scout Threshold Alignment
Both `SCOUT_MIN_WQS_ACTIVE` (scout) and `CHIMERA_SELECTION__MIN_WQS_SCORE` (operator) MUST be set to the same value. If they diverge, scout promotes wallets that operator rejects, or operator tries to trade wallets scout hasn't promoted.

### Key Risk: Wallet Age Filter API Cost
The `_get_wallet_creation_timestamps_batch` method calls `get_wallet_first_transaction` for each wallet, which hits the Helius API. With 300 wallets per run, this adds 300 API calls. Mitigated by Redis caching (token creation cached for 7 days) and the fail-open design.

### Dependency Order
Phase 1 → Phase 2 → Phase 3 → Phase 4. Phase 1 is the critical path — it unblocks trading immediately. Phases 2-3 are quality improvements that can be deployed after observing Phase 1 results.
