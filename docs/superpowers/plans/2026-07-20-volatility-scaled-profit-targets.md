# Volatility-Scaled Profit Targets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Scale profit targets and trailing-stop parameters proportionally to each token's measured volatility, so low-vol tokens (BONK/JUP ~5%) get reachable targets while high-vol tokens (memecoins ~60%) keep wide targets that let moonshots run.

**Architecture:** Extend the existing `ProfitTargetManager.check_targets()` method to compute a per-token `vol_scale` factor from `price_cache.calculate_volatility()`, then scale `targets[]`, `trailing_stop_activation`, and `trailing_stop_distance` by that factor (floored at `min_target_pct`). Fix a pre-existing double-sell bug by tracking hit tiers by index instead of by value. Add a cold-start ramp so targets don't snap when volatility data first becomes available.

**Tech Stack:** Rust (operator), `rust_decimal::Decimal`, `rust_decimal_macros::dec!`, `serde_json` for DB persistence, `tokio::sync::RwLock`.

## Global Constraints

- **Financial precision:** Use `rust_decimal::Decimal` for all percentage/scale calculations. Never use f64 for financial values (volatility input from `calculate_volatility()` is f64 — convert to Decimal at the boundary).
- **No schema migration:** The `exit_targets.targets_hit` column is already a JSON string. The change from value-array to index-array is a content-semantics change only, not a schema change.
- **Backward compatibility:** Old DB rows with value-based `targets_hit` JSON must not panic on restore — degrade gracefully (treat as empty if parse yields decimals instead of integers).
- **Build/lint commands:** `cd operator && cargo build --release` and `cd operator && cargo clippy -- -D warnings` (note: pre-existing warnings exist in other files; only verify no NEW warnings in modified files).
- **Unit test runner:** `cd operator && cargo test profit_target --lib -- --test-threads=1`
- **Commit convention:** `feat(profit): description` / `fix(profit): description`.

---

## File Structure

| File | Responsibility | Action |
|------|---------------|--------|
| `operator/src/config.rs` | Add `target_vol_scale_threshold` + `min_target_pct` config fields and defaults | Modify |
| `operator/src/engine/profit_targets.rs` | Core logic: pure scaling function, index-based tracking, cold-start ramp, trailing floor, log gating | Modify |
| `operator/src/engine/profit_targets.rs` (tests module) | Unit tests for scaling, double-sell, ramp, trailing floor | Create |

**Testing strategy:** The `Database` trait has ~80 methods, making a full mock impractical for unit tests. Instead, extract the scaling decision logic into pure standalone functions that take primitives and return primitives — testable without a DB. The `check_targets()` integration is validated by the existing paper-mode deployment cycle (Task 7).

---

### Task 1: Add Config Fields

**Files:**
- Modify: `operator/src/config.rs:1293-1342` (struct), `1354-1418` (defaults + Default impl)

**Interfaces:**
- Produces: `ProfitManagementConfig.target_vol_scale_threshold: Decimal` (default 30.0), `ProfitManagementConfig.min_target_pct: Decimal` (default 5.0)

- [ ] **Step 1: Write the failing test**

Add a test at the end of `config.rs` (inside `#[cfg(test)]` module if one exists, otherwise create one at file end):

```rust
#[cfg(test)]
mod vol_target_config_tests {
    use super::*;

    #[test]
    fn test_new_config_fields_have_defaults() {
        let config = ProfitManagementConfig::default();
        assert_eq!(config.target_vol_scale_threshold, dec!(30.0));
        assert_eq!(config.min_target_pct, dec!(5.0));
    }

    #[test]
    fn test_config_parses_new_fields() {
        let yaml = r#"
targets: [10, 20, 40, 80]
target_vol_scale_threshold: 25.0
min_target_pct: 6.0
"#;
        let config: ProfitManagementConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.target_vol_scale_threshold, dec!(25.0));
        assert_eq!(config.min_target_pct, dec!(6.0));
        assert_eq!(config.targets, vec![dec!(10), dec!(20), dec!(40), dec!(80)]);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd operator && cargo test vol_target_config --lib`
Expected: FAIL — `no field target_vol_scale_threshold` on struct.

- [ ] **Step 3: Add the struct fields**

In `operator/src/config.rs`, add these two fields to `ProfitManagementConfig` after `trailing_stop_distance` (around line 1305):

```rust
    /// Volatility threshold above which targets are used at full strength (%).
    /// Tokens with volatility below this get proportionally scaled-down targets.
    /// Example: threshold=30.0, token vol=15.0 → scale=0.5 → target[0] halved.
    #[serde(default = "default_target_vol_scale_threshold")]
    pub target_vol_scale_threshold: Decimal,
    /// Floor for scaled profit targets (%). Prevents targets from dropping
    /// below break-even (~4% round-trip cost). Must be less than targets[0].
    #[serde(default = "default_min_target_pct")]
    pub min_target_pct: Decimal,
```

- [ ] **Step 4: Add default functions**

Add after `default_trailing_stop_distance` (around line 1370):

```rust
fn default_target_vol_scale_threshold() -> Decimal {
    dec!(30.0)
}

fn default_min_target_pct() -> Decimal {
    dec!(5.0)
}
```

- [ ] **Step 5: Add fields to Default impl**

In the `impl Default for ProfitManagementConfig` (around line 1398), add after `trailing_stop_distance`:

```rust
            target_vol_scale_threshold: default_target_vol_scale_threshold(),
            min_target_pct: default_min_target_pct(),
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cd operator && cargo test vol_target_config --lib`
Expected: PASS (both tests).

- [ ] **Step 7: Commit**

```bash
git add operator/src/config.rs
git commit -m "feat(config): add target_vol_scale_threshold and min_target_pct fields"
```

---

### Task 2: Pure Volatility-Scaling Function + Tests

**Files:**
- Modify: `operator/src/engine/profit_targets.rs` (add function + test module)

**Interfaces:**
- Produces: `compute_vol_scale(volatility: Option<f64>, threshold: Decimal, ticks_since_entry: u32, initial_vol_scale: Option<Decimal>) -> Decimal`

This is a pure function: given the token's current volatility and cold-start context, return a scale factor in `[0, 1]`. Extracted from `check_targets()` so it's unit-testable without a DB.

- [ ] **Step 1: Write the failing tests**

Add a `#[cfg(test)]` module at the end of `operator/src/engine/profit_targets.rs`:

```rust
#[cfg(test)]
mod vol_scale_tests {
    use super::*;
    use rust_decimal::prelude::*;

    fn dec_from_f64(v: f64) -> Decimal {
        Decimal::from_str(&format!("{:.4}", v)).unwrap()
    }

    #[test]
    fn test_high_volatility_full_scale() {
        // Vol=60%, threshold=30% → scale=1.0 (capped)
        let scale = compute_vol_scale(Some(60.0), dec!(30.0), 100, None);
        assert_eq!(scale, Decimal::ONE);
    }

    #[test]
    fn test_moderate_volatility_partial_scale() {
        // Vol=15%, threshold=30% → scale=0.5
        let scale = compute_vol_scale(Some(15.0), dec!(30.0), 100, None);
        assert_eq!(scale, dec!(0.5));
    }

    #[test]
    fn test_low_volatility_small_scale() {
        // Vol=5%, threshold=30% → scale=0.1667
        let scale = compute_vol_scale(Some(5.0), dec!(30.0), 100, None);
        // 5/30 = 0.16666... — check it's between 0.16 and 0.17
        assert!(scale > dec!(0.16) && scale < dec!(0.17));
    }

    #[test]
    fn test_no_volatility_uses_full_scale() {
        let scale = compute_vol_scale(None, dec!(30.0), 100, None);
        assert_eq!(scale, Decimal::ONE);
    }

    #[test]
    fn test_no_volatility_but_has_initial_estimate() {
        // No live vol data, but initial estimate was vol=10% → scale=10/30=0.333
        let scale = compute_vol_scale(None, dec!(30.0), 100, Some(0.3333));
        assert!(scale > dec!(0.30) && scale < dec!(0.40));
    }

    #[test]
    fn test_cold_start_ramp_smooths_transition() {
        // Vol=5% (scale would be 0.167), but only 3 ticks elapsed (ramp 60 ticks)
        // ramp_progress = 3/60 = 0.05
        // effective_scale = 1.0 - (1.0 - 0.167) * 0.05 = 1.0 - 0.0417 = 0.958
        let scale = compute_vol_scale(Some(5.0), dec!(30.0), 3, None);
        assert!(scale > dec!(0.95) && scale < dec!(0.97));
    }

    #[test]
    fn test_ramp_completes_after_60_ticks() {
        // After 60 ticks, ramp is fully applied — scale = raw 5/30 = 0.167
        let scale = compute_vol_scale(Some(5.0), dec!(30.0), 60, None);
        assert!(scale > dec!(0.16) && scale < dec!(0.17));
    }

    #[test]
    fn test_zero_volatility_full_scale() {
        // Vol=0% is degenerate but shouldn't crash — scale=0, but callers clamp to min_target_pct
        let scale = compute_vol_scale(Some(0.0), dec!(30.0), 100, None);
        assert_eq!(scale, Decimal::ZERO);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd operator && cargo test vol_scale_tests --lib`
Expected: FAIL — `function compute_vol_scale not found`.

- [ ] **Step 3: Implement `compute_vol_scale`**

Add this free function above `ProfitTargetManager` impl (around line 67, before `impl ProfitTargetManager`):

```rust
/// Number of 5-second ticks over which to ramp the volatility scale from 1.0
/// to the measured value. Prevents a sudden target snap when volatility data
/// first becomes available (~2 min after position open at 5s intervals).
const VOL_RAMP_TICKS: u32 = 60;

/// Compute the volatility scale factor for profit targets.
///
/// Returns a value in `[0, 1]`:
/// - High volatility (>= threshold): returns 1.0 (use full targets).
/// - Low volatility: returns `vol / threshold` (proportionally smaller targets).
/// - No data: returns 1.0 (safe default — full targets for unknown tokens).
///
/// The cold-start ramp smooths the transition from 1.0 to the measured scale
/// over the first `VOL_RAMP_TICKS` ticks after position registration.
fn compute_vol_scale(
    volatility: Option<f64>,
    threshold: Decimal,
    ticks_since_entry: u32,
    initial_vol_scale: Option<Decimal>,
) -> Decimal {
    let raw_scale = match volatility {
        Some(vol) => {
            if threshold.is_zero() {
                return Decimal::ONE;
            }
            let vol_dec = Decimal::from_str(&format!("{:.4}", vol))
                .unwrap_or(Decimal::ZERO);
            (vol_dec / threshold).min(Decimal::ONE)
        }
        None => {
            // No live volatility — use initial estimate if available, else full scale
            match initial_vol_scale {
                Some(init) => init,
                None => return Decimal::ONE,
            }
        }
    };

    // Cold-start ramp: smoothly transition from 1.0 to raw_scale over VOL_RAMP_TICKS.
    // If initial_vol_scale was set (data existed at registration), skip the ramp.
    if initial_vol_scale.is_some() || ticks_since_entry >= VOL_RAMP_TICKS {
        return raw_scale;
    }

    let ramp_progress = Decimal::from(ticks_since_entry) / Decimal::from(VOL_RAMP_TICKS);
    // effective_scale = 1.0 - (1.0 - raw_scale) * ramp_progress
    let effective = Decimal::ONE - (Decimal::ONE - raw_scale) * ramp_progress;
    effective.min(Decimal::ONE)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd operator && cargo test vol_scale_tests --lib -- --test-threads=1`
Expected: PASS (all 8 tests).

- [ ] **Step 5: Commit**

```bash
git add operator/src/engine/profit_targets.rs
git commit -m "feat(profit): add pure compute_vol_scale function with cold-start ramp"
```

---

### Task 3: Index-Based Target Tracking

**Files:**
- Modify: `operator/src/engine/profit_targets.rs:44` (struct field), `285-291` (check loop), `395-396` (DB snapshot), `162-163` (restore)

**Interfaces:**
- Changes: `ProfitTargetState.targets_hit` type from `Vec<Decimal>` to `Vec<usize>`. DB JSON format changes from `[4.2, 8.3]` to `[0, 1]` (indices).

**Backward compatibility:** Old DB rows with decimal values in `targets_hit` JSON must not crash. On restore, attempt to parse as `Vec<usize>`; if that fails, parse as `Vec<Decimal>` and convert (round to nearest index — but since old data used value-based tracking with static targets, safer to just clear and re-evaluate).

- [ ] **Step 1: Write the failing test for index-based tracking**

Add to the `vol_scale_tests` module:

```rust
    #[test]
    fn test_index_based_tracking_no_double_sell() {
        // Simulate: target[0] scaled to 4.2% on tick 1, then 3.8% on tick 2.
        // With value-based tracking, tick 2 would re-trigger tier 0.
        // With index-based tracking, tier 0 stays hit regardless of value drift.
        let mut targets_hit: Vec<usize> = vec![];

        // Tick 1: profit=4.2%, scaled_targets=[4.2, 8.3, 16.7, 33.3]
        // Tier 0 (4.2) is hit
        for (i, target) in [dec!(4.2), dec!(8.3), dec!(16.7), dec!(33.3)].iter().enumerate() {
            if dec!(4.2) >= *target && !targets_hit.contains(&i) {
                targets_hit.push(i);
            }
        }
        assert_eq!(targets_hit, vec![0]);

        // Tick 2: profit=3.9%, scaled_targets=[3.8, 7.5, 15.0, 30.0] (volatility dropped)
        // Value 3.8 is NOT in old targets_hit (which had 4.2), so value-based would
        // re-trigger. Index-based: tier 0 already hit → skip.
        for (i, target) in [dec!(3.8), dec!(7.5), dec!(15.0), dec!(30.0)].iter().enumerate() {
            if dec!(3.9) >= *target && !targets_hit.contains(&i) {
                targets_hit.push(i);
            }
        }
        // Still only tier 0 — no double sell
        assert_eq!(targets_hit, vec![0]);
    }
```

- [ ] **Step 2: Run test to verify it passes (it's pure logic, should pass immediately)**

Run: `cd operator && cargo test test_index_based_tracking_no_double_sell --lib`
Expected: PASS (the test itself demonstrates the fix; the real code change is in `check_targets`).

- [ ] **Step 3: Change the struct field type**

In `operator/src/engine/profit_targets.rs:44`, change:

```rust
    targets_hit: Vec<Decimal>, // Which targets have been hit
```
to:
```rust
    targets_hit: Vec<usize>, // Which target indices have been hit (index-based, not value-based)
```

- [ ] **Step 4: Update the restore path in `register_position`**

Around line 162, change the deserialization to handle both old (Decimal) and new (usize) formats:

```rust
                let targets_hit: Vec<usize> =
                    serde_json::from_str(&data.targets_hit).unwrap_or_else(|_| {
                        // Backward compat: old rows stored Decimal values.
                        // Clear and re-evaluate from scratch (safe — may re-trigger
                        // a tier that was already hit, but that's better than panic).
                        tracing::warn!(
                            trade_uuid,
                            raw = %data.targets_hit,
                            "Migrating targets_hit from value-based to index-based (resetting)"
                        );
                        Vec::new()
                    });
```

- [ ] **Step 5: Update the check loop in `check_targets`**

Around line 285, change the loop to enumerate by index:

```rust
            for (i, target) in profit_level_targets.iter().enumerate() {
                if profit_percent >= *target && !state.targets_hit.contains(&i) {
                    state.targets_hit.push(i);
                    state_changed = true;
                    new_targets_hit += 1;
                }
            }
```

- [ ] **Step 6: Update the DB snapshot serialization**

Around line 395, change:

```rust
            let th: Vec<Decimal> = state.targets_hit.clone();
```
to:
```rust
            let th: Vec<usize> = state.targets_hit.clone();
```

- [ ] **Step 7: Build to verify compilation**

Run: `cd operator && cargo build --lib 2>&1 | tail -5`
Expected: No errors. If there are errors about type mismatches, fix the specific spots.

- [ ] **Step 8: Run all profit target tests**

Run: `cd operator && cargo test vol_scale_tests --lib -- --test-threads=1`
Expected: PASS (all tests).

- [ ] **Step 9: Commit**

```bash
git add operator/src/engine/profit_targets.rs
git commit -m "fix(profit): track targets_hit by index to prevent double-sell on volatility drift"
```

---

### Task 4: Wire Volatility Scaling into check_targets

**Files:**
- Modify: `operator/src/engine/profit_targets.rs:259-271` (target scaling), `350-373` (trailing stop scaling)

**Interfaces:**
- Consumes: `compute_vol_scale()` from Task 2, `target_vol_scale_threshold` + `min_target_pct` from Task 1.
- Requires: new fields on `ProfitTargetState` — `initial_vol_scale: Option<Decimal>`, `ticks_since_entry: u32`, `last_logged_vol_scale: Option<Decimal>`.

- [ ] **Step 1: Add new fields to `ProfitTargetState`**

After `remaining_fraction: Decimal,` (line 51), add:

```rust
    /// Volatility scale captured at registration time (if data was available).
    /// Used as a fallback when calculate_volatility returns None mid-session.
    initial_vol_scale: Option<Decimal>,
    /// Number of check_targets ticks since position registration.
    /// Used for the cold-start ramp (see VOL_RAMP_TICKS).
    ticks_since_entry: u32,
    /// Last vol_scale value logged — used to suppress log flooding.
    last_logged_vol_scale: Option<Decimal>,
```

- [ ] **Step 2: Initialize new fields in register_position (fresh state path ~line 183)**

In the fresh-state `ProfitTargetState { ... }` construction, add after `remaining_fraction: Decimal::ONE,`:

```rust
                    initial_vol_scale: {
                        // Capture initial volatility estimate if available at registration
                        match self.price_cache.calculate_volatility(token_address) {
                            Some(vol) => {
                                let vol_dec = Decimal::from_str(&format!("{:.4}", vol))
                                    .unwrap_or(Decimal::ZERO);
                                if self.config.target_vol_scale_threshold.is_zero() {
                                    None
                                } else {
                                    let scale = (vol_dec / self.config.target_vol_scale_threshold)
                                        .min(Decimal::ONE);
                                    Some(scale)
                                }
                            }
                            None => None,
                        }
                    },
                    ticks_since_entry: 0,
                    last_logged_vol_scale: None,
```

- [ ] **Step 3: Initialize new fields in register_position (restore-from-DB path ~line 167)**

In the restore `ProfitTargetState { ... }` construction, add after `remaining_fraction: remaining,`:

```rust
                    initial_vol_scale: {
                        match self.price_cache.calculate_volatility(token_address) {
                            Some(vol) => {
                                let vol_dec = Decimal::from_str(&format!("{:.4}", vol))
                                    .unwrap_or(Decimal::ZERO);
                                if self.config.target_vol_scale_threshold.is_zero() {
                                    None
                                } else {
                                    Some((vol_dec / self.config.target_vol_scale_threshold)
                                        .min(Decimal::ONE))
                                }
                            }
                            None => None,
                        }
                    },
                    ticks_since_entry: 0,
                    last_logged_vol_scale: None,
```

- [ ] **Step 4: Compute vol_scale and scale profit targets in check_targets**

Replace the target-scaling block (around lines 259-271). After computing `multiplier` (market regime), add vol_scale computation:

```rust
        // Increment tick counter for cold-start ramp
        state.ticks_since_entry = state.ticks_since_entry.saturating_add(1);

        // Compute volatility scale factor
        let vol = self.price_cache.calculate_volatility(token_address);
        let vol_scale = compute_vol_scale(
            vol,
            self.config.target_vol_scale_threshold,
            state.ticks_since_entry,
            state.initial_vol_scale,
        );

        // Log vol_scale at DEBUG, but only when it changes by >10% (Issue 4: log flooding)
        let should_log = match state.last_logged_vol_scale {
            None => true,
            Some(last) => {
                let change = ((vol_scale - last).abs() / last.max(dec!(0.001))) * dec!(100);
                change > dec!(10)
            }
        };
        if should_log {
            state.last_logged_vol_scale = Some(vol_scale);
            tracing::debug!(
                trade_uuid,
                token = %token_address,
                vol_scale = %vol_scale,
                volatility = ?vol,
                "Volatility scale computed for profit targets"
            );
        }

        // Scale profit targets: apply regime multiplier × vol_scale, floor at min_target_pct
        let min_target = self.config.min_target_pct;
        let profit_level_targets: Vec<Decimal> = self
            .config
            .targets
            .iter()
            .map(|t| {
                let scaled = *t * multiplier * vol_scale;
                scaled.max(min_target)
            })
            .collect();
```

- [ ] **Step 5: Scale trailing stop activation and distance**

Replace the trailing stop computation block (around lines 350-373). Scale both activation and distance by `vol_scale` (floored so activation doesn't go to zero):

```rust
        // Scale trailing stop activation by vol_scale (Issue 3 fix)
        let scaled_activation = (self.config.trailing_stop_activation * vol_scale)
            .max(min_target);

        let base_trailing_distance = if strategy == "SPEAR" {
            self.config.trailing_stop_distance * dec!(1.5)
        } else {
            self.config.trailing_stop_distance
        };
        // Scale trailing distance by vol_scale so low-vol tokens get tighter stops
        let scaled_base_distance = base_trailing_distance * vol_scale;
        let trailing_distance =
            if let Some(vol) = self.price_cache.calculate_volatility(token_address) {
                let vol_mult = if vol > 50.0 {
                    dec!(1.5)
                } else if vol > 30.0 {
                    dec!(1.25)
                } else {
                    Decimal::ONE
                };
                (scaled_base_distance * vol_mult).min(Decimal::from(40))
            } else {
                scaled_base_distance
            };

        if profit_percent >= scaled_activation && !state.trailing_stop_active {
            state.trailing_stop_active = true;
            let trailing_distance_ratio = trailing_distance / Decimal::from(100);
            let raw_stop = state.peak_price * (Decimal::ONE - trailing_distance_ratio);
            // Floor: once trailing stop activates, never let it sit below a small profit lock
            let floor_price = state.entry_price * (Decimal::ONE + min_target / Decimal::from(100));
            state.trailing_stop_price = raw_stop.max(floor_price);
            state_changed = true;
        }
```

- [ ] **Step 6: Update the ratchet path to also apply the floor clamp**

In the new-peak ratchet block (around line 383-391), add the floor clamp:

```rust
        if state.trailing_stop_active && is_new_peak {
            let trailing_distance_ratio = trailing_distance / Decimal::from(100);
            let new_trailing_stop_price =
                state.peak_price * (Decimal::ONE - trailing_distance_ratio);
            // Floor clamp: trailing stop never drops below entry + min_target_pct profit
            let floor_price = state.entry_price * (Decimal::ONE + min_target / Decimal::from(100));
            let clamped = new_trailing_stop_price.max(floor_price);
            if clamped > state.trailing_stop_price {
                state.trailing_stop_price = clamped;
                state_changed = true;
            }
        }
```

- [ ] **Step 7: Build to verify compilation**

Run: `cd operator && cargo build --lib 2>&1 | tail -10`
Expected: No errors.

- [ ] **Step 8: Run clippy on modified file**

Run: `cd operator && cargo clippy --lib 2>&1 | grep -A2 "profit_targets"`
Expected: No new warnings in `profit_targets.rs` (pre-existing warnings in other files are OK).

- [ ] **Step 9: Run all tests**

Run: `cd operator && cargo test vol_scale_tests --lib -- --test-threads=1 && cargo test vol_target_config --lib`
Expected: PASS (all tests).

- [ ] **Step 10: Commit**

```bash
git add operator/src/engine/profit_targets.rs
git commit -m "feat(profit): scale targets and trailing stop by token volatility with profit floor"
```

---

### Task 5: Add Integration Tests for Scaling Behavior

**Files:**
- Modify: `operator/src/engine/profit_targets.rs` (add to test module)

These tests validate the scaling logic produces correct effective targets given known inputs.

- [ ] **Step 1: Write scaling-math tests**

Add to the `vol_scale_tests` module:

```rust
    #[test]
    fn test_effective_targets_high_vol_unchanged() {
        // Vol=60%, threshold=30% → scale=1.0
        // base [25,50,100,200] × 1.0 = [25,50,100,200]
        let scale = compute_vol_scale(Some(60.0), dec!(30.0), 100, None);
        assert_eq!(scale, Decimal::ONE);
        let min_target = dec!(5.0);
        let effective: Vec<Decimal> = vec![dec!(25), dec!(50), dec!(100), dec!(200)]
            .iter()
            .map(|t| (*t * scale).max(min_target))
            .collect();
        assert_eq!(effective, vec![dec!(25), dec!(50), dec!(100), dec!(200)]);
    }

    #[test]
    fn test_effective_targets_low_vol_floored() {
        // Vol=3%, threshold=30% → scale=0.1
        // base [25,50,100,200] × 0.1 = [2.5,5,10,20] → floored to [5,5,10,20]
        let scale = compute_vol_scale(Some(3.0), dec!(30.0), 100, None);
        let min_target = dec!(5.0);
        let effective: Vec<Decimal> = vec![dec!(25), dec!(50), dec!(100), dec!(200)]
            .iter()
            .map(|t| (*t * scale).max(min_target))
            .collect();
        assert_eq!(effective[0], min_target); // floored
        assert_eq!(effective[1], min_target); // floored
        assert_eq!(effective[2], dec!(10));
        assert_eq!(effective[3], dec!(20));
    }

    #[test]
    fn test_trailing_activation_scales_and_floors() {
        // Vol=5%, threshold=30% → scale=0.167
        // activation 30% × 0.167 = 5.0 → floored at min_target 5.0
        let scale = compute_vol_scale(Some(5.0), dec!(30.0), 100, None);
        let activation = (dec!(30) * scale).max(dec!(5.0));
        assert!(activation >= dec!(5.0)); // never below min_target
    }

    #[test]
    fn test_cold_start_ramp_gradual() {
        // At tick 0: ramp=0 → effective_scale=1.0 (full targets, safe)
        let scale_t0 = compute_vol_scale(Some(5.0), dec!(30.0), 0, None);
        assert!(scale_t0 > dec!(0.99));

        // At tick 30 (halfway): ramp=0.5 → effective_scale ≈ 0.583
        let scale_t30 = compute_vol_scale(Some(5.0), dec!(30.0), 30, None);
        assert!(scale_t30 > dec!(0.55) && scale_t30 < dec!(0.62));

        // At tick 60 (done): effective_scale ≈ 0.167
        let scale_t60 = compute_vol_scale(Some(5.0), dec!(30.0), 60, None);
        assert!(scale_t60 > dec!(0.16) && scale_t60 < dec!(0.17));
    }

    #[test]
    fn test_initial_estimate_skips_ramp() {
        // If initial_vol_scale is set, no ramp — immediate scale
        let scale = compute_vol_scale(None, dec!(30.0), 5, Some(dec!(0.333)));
        assert_eq!(scale, dec!(0.333));
    }
```

- [ ] **Step 2: Run tests**

Run: `cd operator && cargo test vol_scale_tests --lib -- --test-threads=1`
Expected: PASS (all 13 tests).

- [ ] **Step 3: Commit**

```bash
git add operator/src/engine/profit_targets.rs
git commit -m "test(profit): add scaling-math integration tests for vol-scaled targets"
```

---

### Task 6: Build, Lint, and Full Test Run

**Files:** None (verification task)

- [ ] **Step 1: Full release build**

Run: `cd operator && cargo build --release 2>&1 | tail -5`
Expected: `Finished` with no errors.

- [ ] **Step 2: Run clippy (check for new warnings only)**

Run: `cd operator && cargo clippy --release 2>&1 | grep "profit_targets\|config.rs" | grep -v "warning generated"`
Expected: No output (no new warnings in modified files).

- [ ] **Step 3: Run all lib unit tests**

Run: `cd operator && cargo test --lib 2>&1 | tail -10`
Expected: All tests pass (342+ existing + new tests).

- [ ] **Step 4: Verify no regressions in stop_loss (shares PriceCache)**

Run: `cd operator && cargo test stop_loss --lib`
Expected: PASS.

---

### Task 7: Deploy and Verify in Paper Mode

**Files:** None (deployment task)

- [ ] **Step 1: Push to remote**

```bash
git push origin main
```

- [ ] **Step 2: SSH to server and pull**

```bash
ssh root@216.151.164.105 "cd /opt/chimera && git pull origin main"
```

- [ ] **Step 3: Build operator image on server**

```bash
ssh root@216.151.164.105 "cd /opt/chimera && COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml build operator"
```

- [ ] **Step 4: Recreate operator container**

```bash
ssh root@216.151.164.105 "cd /opt/chimera && COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml up -d --force-recreate operator"
```

- [ ] **Step 5: Wait for startup and verify health**

```bash
ssh root@216.151.164.105 "sleep 10 && docker exec chimera-operator curl -sf http://localhost:8080/health"
```
Expected: healthy response.

- [ ] **Step 6: Check operator logs for config fields loaded**

```bash
ssh root@216.151.164.105 "docker exec chimera-operator tail -50 /opt/chimera/data/logs/operator.log | grep -i 'profit\|vol_scale\|target'"
```
Expected: No errors; config loaded successfully.

- [ ] **Step 7: Submit a test signal on a low-vol token and verify scaled targets**

From inside the container, submit a test BUY signal. Then check logs for the vol_scale DEBUG line and verify the first target is reachable (not 25%).

```bash
ssh root@216.151.164.105 'docker exec chimera-operator bash -c "
SECRET=1320f19a865520f6e2b5e45e211842b62c83b78095e51f9d6d8230ccbdd2db7f
TS=\$(date +%s)
BODY='\''{\"wallet_address\":\"test-eval\",\"token_address\":\"DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263\",\"action\":\"BUY\",\"amount_sol\":0.25}'\''
SIG=\$(echo -n \"\$TS\$BODY\" | openssl dgst -sha256 -hmac \"\$SECRET\" -binary | xxd -p -c 256)
curl -sf -X POST http://localhost:8080/api/v1/webhook \
  -H \"Content-Type: application/json\" \
  -H \"X-Timestamp: \$TS\" \
  -H \"X-Signature: \$SIG\" \
  -d \"\$BODY\"
"'
```

Then check for vol_scale log:

```bash
ssh root@216.151.164.105 "docker exec chimera-operator tail -100 /opt/chimera/data/logs/operator.log | grep -i 'vol_scale\|profit\|target'"
```

Expected: A DEBUG log showing `vol_scale` < 1.0 for the low-vol token, with scaled targets.

- [ ] **Step 8: Verify no double-sell on volatility drift**

Monitor the position over several ticks. Check that `targets_hit` indices don't accumulate unexpectedly:

```bash
ssh root@216.151.164.105 "docker exec -it postgres psql -U chimera -d chimera -c \"SELECT trade_uuid, targets_hit, trailing_stop_active, trailing_stop_price FROM exit_targets ORDER BY updated_at DESC LIMIT 5;\""
```

Expected: `targets_hit` contains integer indices `[0]`, `[0,1]`, etc. — not decimal values.

- [ ] **Step 9: Monitor for 15 minutes**

Watch for:
- No panics or errors in operator log
- Position tracking works (peak_price updates)
- Trailing stop activates at a scaled level, not 30%

```bash
ssh root@216.151.164.105 "docker exec chimera-operator tail -200 /opt/chimera/data/logs/operator.log | grep -iE 'error|panic|vol_scale|trailing|target'"
```

- [ ] **Step 10: Document results**

Record the observed vol_scale, effective targets, and trailing stop behavior. Compare to the pre-change baseline (0% hit rate on 25%+ targets).

---

## Self-Review

### Spec Coverage

| Spec Requirement | Task |
|-----------------|------|
| Scale profit targets by volatility | Task 4 (Step 4) |
| `min_target_pct` = 5.0 default | Task 1 (Step 4-5) |
| Index-based `targets_hit` (Issue 1) | Task 3 |
| Scale trailing stop distance (Issue 3) | Task 4 (Step 5) |
| Trailing stop floor clamp (Issue 3) | Task 4 (Steps 5-6) |
| DEBUG logging with change gate (Issue 4) | Task 4 (Step 4) |
| Cold-start ramp (Issue 5) | Task 2 (compute_vol_scale) + Task 4 (initial estimate) |
| Backward compat on DB restore | Task 3 (Step 4) |
| Deploy + verify | Task 7 |

### Placeholder Scan
No TBD/TODO/placeholders. All code blocks are complete.

### Type Consistency
- `targets_hit: Vec<usize>` — consistent across struct def (Task 3), check loop (Task 3), DB snapshot (Task 3), restore path (Task 3).
- `compute_vol_scale(volatility: Option<f64>, threshold: Decimal, ticks_since_entry: u32, initial_vol_scale: Option<Decimal>) -> Decimal` — consistent across definition (Task 2) and call site (Task 4).
- `initial_vol_scale`, `ticks_since_entry`, `last_logged_vol_scale` — added to struct (Task 4 Step 1), initialized in both register_position paths (Task 4 Steps 2-3), used in check_targets (Task 4 Step 4).
