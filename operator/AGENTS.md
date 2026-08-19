<!-- Managed by agent: keep sections and order; edit content, not structure. Last updated: 2026-08-19 -->
# operator/ — Chimera Trading Operator (Rust)

Scoped rules for the `chimera_operator` crate. Cross-cutting conventions (git flow,
deployment, versioning, financial precision) live in the root `../AGENTS.md`.

## Overview

The execution hot path: consumes wallet trade signals over an HMAC-authenticated
webhook, validates them (token-safety, circuit breakers), and submits trades —
including Jito bundles — while tracking positions. Crate: `chimera_operator` (Rust 2021).

## Commands (from this dir)

| Command | Purpose |
|---------|---------|
| `cargo build --release` | Release build (`make build-operator`) |
| `cargo test` | Unit tests |
| `cargo test --test '*' -- --test-threads=1` | Integration tests (serial — `make test-integration`) |
| `cargo test --test chaos_tests` | Chaos/resilience tests (`make test-chaos`) |
| `cargo clippy --all-targets --all-features -- -D warnings` | Lint — `-D warnings` is enforced |
| `cargo fmt` | Format |
| `CHIMERA_DEV_MODE=true RUST_LOG=debug cargo run` | Dev mode (`make dev-operator`) |
| `cargo audit --ignore RUSTSEC-2023-0071` | Security audit |

## Project Structure

| Path | Purpose |
|------|---------|
| `src/lib.rs` | Library root |
| `src/engine/` | Execution engine: `executor`, `signal_pipeline`, `stop_loss`, `position_sizer`, `exit_profile`, `profit_targets`, `reconciliation`, `recovery`, `shadow_trader`, `shadow_fill`, `onchain_assessment`, `jito_searcher`, `entry_confirmation`, `worker_pool`, `transaction_builder`, `selection`, `decision_recorder` |
| `src/handlers/` | HTTP handlers: `webhook`, `signals`, `scout`, `health`, `market`, `risk`, `profitability`, `operations`, `monitoring`, `ws`, `api`, `auth`, `webhook_lifecycle` |
| `src/middleware/` | `auth`, `hmac`, `rate_limit` |
| `src/monitoring/` | Helius WebSocket + RPC `polling_task`, `helius_wss` |
| `src/circuit_breaker.rs` | Risk control |
| `src/bin/` | Aux binaries: `generate_jwt_secret`, `import_keypair`, `test_websocket` |
| `src/tools/` | CLI helpers (`generate_jwt_secret`, `import_keypair`) |
| `tests/` | Integration test suite |
| `benches/` | Benchmarks |

## Code Style (extends root Rust rules)

- Financial values: `rust_decimal::Decimal` only. Never `f64` for money.
- Errors: `anyhow::Result` / `AppResult<T>`; map with `.map_err(AppError::from)?`.
- Logging: `tracing` structured events — no `println!/eprintln!`.
- Async: `async fn` on tokio; `Arc` for shared state; `tokio::spawn` for background tasks.
- Imports: `std` → external crates → internal (`crate::`, `chimera_core::`, `chimera_infra::`).

## Boundary

_(see root `AGENTS.md` "Conventions" for cross-cutting rules)_

**Always**
- Verify a token is safe (denylist, liquidity, bonding curve) before any execution path.
- Respect circuit breakers and execution lock before submitting.
- Confirm exits with a live sell quote (trailing-stop exits must not bank phantom profits).
- Run clippy with `-D warnings` and `cargo fmt` before finishing edits.

**Ask first**
- Adding a new execution path, Jito bundle flow, or changing stop-loss/exit semantics.
- Touching `executor.rs` / `signal_pipeline.rs` / `circuit_breaker.rs` — high bug-fix & churn history; check `git` history or root risk notes before editing.

**Never**
- Commit secrets or keypairs (use `infra::vault`/encrypted vault).
- Use floats for any financial quantity.
- Change exit/sizing logic without a matching test.

## Setup & environment
- Rust toolchain (edition 2021) with `cargo`. Env: `CHIMERA_DEV_MODE`, `RUST_LOG`, `.env` (symlinked to `../.env`).

## Security & safety
- Never log or commit keypairs/secrets; key access goes through `infra::vault`.
- Validate every webhook/signal input before execution; HMAC auth + rate limiting enforced in middleware.

## Examples
> Prefer real code in this repo over generic patterns — see the `src/engine/` modules and `tests/` for canonical execution flows.

## When stuck
- Check root `../AGENTS.md` for project-wide conventions.
- Read existing handlers/middleware for the established request→execution pattern.
- `cargo clippy --all-targets --all-features -- -D warnings` surfaces contract issues early.
