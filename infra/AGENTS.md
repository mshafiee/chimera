<!-- Managed by agent: keep sections and order; edit content, not structure. Last updated: 2026-08-19 -->
# infra/ — Chimera Infra Adapters (Rust)

Scoped rules for the `chimera_infra` crate. Cross-cutting conventions (git flow,
deployment, versioning, financial precision) live in the root `../AGENTS.md`.

## Overview

Concrete-adapters layer carrying every real-world side effect of the trading loop:
PostgreSQL persistence, Helius/Jupiter/dexscreener clients, token safety, state
coordinator, and notification dispatch (Discord/Telegram). Crate: `chimera_infra`
(workspace versioned).

## Commands (from this dir)

| Command | Purpose |
|---------|---------|
| `cargo build` | Build |
| `cargo test` | Unit tests |
| `cargo clippy --all-targets --all-features -- -D warnings` | Lint — `-D warnings` enforced |
| `cargo fmt` | Format |

## Project Structure

| Path | Purpose |
|------|---------|
| `src/db_abstraction/` | PostgreSQL persistence: `postgres.rs`, `types.rs`, `export.rs` |
| `src/state/` | State coordination: `coordinator`, `registry`, `write_queue` |
| `src/token/` | Token safety: `bonding_curve`, `metadata`, `parser`, `pools`, `cache` |
| `src/monitoring/` | Helius WSS, `dexscreener`, `exit_detector`, `transaction_parser`, `rate_limiter`, `webhook_health_task`, `webhook_lifecycle`, `wallet_performance`, `pre_validator`, `signal_aggregator`, `nav_snapshot` |
| `src/engine/` | `kelly_sizer`, `momentum_exit`, `portfolio_heat`, `tips` |
| `src/notifications/` | `discord.rs`, `telegram.rs` |
| `src/jupiter_http_client.rs` | Jupiter API adapter |
| `src/keypair_utils.rs` / `src/vault.rs` | Key management |
| `src/lib.rs` | Crate root |
| `migrations_postgres/` | SQL migrations |

## Code Style (extends root Rust rules)

- Database: PostgreSQL only (SQLite was decommissioned 2026-07). `sqlx` with
  `%s`/`$n` placeholders — never SQLite `?` placeholder syntax.
  `pub type DbPool = Pool<Postgres>`.
- Financial values: `rust_decimal::Decimal`; never `f64`.
- Errors: `anyhow::Result` / `AppResult<T>`; map external errors with `.map_err(AppError::...)?`.
- Logging: `tracing` structured events.
- Async: `async fn` on tokio; `Arc` for shared state.
- Keypairs: never persisted/logged in plaintext — always via `vault.rs` (encrypted).

## Boundaries

**Always**
- Route all DB access through `db_abstraction/`; do not hand-roll SQL in callers.
- Validate all external inputs before they reach side effects.
- Use `tracing` for any failure event that operators act on.

**Ask first**
- Adding a new external-client adapter or webhook subscription (Helius quota-sensitive).
- Changing `postgres.rs` or the shared schema — broad blast radius across operator/api.

**Never**
- Write secrets or keypairs to logs, config, or git.
- Use floats for financial quantities.
- Block the trading hot path behind a slow synchronous client without a rate limiter.

## Setup & environment
- Rust toolchain (workspace, edition 2021). PostgreSQL required for `db_abstraction`; external client adapters need their API keys via env.

## Security & safety
- All secrets through env/vault — never in code, logs, or git.
- Validate external responses before side effects; hold hot-path consumers behind rate limiters.

## Examples
> Prefer real code in this repo — `src/db_abstraction/postgres.rs` and `src/notifications/*` show the adapter pattern.

## When stuck
- Check root `../AGENTS.md` for cross-cutting conventions.
- Keep pure logic in `../core`; this crate only carries real side effects.
