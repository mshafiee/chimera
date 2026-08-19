<!-- Managed by agent: keep sections and order; edit content, not structure. Last updated: 2026-08-19 -->
# core/ — Chimera Core Shared Library (Rust)

Scoped rules for the `chimera_core` crate. Cross-cutting conventions (git flow,
deployment, versioning, financial precision) live in the root `../AGENTS.md`.

## Overview

Shared foundation consumed by `chimera_operator` and `chimera_infra`: config,
error types, data models, price cache, and Jupiter request handling. No I/O
adapters or side effects live here — that belongs in `../infra`. Crate:
`chimera_core` (workspace versioned).

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
| `src/lib.rs` | Crate root |
| `src/models/` | Domain types: `signal.rs`, `trade.rs` |
| `src/engine/` | Pure engine logic: `signal_quality`, `slippage`, `market_regime`, `mev_protection`, `dex_comparator`, `rpc_cache`, `volume_cache`, `rejection_mute`, `tip_inlining`, `v0_reconstruction`, `degradation`, `run_context`, `channel` |
| `src/experiment/` | Experiment/AB machinery: `controls`, `ledger`, `toxic`, `tracer`, `verdict` |
| `src/config.rs` | Configuration |
| `src/constants.rs` | Shared constants |
| `src/error.rs` | `AppError` types |
| `src/jupiter.rs` | Jupiter request types |
| `src/price_cache.rs` | Price caching |
| `src/retry.rs` | Retry policy |
| `src/roster.rs` | Wallet roster types |
| `src/utils.rs` | Shared utilities |

## Code Style (extends root Rust rules)

- Pure, dependency-light: prefer plain functions/structs over external side effects.
- Financial values: `rust_decimal::Decimal`; never `f64`.
- Errors: `AppError` + `AppResult<T>`; `#[derive(Debug)]` on all models.
- Imports: `std` → external → internal `crate::`.
- Models must be `Serialize`/`Deserialize` (used across operator/infra/api boundaries).
- Add unit tests inline (`#[cfg(test)]`) for any pure logic added here.

## Boundaries

**Always**
- Keep this crate free of I/O; put database/network adapters in `../infra`.
- Place shared domain types here, not in `operator/` or `infra/`, to avoid duplication.

**Ask first**
- Changing `signal.rs` / `trade.rs` models — they are the contract consumed by operator, infra, api, and the dashboard.

**Never**
- Add `tokio::spawn`, HTTP clients, or DB access here.
- Use floats for financial quantities.

## Setup & environment
- Rust toolchain (workspace, edition 2021). No runtime env; it is a pure shared library.

## Security & safety
- Sanitize/validate any inputs before they enter shared models used across crates.

## Examples
> Prefer real code in this repo — `src/models/signal.rs` and `src/engine/model_*` show the canonical type + pure-logic pattern.

## When stuck
- Check root `../AGENTS.md` for cross-cutting conventions.
- Keep logic pure; push side effects to `../infra`.
