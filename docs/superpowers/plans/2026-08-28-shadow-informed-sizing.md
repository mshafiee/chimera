# Shadow-Tiered Proven Sizing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Modulate the proven-wallet position size by the wallet's trailing deduped shadow edge, so a +18.6%-expectancy star wallet and a +1.3% marginal wallet no longer size identically.

**Architecture:** A new read-only DB query (`get_wallet_shadow_kelly_stats`) aggregates a wallet's trailing-30d deduped `mirror_main` shadow exits into Kelly-style inputs (samples, win_rate, avg_win, avg_loss). A pure tier function maps net expectancy (after cost) to a multiplier (0.5×–1.5×) applied on top of the existing proven-sizing override (`proven_size_pct`) in `PositionSizer::calculate_size`. Dark-launch default OFF; enabled via `config.yaml`.

**Tech Stack:** Rust (operator `chimera_operator`, infra `chimera_infra`, config in `chimera_core`), sqlx/PostgreSQL, rust_decimal. Tests: DB-backed integration tests against a disposable Postgres (`TEST_DATABASE_URL`), pure unit tests for the tier function.

## Global Constraints

- Financial values: `rust_decimal::Decimal` only — never f64 (root AGENTS.md).
- SQL: sqlx with `$n` placeholders, PostgreSQL only.
- Dark launch: `shadow_kelly_enabled` defaults to `false`; nothing changes until `config.yaml` flips it.
- Fail-open: shadow-stats lookup errors must never block a proven trade — log warn, apply flat `proven_size_pct`.
- Dormancy/evidence guard: fewer than `shadow_kelly_min_samples` exits in the window → multiplier 1.0 (unchanged behavior). Absence of evidence is not negative evidence (2026-08-17 star-wallet blackout lesson).
- Dedup consistency: same read-side dedup as `get_wallet_pnl_statistics` — one exit per `(token, hour-of-opened_at)`, exclude `no_price` exits.
- Exits stay frozen: this plan does NOT touch exit rails (mirror_v3 100-trade verdict pending).
- Scope: P1 only. P2 (consensus netting) is a separate future plan.
- All existing tests must keep passing; pre-existing failures in `tiered_polling_tests` (stale defaults vs prod tuning, 8 failures on clean HEAD) are out of scope.

---

### Task 1: `ShadowKellyStats` + DB trait method

**Files:**
- Modify: `infra/src/db_abstraction/mod.rs` (add struct + trait method next to `get_wallet_pnl_statistics` declaration)
- Modify: `infra/src/db_abstraction/postgres.rs` (impl, near `get_wallet_pnl_statistics` at line ~4057)
- Modify: `operator/src/monitoring/test_db.rs` (MockDb stub — keep all `impl Database for` blocks compiling)
- Modify: `infra/src/engine/kelly_sizer.rs` (MockDatabase stub in `#[cfg(test)] mod tests`, near `get_wallet_pnl_statistics` mock at line ~1245)

**Interfaces:**
- Produces: `pub struct ShadowKellyStats { pub samples: i64, pub win_rate: Decimal, pub avg_win: Decimal, pub avg_loss: Decimal }` (fractions: `avg_win = 0.04` means +4%); `Database::get_wallet_shadow_kelly_stats(&self, wallet_address: &str, window_days: i32) -> AppResult<Option<ShadowKellyStats>>`. Task 2 and Task 4 consume these exact names.

- [ ] **Step 1: Add the struct and trait declaration**

In `infra/src/db_abstraction/mod.rs`, add near the other result structs (by `DbPool`):

```rust
/// Kelly-style inputs derived from a wallet's trailing deduped mirror_main
/// shadow exits (2026-08-28 shadow-tiered proven sizing). `avg_win`/`avg_loss`
/// are per-trade return FRACTIONS (0.04 = +4%); `win_rate` is 0.0–1.0.
#[derive(Debug, Clone)]
pub struct ShadowKellyStats {
    pub samples: i64,
    pub win_rate: Decimal,
    pub avg_win: Decimal,
    pub avg_loss: Decimal,
}
```

In the `Database` trait, directly after the `get_wallet_pnl_statistics` declaration, add:

```rust
    /// Trailing deduped mirror_main shadow-exit stats for one wallet.
    /// None when the wallet has no qualifying exits in the window.
    async fn get_wallet_shadow_kelly_stats(
        &self,
        wallet_address: &str,
        window_days: i32,
    ) -> AppResult<Option<ShadowKellyStats>>;
```

Confirm `use rust_decimal::Decimal;` exists at the top of `mod.rs` (it does — `get_wallet_realized_pnl_window` returns `Option<Decimal>`).

- [ ] **Step 2: Implement in postgres.rs**

In `infra/src/db_abstraction/postgres.rs`, directly after the `get_wallet_pnl_statistics` impl (ends near line ~4090), add — the SQL mirrors that method's dedup exactly, restricted to `mirror_main`:

```rust
    async fn get_wallet_shadow_kelly_stats(
        &self,
        wallet_address: &str,
        window_days: i32,
    ) -> AppResult<Option<ShadowKellyStats>> {
        let row: Option<(i64, Option<f64>, Option<Decimal>, Option<Decimal>)> = sqlx::query_as(
            // Dedup mirrors get_wallet_pnl_statistics (2026-08-14): one exit
            // per (token, hour of opened_at). mirror_main only — the promoter
            // bar this sizing tiers on is mirror_main-based. no_price exits
            // excluded (zero-PnL distortions).
            r#"WITH dedup AS (
                 SELECT DISTINCT ON (sp.token_address, date_trunc('hour', sp.opened_at))
                        se.pnl_pct
                 FROM shadow_exits se
                 JOIN shadow_positions sp ON sp.shadow_id = se.shadow_id
                 WHERE sp.wallet_address = $1
                   AND se.exit_strategy = 'mirror_main'
                   AND se.exit_reason IS DISTINCT FROM 'no_price'
                   AND se.pnl_pct IS NOT NULL
                   AND sp.opened_at > NOW() - ($2 || ' days')::interval
                 ORDER BY sp.token_address, date_trunc('hour', sp.opened_at), sp.opened_at
               )
               SELECT COUNT(*)::bigint,
                      COUNT(*) FILTER (WHERE pnl_pct > 0)::float8 / NULLIF(COUNT(*), 0),
                      AVG(pnl_pct) FILTER (WHERE pnl_pct > 0),
                      AVG(ABS(pnl_pct)) FILTER (WHERE pnl_pct < 0)
               FROM dedup
               HAVING COUNT(*) > 0"#,
        )
        .bind(wallet_address)
        .bind(window_days)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::Database)?;

        Ok(row.map(|(samples, win_rate, avg_win, avg_loss)| ShadowKellyStats {
            samples,
            win_rate: Decimal::from_f64_retain(win_rate.unwrap_or(0.0))
                .unwrap_or(Decimal::ZERO),
            // pnl_pct is stored as percent (4.0 = +4%) — convert to fraction.
            avg_win: avg_win.unwrap_or(Decimal::ZERO) / Decimal::from(100),
            avg_loss: avg_loss.unwrap_or(Decimal::ZERO) / Decimal::from(100),
        }))
    }
```

`use rust_decimal::prelude::*;` is already imported in this file (line 15) — `from_f64_retain` resolves. Add `ShadowKellyStats` to the existing `use crate::db_abstraction::...` import list in this file if it is not glob-imported.

- [ ] **Step 3: Stub in operator MockDb**

In `operator/src/monitoring/test_db.rs`, inside `impl Database for MockDb`, add (after the `get_wallet_realized_pnl_window` stub):

```rust
    async fn get_wallet_shadow_kelly_stats(
        &self,
        _wallet_address: &str,
        _window_days: i32,
    ) -> AppResult<Option<ShadowKellyStats>> {
        Ok(None)
    }
```

Extend the file's existing `use chimera_operator::db_abstraction::...` import with `ShadowKellyStats`.

- [ ] **Step 4: Stub in infra MockDatabase**

In `infra/src/engine/kelly_sizer.rs` test module: add the field to `MockDatabase` (next to `wallet_pnl_stats` at line ~407):

```rust
        pub shadow_kelly_stats: RwLock<HashMap<String, Option<ShadowKellyStats>>>,
```

(`#[derive(Default)]` covers it; `RwLock` here is `parking_lot::RwLock`.) Inside `impl Database for MockDatabase`, after the `get_wallet_pnl_statistics` mock (line ~1245), add — same read pattern:

```rust
        async fn get_wallet_shadow_kelly_stats(
            &self,
            wallet_address: &str,
            _window_days: i32,
        ) -> AppResult<Option<ShadowKellyStats>> {
            Ok(self
                .shadow_kelly_stats
                .read()
                .get(wallet_address)
                .cloned()
                .unwrap_or(None))
        }
```

Add `ShadowKellyStats` to the test module's `use crate::db_abstraction::*;` (glob already covers it).

- [ ] **Step 5: Compile check**

Run: `cargo check -p chimera_infra -p chimera_operator --all-targets 2>&1 | grep -cE "^error"`
Expected: `0`

- [ ] **Step 6: Commit**

```bash
git add infra/src/db_abstraction/mod.rs infra/src/db_abstraction/postgres.rs operator/src/monitoring/test_db.rs infra/src/engine/kelly_sizer.rs
git commit -m "feat(sizing): get_wallet_shadow_kelly_stats — trailing deduped mirror_main Kelly inputs"
```

---

### Task 2: Pure tier function `shadow_proven_size_multiplier`

**Files:**
- Modify: `operator/src/engine/position_sizer.rs` (new private fn + import)
- Test: `operator/src/engine/position_sizer.rs` (inline `#[cfg(test)]` if the file has one; otherwise add to `operator/tests/unit/position_sizer_tests.rs` — this plan uses the external test file)

**Interfaces:**
- Consumes: `ShadowKellyStats` (Task 1).
- Produces: `fn shadow_proven_size_multiplier(stats: &ShadowKellyStats, cost_pct: Decimal, min_samples: i64) -> Decimal` returning one of `1.5 | 1.25 | 1.0 | 0.5`. Task 4 calls this.

- [ ] **Step 1: Write the failing tests**

Append to `operator/tests/unit/position_sizer_tests.rs`:

```rust
// ─── Shadow-tier proven sizing multiplier (2026-08-28) ──────────────────────

use chimera_operator::db_abstraction::ShadowKellyStats;
use chimera_operator::engine::position_sizer::shadow_proven_size_multiplier;

fn shadow_stats(samples: i64, win: &str, avg_win: &str, avg_loss: &str) -> ShadowKellyStats {
    ShadowKellyStats {
        samples,
        win_rate: Decimal::from_str(win).unwrap(),
        avg_win: Decimal::from_str(avg_win).unwrap(),
        avg_loss: Decimal::from_str(avg_loss).unwrap(),
    }
}

#[test]
fn test_shadow_tier_star_edge() {
    // p=0.8, aw=0.20, al=0.02 -> expectancy 15.6% gross, 15.1% net >= 10 -> 1.5x
    let stats = shadow_stats(25, "0.8", "0.20", "0.02");
    assert_eq!(
        shadow_proven_size_multiplier(&stats, Decimal::from_str("0.5").unwrap(), 20),
        Decimal::from_str("1.5").unwrap()
    );
}

#[test]
fn test_shadow_tier_strong_edge() {
    // expectancy 6.0% gross, 5.5% net in [5, 10) -> 1.25x
    let stats = shadow_stats(25, "0.8", "0.08", "0.02");
    assert_eq!(
        shadow_proven_size_multiplier(&stats, Decimal::from_str("0.5").unwrap(), 20),
        Decimal::from_str("1.25").unwrap()
    );
}

#[test]
fn test_shadow_tier_net_clear_edge() {
    // expectancy 2.8% gross, 2.3% net in [0, 5) -> 1.0x (unchanged behavior)
    let stats = shadow_stats(25, "0.8", "0.04", "0.02");
    assert_eq!(
        shadow_proven_size_multiplier(&stats, Decimal::from_str("0.5").unwrap(), 20),
        Decimal::ONE
    );
}

#[test]
fn test_shadow_tier_below_cost() {
    // expectancy 0.2% gross, -0.3% net < 0 -> 0.5x (defensive)
    let stats = shadow_stats(25, "0.8", "0.01", "0.03");
    assert_eq!(
        shadow_proven_size_multiplier(&stats, Decimal::from_str("0.5").unwrap(), 20),
        Decimal::from_str("0.5").unwrap()
    );
}

#[test]
fn test_shadow_tier_exact_star_boundary() {
    // p=0.5, aw=0.21, al=0.01 -> expectancy 10.0% gross, 9.5% gross - 0.5 cost
    // = 9.5 net... use win 0.5/aw 0.21/al 0.005: 0.105 - 0.005 = 0.10 -> 10.0 net
    let stats = shadow_stats(25, "0.5", "0.21", "0.005");
    assert_eq!(
        shadow_proven_size_multiplier(&stats, Decimal::from_str("0.5").unwrap(), 20),
        Decimal::from_str("1.5").unwrap()
    );
}

#[test]
fn test_shadow_tier_thin_evidence_is_neutral() {
    // Absence of evidence is NOT negative evidence: thin book -> 1.0x.
    let stats = shadow_stats(5, "0.0", "0.0", "0.5");
    assert_eq!(
        shadow_proven_size_multiplier(&stats, Decimal::from_str("0.5").unwrap(), 20),
        Decimal::ONE
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p chimera_operator --test unit position_sizer_tests::test_shadow_tier 2>&1 | tail -3`
Expected: FAIL — `shadow_proven_size_multiplier` not found / `ShadowKellyStats` import error if Task 1 not compiled in test scope (it is — glob re-export).

- [ ] **Step 3: Implement**

In `operator/src/engine/position_sizer.rs`, add below the imports:

```rust
use chimera_operator::db_abstraction::ShadowKellyStats;

/// Map trailing deduped shadow-edge stats to a proven-size tier multiplier.
///
/// Net expectancy (gross minus `cost_pct`) drives the tier:
///   >= +10% -> 1.5x  (star: the 132Tkgf5YE class, +18.6% gross)
///   >= +5%  -> 1.25x
///   >= 0%   -> 1.0x  (net-clear — the promotion bar; unchanged behavior)
///    < 0%   -> 0.5x  (trailing below-cost drift: defensive)
/// Thin evidence (< min_samples) is NEUTRAL 1.0x — absence of evidence is not
/// negative evidence (coverage loss, not bleeding, is the blackout failure mode).
fn shadow_proven_size_multiplier(
    stats: &ShadowKellyStats,
    cost_pct: Decimal,
    min_samples: i64,
) -> Decimal {
    if stats.samples < min_samples {
        return Decimal::ONE;
    }
    let expectancy_pct =
        (stats.win_rate * stats.avg_win - (Decimal::ONE - stats.win_rate) * stats.avg_loss)
            * Decimal::from(100);
    let net_pct = expectancy_pct - cost_pct;
    if net_pct >= Decimal::from(10) {
        Decimal::from_str("1.5").unwrap_or(Decimal::ONE)
    } else if net_pct >= Decimal::from(5) {
        Decimal::from_str("1.25").unwrap_or(Decimal::ONE)
    } else if net_pct >= Decimal::ZERO {
        Decimal::ONE
    } else {
        Decimal::from_str("0.5").unwrap_or(Decimal::ONE)
    }
}
```

Check `use std::str::FromStr;` is present in this file's imports (add if missing).

- [ ] **Step 4: Export from the engine module**

In `operator/src/engine/mod.rs`, next to `pub use position_sizer::PositionSizer;` add:

```rust
pub use position_sizer::shadow_proven_size_multiplier;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p chimera_operator --test unit position_sizer_tests::test_shadow_tier 2>&1 | tail -3`
Expected: 6 passed

- [ ] **Step 6: Commit**

```bash
git add operator/src/engine/position_sizer.rs operator/src/engine/mod.rs operator/tests/unit/position_sizer_tests.rs
git commit -m "feat(sizing): shadow-edge tier multiplier for proven sizing (pure fn + tests)"
```

---

### Task 3: Config fields

**Files:**
- Modify: `core/src/config.rs` (`PositionSizingConfig`, fields near `proven_sizing_boost` at line ~2333; `Default` impl at line ~2525; default fns near line ~2447)

**Interfaces:**
- Produces: `PositionSizingConfig { ..., shadow_kelly_enabled: bool, shadow_kelly_window_days: i32, shadow_kelly_min_samples: i64, shadow_kelly_cost_pct: Decimal }`. Task 4 consumes these.

- [ ] **Step 1: Add fields to `PositionSizingConfig`**

After `proven_sizing_boost` (line ~2333):

```rust
    /// Shadow-tiered proven sizing (2026-08-28): modulate proven_size_pct by
    /// the wallet's trailing deduped mirror_main expectancy. Dark-launch
    /// default false — enable via config.yaml `position_sizing`.
    #[serde(default)]
    pub shadow_kelly_enabled: bool,
    /// Trailing window (days) for the shadow evidence.
    #[serde(default = "default_shadow_kelly_window_days")]
    pub shadow_kelly_window_days: i32,
    /// Minimum deduped exits in the window; fewer -> neutral 1.0x tier.
    #[serde(default = "default_shadow_kelly_min_samples")]
    pub shadow_kelly_min_samples: i64,
    /// Round-trip cost percent subtracted from gross shadow expectancy.
    #[serde(default = "default_shadow_kelly_cost_pct")]
    pub shadow_kelly_cost_pct: Decimal,
```

- [ ] **Step 2: Add default fns**

Near `default_min_size_sol` (line ~2447):

```rust
fn default_shadow_kelly_window_days() -> i32 {
    30
}

fn default_shadow_kelly_min_samples() -> i64 {
    20
}

fn default_shadow_kelly_cost_pct() -> Decimal {
    dec!(0.5)
}
```

- [ ] **Step 3: Add to the `Default` impl**

In the `Default for PositionSizingConfig` block (line ~2525), after `proven_sizing_boost: true,`:

```rust
            shadow_kelly_enabled: false,
            shadow_kelly_window_days: default_shadow_kelly_window_days(),
            shadow_kelly_min_samples: default_shadow_kelly_min_samples(),
            shadow_kelly_cost_pct: default_shadow_kelly_cost_pct(),
```

- [ ] **Step 4: Compile + commit**

Run: `cargo check -p chimera_core -p chimera_operator --all-targets 2>&1 | grep -cE "^error"`
Expected: `0`

```bash
git add core/src/config.rs
git commit -m "feat(config): shadow-tiered proven sizing flags (dark-launch default off)"
```

---

### Task 4: Proven-override integration

**Files:**
- Modify: `operator/src/engine/position_sizer.rs` (proven override at line ~466)
- Test: `operator/tests/unit/position_sizer_tests.rs` (DB-backed — this file uses real Postgres via `create_test_db()`)

**Interfaces:**
- Consumes: Task 1 `get_wallet_shadow_kelly_stats`, Task 2 `shadow_proven_size_multiplier`, Task 3 config fields.
- Produces: proven override now multiplies by the tier multiplier; log marker `"shadow-tier proven sizing applied"` for ops verification.

- [ ] **Step 1: Write the failing tests**

Append to `operator/tests/unit/position_sizer_tests.rs`:

```rust
// ─── Shadow-tier proven sizing integration ──────────────────────────────────

/// Seed `wins` wins at `win_pct` and `losses` losses at `loss_pct` as deduped
/// mirror_main shadow exits for `wallet` (one per hour — dedup key).
async fn seed_shadow_exits(
    pool: &Pool<Postgres>,
    wallet: &str,
    wins: usize,
    win_pct: &str,
    losses: usize,
    loss_pct: &str,
) {
    let mut hour: i32 = 1;
    for pct in std::iter::repeat(win_pct).take(wins).chain(std::iter::repeat(loss_pct).take(losses)) {
        let sid = format!("sk-{}-{}", wallet, hour);
        sqlx::query(
            "INSERT INTO shadow_positions (shadow_id, decision_id, run_id, wallet_address, token_address, main_admitted, entry_amount_sol, ingress, opened_at) \
             VALUES ($1, 'd', 'run', $2, 'seedtoken', false, 0.1, 'webhook', NOW() - make_interval(hours => $3))",
        )
        .bind(&sid)
        .bind(wallet)
        .bind(hour)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO shadow_exits (shadow_id, exit_strategy, pnl_pct, exit_reason) \
             VALUES ($1, 'mirror_main', $2, 'profit_target')",
        )
        .bind(&sid)
        .bind(Decimal::from_str(pct).unwrap())
        .execute(pool)
        .await
        .unwrap();
        hour += 1;
    }
}

fn shadow_sizing_config(enabled: bool) -> Arc<PositionSizingConfig> {
    Arc::new(PositionSizingConfig {
        shadow_kelly_enabled: enabled,
        shadow_kelly_haircut_unused_guard: (), // placeholder removed below
        ..PositionSizingConfig::default()
    })
}
```

**IMPORTANT:** remove the `shadow_kelly_haircut_unused_guard` line — the final helper is:

```rust
fn shadow_sizing_config(enabled: bool) -> Arc<PositionSizingConfig> {
    Arc::new(PositionSizingConfig {
        shadow_kelly_enabled: enabled,
        ..PositionSizingConfig::default()
    })
}
```

Then the tests (factors via the existing `neutral_factors()` helper — wallet `test_wallet`, capital 10.0, WQS 50):

```rust
#[tokio::test]
async fn test_shadow_tier_star_proven_sized_up() {
    // 20 wins +20%, 5 losses -2% -> expectancy 15.6% gross, 15.1% net -> 1.5x.
    // proven base = 10 * 0.15 = 1.5 -> tiered 2.25 -> strategy_max = min(10*0.30, 2.0) = 2.0.
    let (db, _guard) = create_test_db().await;
    seed_shadow_exits(&pg_pool(&db), "test_wallet", 20, "20.0", 5, "-2.0").await;

    let mut factors = neutral_factors();
    factors.is_proven = true;
    let sizer = PositionSizer::new(db, shadow_sizing_config(true));
    let size = sizer.calculate_size(factors).await.unwrap();
    assert_eq!(size, Decimal::from_str("2.0").unwrap());
}

#[tokio::test]
async fn test_shadow_tier_below_cost_proven_sized_down() {
    // 20 wins +1%, 5 losses -3% -> expectancy 0.2% gross, -0.3% net -> 0.5x.
    // proven base 1.5 -> 0.75 (strategy_max 2.0 does not bind).
    let (db, _guard) = create_test_db().await;
    seed_shadow_exits(&pg_pool(&db), "test_wallet", 20, "1.0", 5, "-3.0").await;

    let mut factors = neutral_factors();
    factors.is_proven = true;
    let sizer = PositionSizer::new(db, shadow_sizing_config(true));
    let size = sizer.calculate_size(factors).await.unwrap();
    assert_eq!(size, Decimal::from_str("0.75").unwrap());
}

#[tokio::test]
async fn test_shadow_tier_disabled_keeps_flat_proven_size() {
    // Dark-launch guard: disabled -> flat proven_size_pct (1.5), no DB call effect.
    let (db, _guard) = create_test_db().await;
    seed_shadow_exits(&pg_pool(&db), "test_wallet", 20, "20.0", 5, "-2.0").await;

    let mut factors = neutral_factors();
    factors.is_proven = true;
    let sizer = PositionSizer::new(db, shadow_sizing_config(false));
    let size = sizer.calculate_size(factors).await.unwrap();
    assert_eq!(size, Decimal::from_str("1.5").unwrap());
}

#[tokio::test]
async fn test_shadow_tier_thin_evidence_flat_proven_size() {
    // 3 exits only (< 20 min samples) -> neutral 1.0x even when enabled.
    let (db, _guard) = create_test_db().await;
    seed_shadow_exits(&pg_pool(&db), "test_wallet", 3, "20.0", 0, "0").await;

    let mut factors = neutral_factors();
    factors.is_proven = true;
    let sizer = PositionSizer::new(db, shadow_sizing_config(true));
    let size = sizer.calculate_size(factors).await.unwrap();
    assert_eq!(size, Decimal::from_str("1.5").unwrap());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p chimera_operator --test unit position_sizer_tests::test_shadow_tier_star_proven 2>&1 | tail -3`
Expected: FAIL — star case returns `1.5` (flat proven) instead of `2.0`.

- [ ] **Step 3: Implement**

Replace the proven override block (position_sizer.rs, line ~466):

```rust
        if self.config.proven_sizing_boost && factors.is_proven {
            let mut tier_mult = Decimal::ONE;
            if self.config.shadow_kelly_enabled {
                match self
                    .db
                    .get_wallet_shadow_kelly_stats(
                        &factors.wallet_address,
                        self.config.shadow_kelly_window_days,
                    )
                    .await
                {
                    Ok(Some(stats)) => {
                        tier_mult = shadow_proven_size_multiplier(
                            &stats,
                            self.config.shadow_kelly_cost_pct,
                            self.config.shadow_kelly_min_samples,
                        );
                        tracing::info!(
                            wallet = %factors.wallet_address,
                            samples = stats.samples,
                            tier_mult = %tier_mult,
                            "shadow-tier proven sizing applied"
                        );
                    }
                    Ok(None) => {
                        tracing::debug!(
                            wallet = %factors.wallet_address,
                            "shadow-tier sizing: no trailing shadow evidence — flat proven size"
                        );
                    }
                    Err(e) => {
                        // Fail-open: a stats lookup failure must never block a
                        // proven trade — flat proven_size_pct applies.
                        tracing::warn!(
                            wallet = %factors.wallet_address,
                            error = %e,
                            "shadow-tier sizing lookup failed — flat proven size"
                        );
                    }
                }
            }
            tracing::info!(
                wallet = %factors.wallet_address,
                strategy = ?factors.strategy,
                wqs_chain_size = %size,
                proven_size_sol = %self.config.proven_size_pct,
                tier_mult = %tier_mult,
                "Proven-wallet sizing override applied (bypasses WQS × confidence chain)"
            );
            size = ((capital * self.config.proven_size_pct) * tier_mult)
                .min(self.config.max_size_sol);
        }
```

Keep the original log line text (`"Proven-wallet sizing override applied…"`) plus the added `tier_mult` field so existing dashboards/greps keep matching.

- [ ] **Step 4: Run tests to verify they pass**

Run: `TEST_DATABASE_URL=postgresql://postgres:test@localhost:54329/postgres cargo test -p chimera_operator --test unit position_sizer_tests -- --test-threads=1 2>&1 | tail -3`
Expected: all pass (existing + 4 new). If no local PG: `docker run -d --name chimera-test-pg -e POSTGRES_PASSWORD=test -p 54329:5432 postgres:16` first; remove it after.

- [ ] **Step 5: Commit**

```bash
git add operator/src/engine/position_sizer.rs operator/tests/unit/position_sizer_tests.rs
git commit -m "feat(sizing): shadow-tiered proven sizing — trailing edge modulates proven_size_pct (dark-launch off)"
```

---

### Task 5: Full verification + deploy

**Files:** none new.

- [ ] **Step 1: Full affected-suite pass**

```bash
docker run -d --name chimera-test-pg -e POSTGRES_PASSWORD=test -p 54329:5432 postgres:16
export TEST_DATABASE_URL="postgresql://postgres:test@localhost:54329/postgres"
cargo test -p chimera_operator --test unit position_sizer -- --test-threads=1
cargo test -p chimera_operator --test unit selection_coverage -- --test-threads=1
cargo test -p chimera_operator --test integration webhook_flow -- --test-threads=1
cargo test -p chimera_operator --test unit kelly_sizer -- --test-threads=1
cargo clippy -p chimera_operator -p chimera_infra -p chimera_api 2>&1 | grep -cE "^error"
cargo fmt -p chimera_operator -p chimera_infra -p chimera_core -- --check 2>&1 | grep -E "position_sizer|kelly_sizer|db_abstraction|config.rs" || echo FMT_OK
docker rm -f chimera-test-pg
```
Expected: all suites pass (position_sizer suite must be serial `--test-threads=1` — shared seed-token rows), clippy `0`, fmt clean on touched files.

- [ ] **Step 2: Push**

```bash
git push origin main
```

- [ ] **Step 3: Deploy (server rebuild, no schema migration)**

```bash
ssh root@chimera-01.moez.tech 'cd /opt/chimera && git pull origin main && nohup sh -c "COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml build operator > /tmp/operator-build.log 2>&1; echo BUILD_EXIT=\$? >> /tmp/operator-build.log" >/dev/null 2>&1 & echo started'
# poll: grep BUILD_EXIT /tmp/operator-build.log (expect 0; cargo release build ~5-8 min)
ssh root@chimera-01.moez.tech 'cd /opt/chimera && COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml up -d --force-recreate operator && sleep 20 && docker ps --filter name=operator --format "{{.Status}}" && curl -s -o /dev/null -w "health:%{http_code}\n" http://localhost:8080/api/v1/health'
```
Expected: `Up … (healthy)`, `health:200`.

- [ ] **Step 4: Verify dark state**

```bash
ssh root@chimera-01.moez.tech 'docker exec chimera-operator env | grep -c SHADOW_KELLY || echo 0'
```
Expected: `0` — no env overrides exist; the flag lives in config.yaml. Confirm `config/config.yaml` has no `shadow_kelly_enabled` key yet → dark.

- [ ] **Step 5: Enablement (separate ops step, after 24h dark observation)**

Edit `/opt/chimera/config/config.yaml` under `position_sizing:`:

```yaml
  shadow_kelly_enabled: true        # shadow-tiered proven sizing (2026-08-28)
  # shadow_kelly_window_days: 30    # defaults shown; uncomment to override
  # shadow_kelly_min_samples: 20
  # shadow_kelly_cost_pct: 0.5
```

Then `COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml up -d --force-recreate operator` and verify the marker:

```bash
ssh root@chimera-01.moez.tech 'docker logs chimera-operator --since 10m 2>&1 | grep -c "shadow-tier proven sizing applied" || true'
```

**Revert:** set `shadow_kelly_enabled: false` (or remove the key) and recreate. No schema/migration involvement.

---

## Self-Review

- **Spec coverage:** P1 scope = trailing shadow evidence modulates proven sizing, dark-launch, fail-open, dormancy guard. Tasks 1–4 implement; Task 5 verifies/deploys. P2 (consensus netting) explicitly out of scope — follow-up plan.
- **Placeholders:** none — all steps carry complete code; the one "IMPORTANT" note in Task 4 exists to prevent copying an intentionally-shown-then-removed bad line.
- **Type consistency:** `ShadowKellyStats { samples: i64, win_rate: Decimal, avg_win: Decimal, avg_loss: Decimal }` used identically in Tasks 1, 2, 4; `shadow_proven_size_multiplier(&ShadowKellyStats, Decimal, i64) -> Decimal` matches call site in Task 4; config field names match between Task 3 and Task 4.
- **Design change vs original proposal:** the plan originally targeted a pseudo-Kelly fallback in the dormant Kelly branch; investigation showed `use_kelly_sizing: false` in prod and that the **proven sizing override already exists** — so the real gap is edge-differentiation *within* proven sizing. The tier-multiplier design is the corrected, smaller version of the same idea.
