# Volatility-Scaled Profit Targets

**Status:** Approved
**Date:** 2026-07-20
**Author:** Chimera Agent
**Related:** Profitability investigation (ses_08428ccdbffeTVWzO3LQVTwsS7)

## Problem Statement

Current profit targets `[25, 50, 100, 200]%` have a **0% hit rate**. Historical
BONK trades moved only ±0.5%, never reaching the +25% first target. Every
position exited via time-based exit at a loss because round-trip costs (~4%)
exceeded the tiny price movements.

The system already has adaptive logic (market-regime multipliers, WQS-based
stop-loss, strategy-specific time exits), but the **base profit targets are
calibrated for memecoin pumps, not for copy-trading moderate-volatility tokens**.

A single static config cannot serve both token types:
- New memecoins (60%+ volatility) need wide targets to let moonshots run.
- Established tokens (BONK/JUP, ~5% volatility) need targets they can actually reach.

## Goal

Make profit targets **self-adapt per token** using the existing
`price_cache.calculate_volatility()` infrastructure, so the system serves both
high-vol and low-vol tokens without manual tuning.

## Non-Goals

- Per-strategy target profiles (Shield vs Spear). Strategy ≠ volatility; a
  Shield signal on a volatile token still needs wide targets.
- Changing stop-loss logic. Already adapts via WQS + volatility.
- Changing time-exit logic. Already strategy-specific.
- Changing tiered exit percent. The fraction sold per tier doesn't depend on volatility.

## Design — Approach B: Volatility-Scaled Targets

### Core Formula

```
scale_factor = min(1.0, token_volatility% / vol_scale_threshold)
effective_target[i] = max(base_target[i] × scale_factor, min_target_pct)
```

| Token Type        | Volatility | Scale | Effective Targets              |
|-------------------|-----------:|------:|--------------------------------|
| New memecoin      |        60% |   1.0 | `[25, 50, 100, 200]` (full)    |
| WIF / POPCAT      |        20% |  0.67 | `[16.7, 33.3, 66.7, 133.3]`    |
| BONK / JUP        |         5% |  0.17 | `[5, 8.3, 16.7, 33.3]` (floored at min) |

### What Gets Scaled

| Parameter                          | Scaled? | Rationale                                   |
|------------------------------------|:-------:|---------------------------------------------|
| Profit targets `[25,50,100,200]`   | **Yes** | Core fix — unreachable for low-vol tokens   |
| Trailing stop activation (30%)     | **Yes** | Must activate earlier on low-vol tokens     |
| Trailing stop distance (20%)       | **Yes** | Unscaled distance makes stop useless (Issue 3) |
| Stop-loss (-10% to -25%)           |    No   | Already adapts via WQS + volatility          |
| Time exits (2h/4h)                 |    No   | Already strategy-specific                    |
| Tiered exit percent (25%)          |    No   | Fraction sold per tier is volatility-independent |

### New Config Fields

```yaml
profit_management:
  targets: [25, 50, 100, 200]       # Base targets at full volatility
  target_vol_scale_threshold: 30.0   # Tokens at 30%+ vol get full targets
  min_target_pct: 5.0                # Floor — never target below 5% (above ~4% break-even)
  trailing_stop_activation: 30       # Base activation (scales with vol)
  trailing_stop_distance: 20         # Base distance (scales with vol)
```

Defaults: `target_vol_scale_threshold: 30.0`, `min_target_pct: 5.0`.

### Why min_target_pct = 5.0 (Not 3.0)

Round-trip break-even is ~4% (Jito tip ceiling + DEX fee + slippage on 0.25 SOL
trades). A first target at 3% sells at a net loss after costs. 5.0 provides a
clear margin above break-even so the first tier actually nets positive.

## Review Fixes (5 Issues)

### Issue 1: CRITICAL — `targets_hit` double-sell bug

**Root cause:** `profit_targets.rs:286` tracks hit targets by **value**:
```rust
if profit_percent >= *target && !state.targets_hit.contains(target) {
    state.targets_hit.push(*target);
```
When targets scale dynamically, values change every tick as volatility
fluctuates. A target "hit" at 4.2% becomes 3.8% next tick, and the new value
isn't in `targets_hit` → it triggers a **second sell** on the same tier.

**Fix:** Track hit targets by **index** (0, 1, 2, 3), not by value.

- Change `targets_hit: Vec<Decimal>` → `targets_hit: Vec<usize>`.
- Iterate `targets.iter().enumerate()` and check `!state.targets_hit.contains(&i)`.
- DB persistence: `targets_hit` JSON stores `[0, 1, 3]` (indices) instead of `["4.2", "8.3"]`.
- `ExitTargetData.targets_hit` field stays `String` (JSON), but content semantics change to indices.

### Issue 2: `min_target_pct` below break-even

**Root cause:** Initial design used 3.0%, which is below the ~4% round-trip cost.

**Fix:** Default `min_target_pct` to **5.0**.

### Issue 3: Trailing stop provides no profit protection for low-vol tokens

**Root cause:** With scaled activation at +5% but **unscaled** distance of 20%,
the trailing stop sits at -15% from entry — below the stop-loss, so it never
protects profit:
```
Activation: +5.1% (scaled)
Distance:   -20% (NOT scaled)
Stop price: 5.1% - 20% = -14.9% from entry  ← below stop-loss!
```

**Fix:** Scale the trailing stop distance by the same `vol_scale`. Additionally,
clamp the trailing stop price to never drop below
`entry_price × (1 + min_target_pct/100)` once activated — guaranteeing a
minimum profit lock once the trailing stop engages.

```
effective_distance = base_distance × vol_scale
raw_stop_price = peak_price × (1 - effective_distance/100)
floor_price = entry_price × (1 + min_target_pct/100)
trailing_stop_price = max(raw_stop_price, floor_price)
```

### Issue 4: Log flooding

**Root cause:** `check_targets` runs every 5 seconds. Logging scaled targets at
INFO would produce ~17,280 log lines/day per position.

**Fix:** Log scaled targets at **DEBUG** level. Additionally, cache the last
logged `vol_scale` and only log at DEBUG when it changes by more than 10%,
suppressing per-tick noise.

### Issue 5: Cold-start target snap

**Root cause:** New positions have no volatility history →
`calculate_volatility()` returns `None` → scale defaults to 1.0 (full targets).
After ~2 minutes of price data accumulates, volatility is calculated and targets
suddenly snap down. A position waiting for +25% suddenly has its first target at +5%.

**Fix:** Two-part approach:
1. **Initial estimate at registration:** In `register_position()`, query
   `price_cache.calculate_volatility()` for the token. If available, store it as
   `initial_vol_scale` on the `ProfitTargetState`. This covers the case where the
   token was already being tracked (e.g., monitored wallet's prior activity
   warmed the cache).
2. **Ramp over first N ticks:** If no volatility data exists at registration,
   ramp the scale gradually. Use a ramp count (e.g., 60 ticks × 5s = 5 min):
   ```
   ramp_progress = min(1.0, ticks_since_entry / RAMP_TICKS)
   effective_scale = 1.0 - (1.0 - vol_scale) × ramp_progress
   ```
   This smooths the transition from full targets to scaled targets, avoiding a
   hard snap that could trigger an immediate sell.

## Code Changes

### 1. `operator/src/config.rs`

Add two fields to `ProfitManagementConfig` (~line 1292):
- `target_vol_scale_threshold: Decimal` (default `30.0`)
- `min_target_pct: Decimal` (default `5.0`)

Ensure `trailing_stop_activation` and `trailing_stop_distance` are already
present (they are).

### 2. `operator/src/engine/profit_targets.rs`

**`ProfitTargetState` struct:**
- Change `targets_hit: Vec<Decimal>` → `targets_hit: Vec<usize>`.
- Add `initial_vol_scale: Option<Decimal>` (for cold-start fix).
- Add `ticks_since_entry: u32` (for ramp fix).
- Add `last_logged_vol_scale: Option<Decimal>` (for log flooding fix).

**`register_position()` (~line 137):**
- Query `price_cache.calculate_volatility()` for initial vol estimate.
- Initialize `initial_vol_scale`, `ticks_since_entry: 0`.

**`check_targets()` (~line 260):**
- Compute `vol_scale = min(1.0, token_volatility / threshold)`.
- If no volatility data: use `initial_vol_scale` if set, else ramp from 1.0.
- Scale `profit_level_targets`, `trailing_stop_activation`, `trailing_stop_distance`.
- Clamp each target to `max(scaled, min_target_pct)`.
- Iterate targets by index (Issue 1 fix).
- Clamp trailing stop floor (Issue 3 fix).
- Log at DEBUG with change-threshold gate (Issue 4 fix).

**DB persistence:**
- `upsert_exit_target` call: serialize `targets_hit` as JSON array of indices.
- `load_exit_target` / restore path: deserialize indices.
- No schema migration needed — `targets_hit` column is already a JSON string.

### 3. `operator/src/db_abstraction/types.rs`

No change needed — `ExitTargetData.targets_hit` stays `String` (JSON).

## Edge Cases

| Case                                   | Behavior                                              |
|----------------------------------------|-------------------------------------------------------|
| No volatility data at registration     | `initial_vol_scale = None`, ramp from 1.0 over 5 min  |
| Volatility data available at registration | `initial_vol_scale = computed`, no ramp needed      |
| Scale would push target below `min_target_pct` | Clamp to `min_target_pct`                     |
| Scale > 1.0 (token more volatile than threshold) | Clamp to 1.0 (never widen beyond base)       |
| `min_target_pct` > `base_target[0]`    | Config error — validate at startup, log warning       |
| Position restored from DB after restart | Restore `targets_hit` indices, recompute scale fresh  |

## Testing

- **Unit test:** `check_targets` with mocked volatility → verify scaled targets and index-based tracking.
- **Unit test:** Double-sell scenario — simulate volatility fluctuation between ticks, assert no tier sells twice.
- **Unit test:** Cold-start ramp — assert scale transitions smoothly, no snap.
- **Unit test:** Trailing stop floor — assert stop price never below `entry × (1 + min_target_pct/100)` once activated.
- **Integration:** Deploy to paper, submit a low-vol token signal, verify first target fires at a reachable percentage.

## Rollout

1. Implement + unit test locally.
2. `make lint-operator && make build-operator`.
3. Commit: `feat(profit): volatility-scaled profit targets with index-based tracking`.
4. Push, deploy to paper mode.
5. Submit test signal on a known low-vol token (BONK/JUP).
6. Verify first target fires within reachable range, no double-sell, trailing stop protects profit.
7. Monitor for 24h, compare hit rate vs baseline (0%).
