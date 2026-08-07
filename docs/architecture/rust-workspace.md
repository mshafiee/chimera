# Rust Workspace Architecture

Enterprise layering for the Chimera Rust crates. The goal is **domain purity**:
core business logic isolated from database drivers and web frameworks, with
compile-time safety and version locks via a single workspace.

## Layout

```text
chimera/
├── Cargo.toml            # Workspace root: [workspace.dependencies] version locks
├── core/                 # Chimera Core — domain-pure foundation (chimera_core)
│   └── src/
│       ├── lib.rs
│       ├── constants.rs  # domain constants (mints, thresholds)
│       └── retry.rs      # framework-agnostic retry/backoff utilities
├── infra/                # Adapters (planned): Postgres backends, API clients
├── api/                  # App entry + HTTP transport (planned): main.rs, handlers
└── operator/             # Legacy facade crate: re-exports core (and future
                          # infra/api) modules so existing chimera_operator::*
                          # paths keep working during incremental extraction
```

## Architectural laws

1. **Domain purity** — `core` never imports database drivers (sqlx, diesel) or
   web frameworks (axum, actix). It hosts entities, value objects, repository
   traits, and framework-agnostic services.
2. **Dependency direction** — `infra` depends on `core`; `core` never depends
   on `infra` or `api`. `api` is the orchestrator: it instantiates infra
   structs, injects them into core services (`Arc<dyn Trait>`), and exposes
   HTTP.
3. **Compile-time safety** — newtypes/enums enforce domain constraints;
   avoid `dyn` unless required at the orchestration boundary.
4. **Feature flags** — expensive/infrastructure dependencies (e.g. axum error
   responses, mock DBs) enter `core` only behind `#[cfg(feature = "...")]`.
5. **Error handling** — `thiserror` for domain errors; `anyhow` only at the
   application entry point (`api/src/main.rs`). No `unwrap()`/`expect()` in
   production paths.

## Incremental extraction pattern (facade)

Each extraction phase moves a module from `operator/` into `core/`/`infra/`/
`api/`, then the operator crate re-exports it:

```rust
// operator/src/lib.rs
pub use chimera_core::{constants, retry};
```

All internal `chimera_operator::*` paths, tests, the Dockerfile build
(`cargo build --release -p chimera_operator`), and the deploy pipeline keep
working — every phase is green and deployable.

## Status

| Phase | Scope | Status |
|---|---|---|
| 1 | Workspace + `[workspace.dependencies]` + `core` (constants, retry) + Dockerfile/compose adaptation | ✅ deployed 2026-08-07 |
| 2 | `core` += `error` (axum impl behind `api` feature flag), `utils` | ✅ 2026-08-07 |
| 3 | `infra` = db_abstraction backend (traits + Postgres + migrations moved as a unit; trait/impl split is follow-up), `core` += `config` (ExecutionLockConfig untied from engine) | ✅ 2026-08-07 |
| 4 | `api` = main.rs + HTTP transport (binary keeps legacy name `chimera_operator`); tools exposed as lib modules + wrapper bins | ✅ 2026-08-07 |
| 5 | `core` += `jupiter` helpers, `price_cache`, `models` (domain entities), `experiment`, `roster`; `infra` += jupiter clients, `token` fetchers, `helius`, `rate_limiter`, `notifications`, `state`, `keypair_utils`, `vault` | ✅ 2026-08-07 |
| 6 | engine split: 13 pure services → `core::engine`, 4 db-backed services (kelly_sizer, momentum_exit, portfolio_heat, tips) → `infra::engine`; 10 monitoring modules → `infra::monitoring` | ✅ 2026-08-07 |
| 7 | remaining cluster (executor, selection, signal_pipeline, transaction_builder, circuit_breaker, handlers, metrics, middleware, remaining monitoring) — needs trait boundaries (core `Executor`/`PriceApi`-style traits, impls in infra) | pending — multi-session |

### Known pragmatic exceptions (documented in code)
- `core` keeps `sqlx`/`config`/`solana-sdk` deps: `AppError` wraps `sqlx::Error`/
  `config::ConfigError`, and config.rs embeds pubkey validation. A future
  phase splits `CoreError` from `InfraError` and moves those variants out.
- `db_abstraction` moved to infra as a unit (traits + impl together); the
  repository-trait/impl split into core is follow-up work.
- The remaining operator cluster (`executor`, `selection`, `signal_pipeline`,
  `transaction_builder`, `circuit_breaker`, `handlers`, `metrics`,
  `middleware`, `helius_wss`, `polling_task`, `rpc_polling`, etc.) is
  mutually entangled — `executor` → circuit_breaker/metrics/notifications/
  vault, `selection` → monitoring, `signal_pipeline` → handlers. Splitting it
  requires defining core traits (e.g. `Executor`, `NotificationSender`,
  `MetricsReporter`) with infra implementations (phase 7).
- `db_abstraction`'s repository traits co-locate with the Postgres impl in
  infra; a trait/impl split into core is part of phase 7.

## Versioning

Versions are locked at `[workspace.package]`; member crates use
`version.workspace = true`. `operator/Cargo.toml` keeps its literal version
for the `scripts/check-version-consistency.sh` parser — extend the script to
read the workspace root when core/ version drift becomes relevant.
