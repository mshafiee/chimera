# Chimera Agent Guidelines

This file provides build, test, and coding conventions for AI agents working on this codebase.

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
pub type DbPool = Pool<Sqlite>;
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
- **Database:** SQLite with WAL mode. Use `sqlx` (Rust) or `sqlite3` (Python) with prepared statements.
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