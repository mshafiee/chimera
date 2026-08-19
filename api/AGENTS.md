<!-- Managed by agent: keep sections and order; edit content, not structure. Last updated: 2026-08-19 -->
# api/ — Chimera HTTP/WebSocket Backend (Rust)

Scoped rules for the `chimera_api` crate. Cross-cutting conventions (git flow,
deployment, versioning, financial precision) live in the root `../AGENTS.md`.

## Overview

Application bootstrap and the dashboard-facing backend: lifts configuration out of
YAML, wires the operator + infra layers into an HTTP/WebSocket server that the
React dashboard (`../web`) talks to via `../web/src/api/*`. Crate: `chimera_api`
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
| `src/main.rs` | Server bootstrap: config loading, wiring, HTTP/WS server |
| `src/bin/bootstrap_dune.rs` | Aux binary (bootstrap/Dune tooling) |

## Conventions

- This crate is thin: bootstrap + request wiring over `chimera_core` / `chimera_infra`.
  Business logic and persistence live in the crates below, not here.
- Endpoints are consumed by `../web/src/api/client.ts` — keep response shapes in
  sync with the dashboard clients (see `../web/src/api/*`).
- Auth: HMAC-signed requests; rate limiting is applied in `operator` middleware.
- Errors surfaced to the dashboard should be structured, not raw panics.
- Financial values: `rust_decimal::Decimal`; never `f64`.
- Imports: `std` → external → internal `crate::`, `chimera_core::`, `chimera_infra::`,
  `chimera_operator::`.

## Boundaries

**Always**
- Keep endpoint response contracts typed and aligned with `../web/src/api/`.
- Validate/handle errors before returning to the wire.

**Ask first**
- Changing routes or response shapes — the dashboard and webhook consumers depend on them.
- Introducing new I/O or business logic directly in this crate (prefer `core`/`infra`).

**Never**
- Place secrets in config/handlers; load from environment/vault.
- Use floats for financial quantities.

## Setup & environment
- Rust toolchain (workspace, edition 2021). Binary is named `chimera_operator` (keeps the deploy pipeline unchanged); runs via `cargo run`/`make dev`.

## Security & safety
- Endpoints are HMAC-authenticated; keep auth + rate limiting (tower_governor) in front of sensitive routes.
- Sanitize/handle errors before they reach the wire; never leak internals in responses.

## Examples
> Prefer real code in this repo — `src/main.rs` (bootstrap) and the `../web/src/api/` clients that consume these routes.

## When stuck
- Check root `../AGENTS.md` for cross-cutting conventions.
- Keep this crate thin; look in `../core` / `../infra` / `../operator` for logic.
