# Execution Reliability (Whale-Sell Skips) + Proving-Lane Observability Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop whale-sell skips from churning the DLQ as fake "capital flow dying" dead-letters, and make the proving lane observable so a starvation like the 12h evidence blackout can never hide again.

**Architecture:** Part A turns the deterministic whale-SELL skip (`copy_wallet_sells=false`) into terminal DLQ bookkeeping at the skip site — the trade row is marked DEAD_LETTER with a terminal-classified reason the moment the pipeline skips it, so the stale-trade sweeper and DLQ retry worker never see it. Part B adds a starvation probe to the existing shadow-gap alarm task (PROVING wallets with zero decisions in 24h → operator alert) and a per-cycle pool-statistics log line in the scout promoter. Investigation findings this plan encodes (2026-08-28): three whale-SELL signals created QUEUED trade rows, were deliberately skipped by the pipeline, then swept stale at ~33min and DLQ-retried once each — zero execution failures, pure bookkeeping churn; separately, the proving lane accrued zero evidence for 12h because the tracked-wallet TTL cache was ACTIVE-only (fixed separately in commit 2c3add6).

**Tech Stack:** Rust (`chimera_operator`, `chimera_infra`), sqlx/PostgreSQL, Python (`scout/core/shadow_promoter.py`). Tests: DB-backed integration tests (disposable Postgres via `TEST_DATABASE_URL`), pure unit tests with monkeypatched fetches.

## Global Constraints

- Financial values: `rust_decimal::Decimal` only — never f64.
- SQL: sqlx with `$n` placeholders, PostgreSQL only.
- Fail-open everywhere: a bookkeeping or stats failure must never block trading, shadow forking, or the promote cycle — log warn and continue.
- The whale-SELL skip itself is CORRECT behavior (`copy_wallet_sells=false` is the production strategy: wallet BUYs are entry signals, exits belong to the position-monitor). Only the bookkeeping changes.
- DLQ terminal-reason mechanism: reasons matching the ILIKE list in `mark_trade_dead_letter` get `can_retry=false` (precedent: risk-gate rejections, 2026-08-23). New terminal reasons must join that list, not bypass it.
- Alarm design: DB-only probes cannot distinguish "provers quiet" from "prover signals dropped" — the starvation alarm deliberately alarms on 24h of zero decisions from a populated pool; the operator response is "check Helius whether provers traded", which is the correct action for both.
- Deploy = git push + server pull + `docker compose build operator scout` + `up -d --force-recreate`. No schema migration in this plan.

---

### Task 1: Whale-sell skip becomes terminal DLQ bookkeeping

**Files:**
- Modify: `operator/src/engine/signal_pipeline.rs` (skip branch at ~line 209 — the block after `skip_wallet_sell_signal(...)`)
- Modify: `infra/src/db_abstraction/postgres.rs` (`mark_trade_dead_letter` terminal ILIKE list, ~line 3917)
- Test: `operator/tests/integration/dlq_terminal_rejection_tests.rs`

**Interfaces:**
- Consumes: `Database::mark_trade_dead_letter(&self, trade_uuid: &str, payload: &str, error: &str) -> AppResult<()>` (existing); `skip_wallet_sell_signal(...)` (existing); `serde_json::to_string(&signal.payload)` (pattern already used at the OFF_HOURS skip, signal_pipeline.rs ~444).
- Produces: reason string constant `"WHALE_SELL_SKIP: ..."` classified terminal in the DLQ. Task 2 does not depend on this; the final review verifies it.

- [ ] **Step 1: Write the failing test**

Append to `operator/tests/integration/dlq_terminal_rejection_tests.rs` (the file already has `seed_trade` (BUY), `retryable_uuids`, and the terminal/transient test pair to mirror):

```rust
/// Seed a SELL trade (the whale-sell skip path creates SELL rows).
async fn seed_sell_trade(db: &Arc<dyn Database>, trade_uuid: &str) {
    sqlx::query(
        "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
         VALUES ($1, '7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU', \
                 '4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R', 'EXIT', 'SELL', 3.0, 'QUEUED')",
    )
    .bind(trade_uuid)
    .execute(&pg_pool(db))
    .await
    .unwrap();
}

/// The whale-SELL skip (copy_wallet_sells=false) is a DETERMINISTIC skip:
/// the exit system owns the position, and replaying the payload can never
/// change the decision. It must be terminal in the DLQ — measured 2026-08-28:
/// three whale-SELL skips created QUEUED rows, were swept stale at ~33min and
/// DLQ-retried once each (pure churn, zero execution intent).
#[tokio::test]
async fn test_whale_sell_skip_is_terminal() {
    let (db, _guard) = common::create_test_db().await;

    let uuid = "whale-sell-skip-1";
    seed_sell_trade(&db, uuid).await;
    let reason = "WHALE_SELL_SKIP: position managed by exit system (copy_wallet_sells=false) — deterministic skip";
    db.mark_trade_dead_letter(uuid, "{}", reason)
        .await
        .expect("dead-letter write succeeds");

    let retryable = retryable_uuids(&db).await;
    assert!(
        !retryable.contains(&uuid.to_string()),
        "WHALE_SELL_SKIP must not be retryable, but {uuid} is"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
docker run -d --name chimera-test-pg -e POSTGRES_PASSWORD=test -p 54329:5432 postgres:16
sleep 4
TEST_DATABASE_URL="postgresql://postgres:test@localhost:54329/postgres" \
  cargo test -p chimera_operator --test integration dlq_terminal_rejection_tests::test_whale_sell_skip_is_terminal 2>&1 | tail -3
```
Expected: FAIL — the assert fires ("WHALE_SELL_SKIP must not be retryable").

- [ ] **Step 3: Add the terminal classification**

In `infra/src/db_abstraction/postgres.rs`, inside `mark_trade_dead_letter`'s `can_retry` ILIKE chain (after the `'%30min cooldown%'` line, keeping the 2026-08-23 comment block intact), append one arm:

```rust
                  -- Whale-SELL skips are DETERMINISTIC (2026-08-28):
                  -- copy_wallet_sells=false means the exit system owns the
                  -- position; replaying the payload re-skips identically.
                  OR $3 ILIKE '%WHALE_SELL_SKIP%'
```

- [ ] **Step 4: Wire the skip branch**

In `operator/src/engine/signal_pipeline.rs`, replace the skip branch body (the `tracing::info!(...)` + `return` after `skip_wallet_sell_signal(...)`) with:

```rust
        if skip_wallet_sell_signal(
            signal.payload.action,
            self.config.strategy.copy_wallet_sells,
            signal.payload.exit_fraction,
        ) {
            tracing::info!(
                trade_uuid = %trade_uuid,
                wallet = %signal.payload.wallet_address,
                token = %signal.token_address().unwrap_or(""),
                "Wallet SELL signal skipped (copy_wallet_sells=false) — position managed by exit system"
            );
            // Terminal bookkeeping (2026-08-28): the queue path already
            // created this trade row QUEUED; leaving it for the stale-trade
            // sweeper made every whale-SELL skip churn a sweeper cancel plus
            // a DLQ retry cycle for a skip that is deterministic. Mark it
            // DEAD_LETTER with a terminal-classified reason instead.
            let skip_reason = "WHALE_SELL_SKIP: position managed by exit system (copy_wallet_sells=false) — deterministic skip";
            if let Err(e) = self
                .db
                .mark_trade_dead_letter(
                    &trade_uuid,
                    &serde_json::to_string(&signal.payload).unwrap_or_default(),
                    skip_reason,
                )
                .await
            {
                // Fail-open: the sweeper still collects the row; do not
                // turn a bookkeeping failure into a trading failure.
                tracing::warn!(
                    trade_uuid = %trade_uuid,
                    error = %e,
                    "Failed to mark skipped whale SELL dead — sweeper will collect it"
                );
            }
            return;
        }
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
TEST_DATABASE_URL="postgresql://postgres:test@localhost:54329/postgres" \
  cargo test -p chimera_operator --test integration dlq_terminal_rejection_tests -- --test-threads=1 2>&1 | tail -2
docker rm -f chimera-test-pg
```
Expected: all pass (existing risk-gate + transient + new whale-sell tests).

- [ ] **Step 6: Commit**

```bash
git add operator/src/engine/signal_pipeline.rs infra/src/db_abstraction/postgres.rs operator/tests/integration/dlq_terminal_rejection_tests.rs
git commit -m "fix(reliability): whale-SELL skips are terminal DLQ bookkeeping — no sweeper/DLQ churn for deterministic skips"
```

---

### Task 2: Proving-lane starvation alarm

**Files:**
- Modify: `infra/src/monitoring/shadow_gap_alarm.rs` (second probe + alert in the existing loop)
- Modify: `infra/src/notifications/mod.rs` (new `NotificationEvent::ProvingLaneStarved` variant — mirror `ShadowRecordingGap`)
- Modify: `infra/src/notifications/discord.rs` and `telegram.rs` (match arms for the new variant)
- Modify: `api/src/main.rs` (alarm task spawn site — pass the interval already wired for the shadow-gap alarm; no new config)

**Interfaces:**
- Consumes: `Database::pool()` (existing, used by `count_missing_shadow_rows`).
- Produces: `NotificationEvent::ProvingLaneStarved { provers: i64, with_decisions: i64 }` and a new private probe `count_proving_decisions_24h(db) -> anyhow::Result<(i64, i64)>` in `shadow_gap_alarm.rs`. The alarm fires only when `provers >= 5 AND with_decisions == 0` sustained two consecutive checks, re-alerting hourly — mirroring the existing gap-alarm state machine.

- [ ] **Step 1: Add the event variant**

In `infra/src/notifications/mod.rs`, next to `ShadowRecordingGap { missing: i64 }`:

```rust
    /// The candidate-proving pool produced zero decisions over a full day
    /// while populated — either every prover went quiet (check Helius) or
    /// prover signals are being dropped upstream (the 2026-08-28 cache
    /// starve class). Data only; no action possible from the DB alone.
    ProvingLaneStarved { provers: i64, with_decisions: i64 },
```

Then add the corresponding match arms in `discord.rs` and `telegram.rs` — copy the `ShadowRecordingGap` arm and adapt title/text:

- Discord arm (inside the existing match, following the `ShadowRecordingGap` formatting pattern):

```rust
            NotificationEvent::ProvingLaneStarved { provers, with_decisions } => (
                "🕳️ Proving lane starved",
                format!(
                    "{with_decisions}/{provers} PROVING wallets produced a decision in 24h — prover signals are being dropped or every prover went quiet. Check Helius activity for PROVING addresses.",
                ),
            ),
```

- Telegram arm: same message shape as the `ShadowRecordingGap` arm (match the file's existing formatting/emoji conventions exactly).

- [ ] **Step 2: Add the probe and alarm logic**

In `infra/src/monitoring/shadow_gap_alarm.rs`, add below `count_missing_shadow_rows`:

```rust
/// PROVING pool size and how many provers produced at least one decision in
/// the trailing 24h. Zero decisions from a populated pool is the 2026-08-28
/// cache-starve signature (12h of provers trading with zero evidence).
async fn count_proving_decisions_24h(
    db: &Arc<dyn Database>,
) -> anyhow::Result<(i64, i64)> {
    use crate::db_abstraction::DbPool;
    let DbPool::PostgreSQL(pool) = db.pool();
    let row: (i64, i64) = sqlx::query_as(
        r#"SELECT
             (SELECT COUNT(*) FROM wallets WHERE status = 'PROVING'),
             (SELECT COUNT(DISTINCT dr.wallet_address)
              FROM decision_records dr
              JOIN wallets w ON w.address = dr.wallet_address
              WHERE w.status = 'PROVING'
                AND dr.received_at > NOW() - INTERVAL '24 hours')"#,
    )
    .fetch_one(&pool)
    .await?;
    Ok(row)
}
```

In the alarm loop, add a second state pair next to the existing ones (`consecutive_positive_starved`, `last_alert_starved`) and inside the `Ok(missing)` arm of the existing match (after the shadow-gap handling, before the `else` clear branch), probe and handle:

```rust
                match count_proving_decisions_24h(&db).await {
                    Ok((provers, with_decisions)) => {
                        let starved = provers >= 5 && with_decisions == 0;
                        if starved {
                            consecutive_positive_starved =
                                consecutive_positive_starved.saturating_add(1);
                            let sustained = consecutive_positive_starved >= 2;
                            let due = last_alert_starved
                                .map(|t| t.elapsed() >= realert_after)
                                .unwrap_or(true);
                            if sustained && due {
                                error!(
                                    provers,
                                    with_decisions,
                                    "Proving lane starved: zero decisions from populated pool in 24h"
                                );
                                notifier
                                    .notify(NotificationEvent::ProvingLaneStarved {
                                        provers,
                                        with_decisions,
                                    })
                                    .await;
                                last_alert_starved = Some(tokio::time::Instant::now());
                            }
                        } else {
                            if consecutive_positive_starved > 0 {
                                info!("Proving lane starvation cleared");
                            }
                            consecutive_positive_starved = 0;
                            last_alert_starved = None;
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Proving-lane starvation probe failed");
                    }
                }
```

Declare the two new state variables next to `consecutive_positive` / `last_alert`:

```rust
    let mut consecutive_positive_starved: u32 = 0;
    let mut last_alert_starved: Option<tokio::time::Instant> = None;
```

- [ ] **Step 3: Compile + clippy**

```bash
cargo clippy -p chimera_infra 2>&1 | grep -cE "^error"
```
Expected: `0` (fix any dead-code warnings on the new variant's unused fields by keeping the notify arms exhaustive).

- [ ] **Step 4: Commit**

```bash
git add infra/src/monitoring/shadow_gap_alarm.rs infra/src/notifications/mod.rs infra/src/notifications/discord.rs infra/src/notifications/telegram.rs api/src/main.rs
git commit -m "feat(observability): proving-lane starvation alarm — 24h zero-decision probe on populated PROVING pool"
```

(`api/src/main.rs` only if the spawn site needs the import path updated — include it in `git add` only when actually changed.)

---

### Task 3: Scout proving-pool statistics per cycle

**Files:**
- Modify: `scout/core/shadow_promoter.py` (new `proving_pool_stats()` + one INFO line in `run_cycle`)
- Test: `scout/tests/test_shadow_promoter.py`

**Interfaces:**
- Consumes: `execute_and_fetchall` (existing), `rebalance_proving_pool` placement in `run_cycle` (existing).
- Produces: `proving_pool_stats() -> dict` with keys `provers: int, with_evidence: int` and a per-cycle INFO line `proving pool stats: size=N, with_evidence=M` — the operator reads this line to confirm evidence accrual after the cache fix.

- [ ] **Step 1: Write the failing test**

Append to `scout/tests/test_shadow_promoter.py`:

```python
def test_proving_pool_stats_counts_evidence(monkeypatch):
    def fake_fetch(query, params=()):
        if "has_evidence" in query:
            return [{"provers": 30, "with_evidence": 12}]
        raise AssertionError(f"unexpected query: {query}")

    monkeypatch.setattr(sp, "execute_and_fetchall", fake_fetch)
    stats = sp.proving_pool_stats()
    assert stats == {"provers": 30, "with_evidence": 12}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd scout && python3 -m pytest tests/test_shadow_promoter.py::test_proving_pool_stats_counts_evidence -q 2>&1 | tail -2
```
Expected: FAIL — `proving_pool_stats` not defined.

- [ ] **Step 3: Implement**

In `scout/core/shadow_promoter.py`, add before `run_cycle`:

```python
def proving_pool_stats() -> dict:
    """Pool visibility: PROVING size and how many provers hold any shadow
    evidence. Logged every cycle so a starved lane (provers trading but
    zero evidence — the 2026-08-28 cache-starve class) is visible in the
    scout log without touching the DB by hand."""
    rows = execute_and_fetchall(
        """
        SELECT COUNT(*) AS provers,
               COUNT(*) FILTER (WHERE has_evidence) AS with_evidence
        FROM (
            SELECT w.status,
                   EXISTS (
                       SELECT 1 FROM shadow_positions sp
                       WHERE sp.wallet_address = w.address
                   ) AS has_evidence
            FROM wallets w
            WHERE w.status = 'PROVING'
        ) t
        """,
    )
    r = rows[0]
    if isinstance(r, dict):
        return {"provers": int(r["provers"]), "with_evidence": int(r["with_evidence"])}
    return {"provers": int(r[0]), "with_evidence": int(r[1])}
```

In `run_cycle`, directly after the `proving = rebalance_proving_pool()` try/except block (before `promote_perf = ...`):

```python
    try:
        pool_stats = proving_pool_stats()
        logger.info(
            "proving pool stats: size=%d, with_evidence=%d",
            pool_stats["provers"], pool_stats["with_evidence"],
        )
    except Exception as e:  # noqa: BLE001 — stats are advisory visibility.
        logger.warning("proving pool stats failed: %s", e)
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd scout && python3 -m pytest tests/test_shadow_promoter.py -q 2>&1 | tail -2
```
Expected: all pass (previous count + 1).

- [ ] **Step 5: Commit**

```bash
git add scout/core/shadow_promoter.py scout/tests/test_shadow_promoter.py
git commit -m "feat(scout): proving pool stats logged every cycle — evidence-accrual visibility"
```

---

### Task 4: Full verification + deploy

**Files:** none new.

- [ ] **Step 1: Full affected-suite pass**

```bash
docker run -d --name chimera-test-pg -e POSTGRES_PASSWORD=test -p 54329:5432 postgres:16
sleep 4
export TEST_DATABASE_URL="postgresql://postgres:test@localhost:54329/postgres"
cargo test -p chimera_operator --test integration dlq_terminal_rejection_tests -- --test-threads=1
cargo test -p chimera_operator --test unit position_sizer_tests -- --test-threads=1
cargo test -p chimera_operator --test integration webhook_flow -- --test-threads=1
cd scout && python3 -m pytest tests/test_shadow_promoter.py -q; cd ..
cargo clippy -p chimera_operator -p chimera_infra 2>&1 | grep -cE "^error"
docker rm -f chimera-test-pg
```
Expected: all pass, clippy `0`.

- [ ] **Step 2: Push + deploy both images**

```bash
git push origin main
ssh root@chimera-01.moez.tech 'cd /opt/chimera && git pull origin main && nohup sh -c "COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml build operator scout > /tmp/build-rel.log 2>&1; echo BUILD_EXIT=\$? >> /tmp/build-rel.log" >/dev/null 2>&1 & echo started'
# poll: grep BUILD_EXIT /tmp/build-rel.log (expect 0)
ssh root@chimera-01.moez.tech 'cd /opt/chimera && COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml up -d --force-recreate operator scout && sleep 20 && docker ps --filter name=operator --filter name=scout --format "{{.Names}} {{.Status}}"'
```
Expected: both `Up … (healthy)` (scout may show plain `Up`).

- [ ] **Step 3: Post-deploy verification**

```bash
# Next whale-SELL skip should dead-letter TERMINALLY (no sweeper/DLQ churn):
ssh root@chimera-01.moez.tech "docker exec chimera-postgres psql -U chimera -d chimera -c \"SELECT left(error_message,40) err, count(*) FROM trades WHERE status='DEAD_LETTER' AND created_at > NOW() - INTERVAL '1 hour' GROUP BY 1;\""
# Proving evidence should start accruing (cache fix + this deploy):
ssh root@chimera-01.moez.tech "docker logs chimera-scout --since 3h 2>&1 | grep 'proving pool stats' | tail -2"
```
Expected: new dead-letters with `WHALE_SELL_SKIP...` messages; `proving pool stats: size=30, with_evidence=N` with N rising over time.

- [ ] **Step 4: Update the SDD progress ledger**

Append to `.superpowers/sdd/progress.md` under the Shadow-Tiered Proven Sizing section (or a new section `Execution Reliability + Proving-Lane Observability`, started 2026-08-28, base `9f1bdc0`) marking Tasks 1–3 complete with their commit SHAs and Task 4 deployed.
