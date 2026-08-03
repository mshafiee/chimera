# Chimera Full-Repo Scan — Final Report

**Date:** 2026-08-02
**Scope:** 714 reviewable files (whole repo, `ocr scan` full-file mode)
**Coverage:** 557 files fully reviewed (2 sessions, merged); 156 files never completed (LLM 402/timeout — still unscanned)
**Findings:** 2,746 unique comments after dedup (latest-session-per-file merge)
**Severity:** 61 critical · 664 high · 1,423 medium · 598 low
**Category:** 1,641 bug · 439 maintainability · 346 test · 151 security · 111 performance · 28 documentation · 5 concurrency

---

### Top Issues

1. **Production secrets hardcoded in committed deploy scripts** — `prod-deploy.sh`, `deploy-production.sh`, `production-deploy.sh`, `production-deploy-simple.sh` contain live Helius RPC keys, `CHIMERA_SECURITY__WEBHOOK_SECRET`, and Jupiter API keys in plaintext. Anyone with repo read access gets working credentials for the live trading system. Also: `production-deploy-simple.sh` builds geoip/haproxy images locally but never pushes them — `docker compose pull` fails on the server.

2. **`prod-deploy.sh` writes a literal `.env`** — the quoted heredoc (`'ENV'`) disables substitution, so the generated file literally contains `$(openssl rand -hex 16)` instead of a generated secret.

3. **Scout core deadlocks on non-reentrant `threading.Lock`** — `scout/core/circuit_breaker.py` (`can_trade_wallet` → `check_circuit_breaker`, `record_trade_result` → `blacklist_wallet`), `scout/core/realtime_profit_tracker.py` (`get_eta_to_1000`, `trigger_optimization_if_needed`, `get_tracker_summary` all re-acquire the lock), `scout/core/strategy_allocator.py` (`update_regime` → `calculate_allocation`). Every such call blocks forever; a deadlocked thread makes the wallet analyzer effectively stop trading decisions.

4. **Operator test/bench suite largely does not compile** — `operator/tests/integration/jito_integration_tests.rs` references nonexistent structs/fields (`TradeConfig`, `Signal` fields, `JitoConfig.default_tip_lamports`, `RpcConfig` fields) and imports private `default_jito_*` fns; `operator/benches/metadata.rs`, `worker_pool.rs`, `write_queue.rs` import nonexistent module paths (`worker::pool`, `queue::write::WriteQueue`). Several `operator/tests/integration_tests.rs` queries use SQLite `?` binds against `Pool<Postgres>` (only `$1` is valid).

5. **Operator engine issues** — `operator/src/engine/signal_pipeline.rs` has a brace imbalance (compile error, file currently modified); `operator/src/state/write_queue.rs` silently drops `UpsertWallet` ops (returns `Ok` without a DB call, while success counters increment) and parks the worker on a 1s idle timeout until shutdown; `operator/src/circuit_breaker.rs` `evaluation_in_progress` never reset on the Tripped→Cooldown path; `operator/src/engine/rent_scavenger.rs` reads `parsed.parsed.get("tokenAmount")` which is always `None` (`jsonParsed` nests under `parsed.info`); `operator/src/monitoring/wallet_performance.rs` self-deadlocks holding the `metrics_cache` write guard across an `await`.

6. **Scout core logic bugs** — `scout/core/helius_client.py` duplicate `elif` conditions make the `usd_spent`/`usd_received` branches unreachable (stablecoin/token swaps report wrong amounts); `helius_client_broken.py` reads unbound `inflow`/`outflow` for real-SOL swaps; `scout/core/dependency_batcher.py` `deps_satisfied` always False so every dependent request reaches the retry path; `scout/core/cost_estimator.py` binds a method instead of a property (`TypeError` on `os.path.exists`); `scout/core/ml_ensemble_deployer.py` overwrites `_active_methods` with an empty set — prediction always runs zero methods; `scout/core/prediction_logger.py` never closes pooled psycopg connections on the exception path (pool slot leak).

7. **Bash scripts abort under `set -euo pipefail`** — `((COUNTER++))` pre-increment returns status 1 on the first call, killing `ops/preflight-check.sh`, `ops/reconcile.sh`, and `test-devnet-comprehensive.sh` before any check runs; `ops/generate-daily-report.sh` uses `//` comments (parsed as a command → exit 126); `ops/backup-verify.sh` uses `local` at top level; `ops/reconcile.sh` loses counters in a pipeline subshell (`| while`).

8. **Broken scout scripts/tests** — `scout/scripts/bench_baseline.py` and `capture_fixtures.py` import `CreditTracker` which doesn't exist (module exports `HeliusCreditTracker`); `scout/scripts/run_validation.py` calls `generate_report(model_type=)` but the function declares `model_types`; `scout/tests/test_credit_tracking.py` uses `patch` without importing it and tests an API the class doesn't have; `scout/tests/fixtures/replay.py` mock never matches the real `get_wallet_transactions` call pattern.

9. **`tools/geoip-lookup.py` route shadowing** — `/geoip/{ip_address}` registered before `/geoip/batch`, so `/geoip/batch` is captured by the dynamic route and fails validation.

---

### Module Hotspots

| Module | Total | Critical | High |
|---|---|---|---|
| `operator/src` | 617 | 6 | 153 |
| `scout/core` | 473 | 13 | 132 |
| `operator/tests` | 296 | 17 | 60 |
| `scout/tests` | 215 | 5 | 40 |
| `web/src` | 121 | 0 | 13 |
| `scout/scripts` | 55 | 3 | 13 |
| `ops/grafana` | 54 | 0 | 7 |
| `operator/migrations_postgres` | 48 | 0 | 8 |

Highest per-file density: `operator/src/handlers/scout.rs` (14), `scout/core/websocket_client.py` (14), `operator/tests/chaos_tests.rs` (14), `test-devnet.sh` (14), `scout/core/helius_client_broken.py` (12), `operator/src/handlers/api.rs` (12), `scout/core/helius_client.py` (11).

---

### Cross-Cutting Concerns

- **Deadlock pattern (17 findings):** non-reentrant `threading.Lock` re-acquisition via helper calls in scout core (circuit_breaker, realtime_profit_tracker, strategy_allocator). A `threading.RLock` or lock-free reads fix all of them.
- **Tests/benches drifting from implementation (≈60 findings):** operator integration tests and benches reference structs, fields, and module paths that no longer exist — the test suite cannot be trusted as a safety net.
- **SQLite→Postgres migration leftovers:** `?` bind placeholders in `operator/tests/integration_tests.rs` (Postgres requires `$1`); `AUTOINCREMENT` DDL in `operator/tests/unit/circuit_breaker_tests.rs`.
- **Hardcoded secrets across the `prod-*.sh` deploy scripts (4 files).**
- **Shell scripting fragility (95 findings matching `set -e`):** increment counters, unquoted expansions, pipeline-subshell state loss in ops/tools/test scripts.
- **Unreachable/dead code (51 findings):** duplicate `elif` guards in helius_client.py, unbound variable paths, dead branches in engine code.

---

### Quick Wins

1. Rotate the leaked production keys (Helius, webhook secret, Jupiter) and replace hardcoded values with `$VAR` reads in `prod-*.sh` — one evening, removes the worst security exposure.
2. Fix the `((COUNTER++))` pre-increment pattern (use `((COUNTER+=1))` or `((++COUNTER))`) in `ops/preflight-check.sh`, `ops/reconcile.sh`, `test-devnet-comprehensive.sh` — the scripts currently die on their first iteration.
3. Fix the signal_pipeline.rs brace imbalance (it is the current uncommitted edit) and the write_queue.rs UpsertWallet drop — both are live data-path bugs.
4. Replace the scout `threading.Lock` instances in the 3 hot-path classes with `threading.RLock` — instant deadlock removal.
5. Fix `scout/tests/test_credit_tracking.py` (missing `patch` import) and the `CreditTracker` imports in 2 scout scripts so CI can actually run them.
6. Correct the operator bench imports (`engine::worker_pool::WorkerPool`, `state::AsyncWriteQueue`) so `cargo bench` compiles.
7. In `prod-deploy.sh` unquote the heredoc delimiter so secrets are actually generated.
8. Re-run the 156 never-completed files (`402 Payment Required` on the old DeepSeek key) to close coverage; most are web/src components, database schemas, and config files.

---

*Methodology: merged `ocr scan` sessions 98a799f8 (completed, DeepSeek) + af1a90a2 (partial, opencode) + b427a225; per file the latest session's comments were kept; findings extracted from `code_comment` tool-call records in the session JSONL. 156 files were never successfully reviewed (LLM 402/timeout) and are not covered here.*
