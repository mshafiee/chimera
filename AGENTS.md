# Chimera Agent Guidelines

This file provides build, test, and coding conventions for AI agents working on this codebase.

## Deployment Workflow

**Git is the source of truth.** The production server pulls from git, builds images, and runs them with docker compose.

```bash
# 1. Make changes locally and commit
git add -A
git commit -m "fix: description of changes"
git push origin main

# 2. On the production server (root@chimera-01.moez.tech)
cd /opt/chimera
git pull origin main
COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml build <service>
COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml up -d --force-recreate <service>
```

**Never scp binaries or files directly to the server.** Always commit to git and pull on the server.

## Build Commands

```bash
# All components
make build                    # Build all (operator + web)
make build-operator          # Rust operator (release)
make build-operator-debug    # Rust operator (debug)
make build-web              # Web dashboard

# Development
make dev                     # Start operator dev mode (RUST_LOG=debug)
make dev-operator           # Same as above
make dev-web                # Web dashboard dev server
```

## Testing

```bash
# Run all tests
make test                    # All tests (operator + scout)
make test-all               # All suites including integration/chaos

# Individual components
make test-operator          # Rust tests only
make test-scout             # Python pytest
make test-integration       # Operator integration tests (--test-threads=1)
make test-chaos             # Resilience tests
make test-e2e               # Web E2E tests (Playwright)

# Single tests
cd operator && cargo test test_name -- --test-threads=1
cd scout && python -m pytest tests/test_file.py::test_name -v
```

## Linting & Formatting

```bash
make lint                   # All linters
make lint-operator          # Clippy (Rust): cargo clippy -- -D warnings
make lint-scout             # Ruff (Python): python -m ruff check .
make lint-web               # ESLint (TypeScript): npm run lint

make fmt                    # Format all
make fmt-operator           # cargo fmt
make fmt-web                # prettier --write "src/**/*.{ts,tsx}"
```

## Code Style

### Rust (Operator)

**Imports:** Group external crates, then internal modules. Use std imports first.
```rust
use std::path::PathBuf;
use anyhow::Result;
use sqlx::Pool;
use crate::config::AppConfig;
use crate::db::DbPool;
```

**Error Handling:** Use `anyhow::Result` for public functions, custom `AppResult` type alias. Map errors with `.map_err(AppError::from)?`. Use `tracing` for structured logging.
```rust
pub async fn init_pool(config: &DatabaseConfig) -> AppResult<DbPool> {
    sqlx::query("SELECT 1")
        .fetch_one(pool)
        .await
        .map_err(AppError::Database)?;
    Ok(pool)
}
```

**Types:** Use `rust_decimal::Decimal` for all financial values. Define type aliases for complex types.
```rust
pub type DbPool = Pool<Postgres>;  // PostgreSQL only — SQLite was decommissioned 2026-07
pub type AppResult<T> = Result<T, AppError>;
```

**Async:** All hot-path functions use `async fn` with tokio runtime. Use `Arc` for shared state.

**Documentation:** Module docs with `//!`, function docs with `///`.

### Python (Scout)

**Imports:** Organize stdlib, external, internal. Use absolute imports within scout.
```python
import asyncio
from decimal import Decimal
from core.analyzer import WalletAnalyzer
from core.wqs import calculate_wqs
```

**Type Hints:** Required for all functions.
```python
async def analyze_wallet(address: str) -> Optional[WalletMetrics]:
    pass
```

**Error Handling:** Try/except with traceback logging. Return `None` on recoverable errors.
```python
try:
    metrics = await analyzer.get_metrics(address)
except Exception as e:
    print(f"ERROR: {e}")
    traceback.print_exc()
    return None
```

**Financial Values:** Use `Decimal` class for precision (see `core/decimal_utils.py`).

**Async:** Use `asyncio` with `async def`. Limit concurrency with `asyncio.Semaphore`.

### TypeScript (Web)

**Imports:** Named imports preferred.
```typescript
import { useState, useEffect } from 'react'
import { useWallet } from '@solana/wallet-adapter-react'
```

**Components:** Functional components with hooks. TypeScript strict mode enabled.
```typescript
interface Props {
  walletAddress: string
  onTrade: (trade: Trade) => void
}

export function TradeCard({ walletAddress, onTrade }: Props) {
  // ...
}
```

**Styling:** TailwindCSS classes. Use `clsx` for conditional classes.

**State:** Zustand for global state, React hooks for local state.

## Conventions

 - **Financial precision:** Never use float/double for money. Use `rust_decimal::Decimal` (Rust) or `Decimal` (Python).
- **Async patterns:** Use `tokio::spawn` for background tasks (Rust), `asyncio.create_task` (Python).
- **Database:** PostgreSQL in production, SQLite in development. Use `sqlx` (Rust) or `psycopg3` (Python) with `%s` placeholders. Never use `?` placeholders (SQLite only).
- **Logging:** Structured logging with `tracing` (Rust) or `print` with prefixes (Python).
- **Tests:** Write unit tests inline, integration tests in `tests/` directory. Use property-based testing (Hypothesis) for Python.
- **Security:** Never commit secrets. Use encrypted vault (`vault.rs`) for keypairs. Validate all inputs.
- **Dependencies:** Check existing codebase before adding new crates/npm packages. Use versions from `Cargo.toml`/`package.json`.

## Versioning & Releases

**Policy:** Unified Semantic Versioning across all components. Single source of truth = `VERSION` file at repo root. See `docs/core/versioning.md` for full policy.

```bash
make version              # Show current version (reads VERSION file)
make version-check        # Verify VERSION matches all manifests (CI-enforced)
make release TYPE=patch   # Bump patch, sync all manifests, generate changelog, commit & tag
make release TYPE=minor   # Bump minor
make release TYPE=major   # Bump major
make changelog            # Show changes since last tag
```

**Key rules:**
- Never edit version in Cargo.toml/package.json/pyproject.toml manually — use `make release`
- `chore(release):` commits auto-generate the tag; push with `git push --follow-tags`
- Safety-critical changes (circuit_breaker, executor, token safety) get a `🛡️ safety:` CHANGELOG marker
- Pre-releases: use `--pre=alpha|beta|rc` (never trade live on alpha/beta)
- Historical version refs in `docs/archive/` and dated runbook entries are preserved as-is

## Scoped AGENTS.md (subsystem index)

**Precedence:** the closest `AGENTS.md` to the file you're editing wins — this root file for cross-cutting rules, then the scoped `AGENTS.md` in the subsystem directory if present. Each subsystem has its own file with commands, structure, style, and boundaries specific to that crate.

| Subsystem | Scope | Stack |
|-----------|-------|-------|
| `operator/AGENTS.md` | Trading hot path: execution, signal pipeline, risk controls | Rust (`chimera_operator`) |
| `core/AGENTS.md` | Shared domain, config, models, price cache | Rust (`chimera_core`) |
| `infra/AGENTS.md` | PostgreSQL, clients, token safety, notifications | Rust (`chimera_infra`) |
| `api/AGENTS.md` | App bootstrap + HTTP/WS backend for the dashboard | Rust (`chimera_api`) |
| `scout/AGENTS.md` | Wallet intelligence cold path: Helius, WQS, backtesting | Python 3.11 |
| `web/AGENTS.md` | Operator dashboard | TypeScript / React 18 + Vite |

<!-- REPOWISE_AGENTS:START — Do not edit below this line. Auto-generated by Repowise. -->
## Codebase Intelligence for chimera (Repowise)

Indexed by [Repowise](https://repowise.dev). Last indexed: 2026-08-18 (commit b08bc6b). Confidence: 99%.
The MCP tools below serve pre-verified docs, symbols, history, and health from that index. Every response carries `_meta` freshness fields; a `stale_warning` appears only when a file the response actually serves changed after indexing, so silence means current.

### How to work in this repo

- **Pre-edit phase** (locate, understand, assess) is where these tools win: `get_answer` for how/where/why, `search_codebase` to find, `get_context` for a file's map, `get_risk` before touching a hotspot.
- **Edit phase**: reading a file before you edit it is correct and expected. Use these tools to decide *which* files to read and edit, not to replace that read.
- **Noisy commands** (tests, builds, `git log`/`diff`, searches, listings): prefer `repowise distill <cmd>`, the same command with its exit code preserved and errors-first compact output. A `[repowise#<ref>: N lines omitted]` marker is fully recoverable via `repowise expand <ref>` (add `-q <regex>` to filter); never re-run the command to see omitted output.

### Trust protocol

- `verified: true` means the served bytes were checked against the live tree. Never follow it with a re-read of the same lines.
- `get_answer` at `confidence: "high"` or `grounding: "extracted"` is content-grounded: cite it directly. `symbol_bodies`, `quotes`, and `code_rationale` entries are live source, so use them instead of opening the file.
- The **only** re-read triggers: `bounds: "approximate"`, `_meta.stale_warning`, `search_method: "bm25"`, `confidence: "low"`. `index_behind: true` alone is informational; the served content is unaffected by the drift.
- Not valid reasons to re-read: "just to be safe", "to see full context" (use the skeleton or a range read), "the file might have changed" (`verified` already checked).
- For exhaustive literal sweeps (rename every call site) plain text search is unbeatable, so use it. Reach for `get_context(include=["callers"])` when you need the `callers_total`/`callers_truncated` honesty signal instead of a maybe-incomplete grep.

### Tools

| Tool | When and why |
|------|--------------|
| `get_answer(question)` | First call for any how / where / why question. `confidence: "high"` or `grounding: "extracted"` is content-grounded — cite it directly. When the question names an indexed symbol, `symbol_bodies` carries its full live body (skip the `get_symbol` follow-up). Low confidence returns `best_guesses` with one-line justifications plus `code_rationale` (rationale comments mined live from candidate source). |
| `get_context(targets=[...])` | Triage card for files/modules/symbols: summary, signatures, `symbol_id`s, `hotspot` bit. File targets auto-serve a `verified` skeleton (every signature at a fraction of a full Read); `mostly_full` marks files where Read costs little more. Batch targets in one call. Opt-in blocks: `include=["callers"|"callees"|"ownership"|"decisions"|"metrics"]`. |
| `get_symbol(id)` | One verified body: `"path.py::Name"` (indexed symbol), `"path.py:140-180"` (live range read), or `"repowise#<hex>"` (omission ref). Source arrives in Read's numbered format — treat it as an already-performed Read. `truncated` responses carry a `continuation` naming the exact next range; ambiguous ids return every match in `candidates`. Index misses fall back to live-grep `fallback_lines`. |
| `search_codebase(query)` | Hybrid search, auto-routed by query shape: identifier → symbol hits (pipe `symbol_id` into `get_symbol`), path → file pages, prose → wiki-semantic. Force with `mode=symbol|path|concept|hybrid`. Concept hits carry a `sources` list; a hit whose sources are `[fts]` only is a keyword match with no semantic agreement — verify it. |
| `get_why(query, targets?)` | Why the code is shaped this way: decision records with evidence and supersession lineage, falling back to git archaeology and `code_rationale` comments. Call before refactors or pattern divergences. |
| `get_risk(targets, changed_files?)` | What history says about touching these files: churn, owners, co-change partners, blast radius. PR mode (`changed_files`) leads with a `directive` block — read `will_break` / `missing_cochanges` / `missing_tests` / `tests_to_run` first. `tests_to_run` is coverage-backed (the tests the per-test map proves exercise the changed files); empty means unknown, never no tests. To score a whole commit or diff range instead, use `get_change_risk`. |
| `get_change_risk(revspec, extensions?, exclude_patterns?)` | Pre-merge defect score for a whole commit or `base..head` range, computed from its diff shape on the live checkout (no index, no LLM). Lead with `risk_percentile` (this change ranked against sampled recent commits), summarized by `review_priority` and `classification`; `score` / `probability` / `level` are the corpus-calibrated fallback. Distinct from `get_risk`, which scores indexed files by path. A `warning` field flags an empty diff (bad revspec or over-tight extension / exclusion filters). |
| `get_health(targets?, include?)` | Health scores + findings on three dimensions (defect / maintainability / performance). Self-check the files you touched before finishing; `include=["biomarkers"|"refactoring"|"signals"]` for depth. |
| `get_dead_code()` | Confidence-tiered unreachable files / unused exports / zombie packages. For cleanup sweeps, not targeted fixes. |
| `get_overview()` | Architecture map + tool recipes. Call once, first, in an unfamiliar repo; skip it after that. |

**Compose them:** low-confidence `get_answer` then read `best_guesses[0].file`; `get_context` shows `hotspot: true` then `get_risk` before editing; `decision_records` titles then `get_why(targets=[...])`; PR review then `get_risk(targets, changed_files)` and read `directive` first. A `tombstone` error means the file moved, so follow `successor_paths`.

### Architecture
Chimera is a high-frequency copy-trading platform for Solana: it consumes wallet trade signals over an HMAC-authenticated webhook, validates and executes them through a Rust hot path (token-safety checks, circuit breakers, Jito bundle submission, position tracking), and continuously refreshes its tracked-wallet roster from a Python cold path that scores candidate wallets with a Wallet Quality Score and backtests them before promotion — with a React dashboard as the monitoring and control surface. It is a hot/cold split: **operator** trades in near-real time on the back of **scout**'s slower, deliberate intelligence. Two strategies run on the same engine: **Shield** (capital preservation, strict stop-losses and liquidity checks) and **Spear** (high-conviction, asymmetric upside via Jito bundles). Operationally it is a single-owner monorepo (~278k LOC, 919 files) with heavy recent churn concentrated in scout's ingestion and analysis path.

### Key modules
- `web/src` — The operator-facing control surface of Chimera is a React single-page application: web/src/main.tsx boots it, web/src/App.tsx mounts its…
- `scout/core` — Scout Core turns on-chain wallet history — fetched through the Helius API — into the intelligence that decides which wallets the platform…
- `operator/src` — Chimera's copy-trading executes in the Operator: wallet trade signals arrive over an HMAC-authenticated webhook, pass through…
- `core/src` — Core is the shared foundation of the Chimera Operator — the configuration, error types, data models, price cache, and Jupiter request…
- `web/src/api` — web/src/api is the dashboard's data-access layer: a set of typed TypeScript client modules that turn the operator backend's HTTP responses…
- `web/src/components/ui` — The UI component library is the shared presentational vocabulary of the Chimera operator console: twelve primitives — buttons, cards…
- `root` — Application Bootstrap is where a running Chimera Operator process comes from: api/src/main.rs lifts configuration out of YAML files and…
- `operator/src/engine` — The execution engine is the operator's hot path from signal to settlement: it admits wallets by on-chain expectancy, turns unified buy/sell…
- `infra/src` — Infra is the concrete-adapters layer that carries every real-world side effect of the Chimera trading loop — the PostgreSQL persistence…
- `infra/src/notifications` — Notification Dispatch is where the Chimera operator's trading hot path turns outward: the circuit-breaker trips, wallet-drain emergencies…

### Entry points
- `scout/main.py`
- `web/src/main.tsx`
- `web/src/App.tsx`
- `api/src/main.rs`
- `infra/src/lib.rs`
- `operator/src/lib.rs`

### Files that need care (bug-fix history first, then churn — check `get_risk` before editing)
- `scout/main.py` — 57 bug fixes, last fix 2 days ago (bug magnet); 50 commits/90d
- `scout/core/helius_client.py` — 49 bug fixes, last fix 7 days ago (bug magnet); 37 commits/90d
- `scout/core/analyzer.py` — 49 bug fixes, last fix 2 weeks ago (bug magnet); 31 commits/90d
- `operator/src/engine/executor.rs` — 44 bug fixes, last fix 13 days ago (bug magnet); 18 commits/90d
- `operator/src/engine/signal_pipeline.rs` — 26 bug fixes, last fix today (bug magnet); 35 commits/90d

### Code health
Three co-equal signals: defect risk 6.51/10 avg, hotspot health 3.18/10 (stable), worst `infra/src/db_abstraction/postgres.rs` at 1.0/10 · maintainability 7.48/10 · performance risk 206 open static I/O-in-loop / N+1 findings. Detail: `get_health()`.

Critical files:
- `operator/src/handlers/profitability.rs` — complex conditional (evaluate_gates) — impact −2.5
- `operator/src/middleware/rate_limit.rs` — nested complexity (extract) — impact −2.5
- `operator/src/engine/mod.rs` — untested hotspot — impact −2.0
- `operator/src/engine/onchain_assessment.rs` — prior defect — impact −2.0
- `operator/src/engine/shadow_trader.rs` — prior defect — impact −2.0

### Standing decisions (ask `get_why` before diverging)
- Consolidate Helius webhook coverage into a deterministic batch script — Per-wallet registration via the health task was quota-blocked and failed repeatedly without producin
- Consolidate Helius webhook coverage via deterministic batch script — The prior per-wallet registration via the health task was blocked by Helius quota limits and produce
- Consolidate Helius webhook registration into a deterministic batch script — Create-then-delete ordering guarantees coverage exists before any cleanup; batching aligns with the 

### Commands
- Build: `make build`
- Test: `make test`
- Lint: `make lint`
- Dev: `make dev`
- Format: `make fmt`

<!-- REPOWISE_AGENTS:END -->
