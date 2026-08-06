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
| 2 | `core` += `error` (axum impl behind feature flag), `utils` | pending |
| 3 | `core` += `db_abstraction` repository traits (sqlx leak removed), `infra` = Postgres backend + API clients (Jupiter/Helius/Dune) | pending |
| 4 | `api` = main.rs + handlers + middleware; operator becomes thin facade | pending |
| 5 | `core` += `config` (split engine sub-configs), `price_cache` (client extraction), pure engine services | pending |

## Versioning

Versions are locked at `[workspace.package]`; member crates use
`version.workspace = true`. `operator/Cargo.toml` keeps its literal version
for the `scripts/check-version-consistency.sh` parser — extend the script to
read the workspace root when core/ version drift becomes relevant.
