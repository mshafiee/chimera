<!-- Managed by agent: keep sections and order; edit content, not structure. Last updated: 2026-08-19 -->
# scout/ — Chimera Wallet Intelligence Layer (Python)

Scoped rules for `chimera-scout`. Cross-cutting conventions (git flow, deployment,
versioning, financial precision) live in the root `../AGENTS.md`.

## Overview

The cold path that refreshes the tracked-wallet roster: fetches on-chain wallet
history via Helius, scores candidate wallets with a Wallet Quality Score (WQS),
backtests, and promotes wallets into the signal roster. Influences but does not
execute trades — execution is `../operator`. Python >= 3.11.

## Commands (from this dir)

| Command | Purpose |
|---------|---------|
| `python3 -m pytest tests/ -v` | Run all tests (`make test-scout`); `asyncio_mode = auto` |
| `python3 -m pytest tests/test_file.py::test_name -v` | Single test |
| `python3 -m ruff check .` | Lint (line-length 120; ignores E501, W293, I001) |
| `python3 main.py --dry-run` | Dry-run of the scout loop (`make dev-scout`) |
| `python3 scripts/validate_wqs_predictiveness.py` | Validate WQS predictiveness |
| `python3 scripts/backfill_historical_correlation.py` | Backfill PnL correlation |
| `python3 -m scout.scripts.run_validation --db-path ../data/chimera.db --time-window 7d` | ML validation pipeline |
| `python3 -m pytest tests/test_prediction_validation.py -v` | Prediction-validation tests |

## Project Structure

| Path | Purpose |
|------|---------|
| `main.py` | Scout loop entry point |
| `config.py` | Configuration |
| `core/` | Intelligence: `analyzer`, `optimized_analyzer`, `helius_client`, `laserstream_client`, `birdeye_client`, `websocket_client`, `wqs`, `backtester`, `validator`, `feature_store`, `feature_enrichment`, `signal_quality_filter`, `market_regime_detector`, `circuit_breaker`, `position_manager`, `position_sizer`, `stop_loss_optimizer`, `strategy_allocator`, `clustering`, `prediction_matcher`, `prediction_logger`, `model_registry`, `wqs_comparison`, `smart_discovery`, `webhook_discovery`, `roster_writer_db`, `db`, `decimal_utils` |
| `tests/` | 87 pytest files, `conftest.py`, `fixtures/` |
| `scripts/` | Validation/training/backfill CLI tools |
| `analysis/` | Diagnostic/metrics analysis |
| `integrations/` | Cross-layer integrations |
| `config/` | Wallet/seed lists (`wallets.txt`, `seed_wallets.txt`, `active_tokens.txt`) |

## Code Style (extends root Python rules)

- Imports: stdlib → external → internal `core.*`. Absolute imports within scout.
- Type hints required on all function signatures.
- Financial values: `Decimal` only — use `core/decimal_utils.py`, never `float` for money.
- Async: `async def` + `asyncio`; bound concurrency with `asyncio.Semaphore`.
- Logging: `print` with prefixes (e.g. `ERROR:`), traceback on exceptions; return
  `None` on recoverable errors.
- DB: `psycopg3` with `%s` placeholders (never SQLite `?`).
- Tests: property-based testing with Hypothesis; inline unit tests + `tests/` integration.

## Boundaries

**Always**
- Route wallet/coin data through `core/helius_client.py`; respect Helius quota/credit tracking.
- Use `Decimal` for every financial value and WQS/cost computation.
- Keep analyst thresholds/config data-driven (`config.py`) rather than hard-coded.

**Ask first**
- Changing scoring/backtest/promotion logic — it directly changes which wallets get supported.
- Editing `core/analyzer.py`, `core/helius_client.py`, or `main.py` — bug magnets with heavy churn; consult root risk notes / `git` history first.

**Never**
- Use floats for financial quantities or WQS scores.
- Register Helius webhooks ad hoc per wallet — consolidate into the deterministic batch script.
- Commit secrets/API keys (in `.env`, not tracked).

## Setup & environment
- Python >= 3.11. Dependencies from `requirements.txt` (runtime) / `requirements-dev.txt` (dev: pytest, ruff). Env via `.env` — never commit it.

## Security & safety
- API keys (Helius, Birdeye, etc.) come from env only; never log or commit.
- Validate/denylist inputs; route chain data through `core/helius_client.py` and honor credits/quota.
- Use `psycopg3` `%s`-parameterized queries — no SQL string interpolation.

## Examples
> Prefer real code in this repo — `core/analyzer.py`, `core/wqs.py`, and `tests/` show the canonical scoring/test pattern.

## When stuck
- Check root `../AGENTS.md` for cross-cutting conventions.
- Read `core/decimal_utils.py` before any money math; run `python3 -m ruff check .` early.
