# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Project Chimera** is a high-frequency, fault-tolerant copy-trading platform for Solana that executes trades based on signals from tracked wallets. It implements a sophisticated **Barbell Strategy** balancing capital preservation (Shield) with asymmetric upside potential (Spear).

**Stack:** Rust (hot path operator), Python 3.11+ (Scout intelligence layer), TypeScript/React (Web dashboard)

**Current Version:** 1.0.0 (see `VERSION` file)

---

## Architecture at a Glance

Chimera uses a **Hot/Cold Architecture**:

### Hot Path: Rust Operator (Real-time Execution)
- **Framework:** Axum (async web server), Tokio runtime
- **Key Modules:**
  - `engine/`: Trade execution engine with state machine
  - `handlers/`: REST API endpoints (webhooks, WebSocket, config)
  - `middleware/`: HMAC authentication, rate limiting, bearer auth
  - `token/`: Token safety checks (honeypots, liquidity validation)
  - `circuit_breaker.rs`: Risk management with automatic trading halts
  - `db.rs`: SQLite (WAL mode) database operations
  - `price_cache.rs`: Token price caching
  - `vault.rs`: Encrypted keypair storage (AES-256)
  - `monitoring/`: RPC health, transaction parsing, signal aggregation
  - `roster.rs`: Wallet roster management and merging

**Goal:** Sub-5ms internal latency for trade execution

### Cold Path: Python Scout (Wallet Intelligence)
- **Key Modules:**
  - `core/analyzer.py`: Fetches wallet transaction history from Solana
  - `core/wqs.py`: Wallet Quality Score (WQS) calculation with temporal penalties
  - `core/backtester.py`: Pre-promotion trade simulation
  - `core/db_writer.py`: Atomic roster writes (roster_new.db)
  - `core/helius_client.py`: Helius RPC integration
  - `core/liquidity.py`: Liquidity provider (Jupiter, DexScreener)
  - `core/validator.py`: Pre-promotion validation criteria

**Goal:** Runs via cron to analyze wallets and prepare updated roster for Operator merge

### Cold Path: Web Dashboard (TypeScript/React)
- **Framework:** Vite, React 18, TailwindCSS
- **Key Features:**
  - Real-time position tracking via WebSocket
  - Wallet management and promotion/demotion
  - Trade history export (CSV/PDF)
  - Configuration management (non-SIGHUP updates)
  - Performance metrics and incident logs

### Infrastructure: HAProxy Reverse Proxy
- **Framework:** HAProxy 2.8+ (Alpine-based)
- **Key Features:**
  - SSL/TLS termination with modern ciphers
  - Geographic access control with city-level precision
  - Rate limiting per IP (webhooks: 100 req/s, Helius: 45 req/s)
  - Path-based routing to different backends
  - Real-time metrics via Prometheus exporter
- **Access Control:**
  - IP range whitelisting/blacklisting
  - Country-based filtering (OFAC compliance)
  - City-level access restrictions
  - ASN-based organization filtering
  - Per-endpoint policy enforcement
- **Security Services:**
  - Security log parser (attack detection, event categorization)
  - Attack detection service (brute force, DDoS, injection patterns)
  - GeoIP lookup service (MaxMind GeoLite2-City integration)
  - Policy manager (REST API for access control management)

---

## Signal Processing Flow

```
External Signal → Webhook (/api/v1/webhook)
  ├─ HMAC verification & replay prevention
  ├─ Rate limiting (100 req/s per IP)
  └─ Queue depth check (< 1000)
    ↓
Signal Validation
  ├─ Token safety checks (honeypot, freeze authority, liquidity)
  ├─ Wallet status (ACTIVE only)
  ├─ Circuit breaker status
  └─ Strategy allocation (Shield vs Spear)
    ↓
Priority Queue (EXIT > SHIELD > SPEAR)
  └─ Load shedding drops SPEAR if queue > 80%
    ↓
Execution Engine
  ├─ Jito Bundle submission (primary)
  ├─ Standard TPU fallback
  └─ Transaction confirmation & state update
    ↓
Position Tracking & WebSocket notification
```

**Trade Status Machine:**
```
PENDING → QUEUED → EXECUTING → ACTIVE → EXITING → CLOSED
                       ↓
                     FAILED → RETRY → EXECUTING
                       ↓
                   DEAD_LETTER (max retries exceeded)
```

### RPC Failover
- **Primary:** Helius (Jito-enabled, sub-50ms latency required)
- **Fallback:** QuickNode/Triton (disables Spear strategy, auto-recovery every 5 min)
- **Requirement:** Servers must be in US-East (Ashburn, VA) or Amsterdam for low latency

---

## Database Schema

**Core Tables:**
- `trades`: All trading signals received (status, PnL, costs, signatures)
- `positions`: Active positions being tracked (unrealized PnL, current price)
- `wallets`: Tracked wallets with WQS scores (managed by Scout, status: ACTIVE/CANDIDATE/REJECTED)
- `dead_letter_queue`: Failed operations for manual review/retry
- `config_audit`: Immutable audit trail of configuration changes
- `admin_wallets`: API access control (roles: admin/operator/readonly)

**Database Features:**
- **WAL Mode:** Write-Ahead Logging for concurrent reads
- **Busy Timeout:** 5 seconds for lock handling
- **Foreign Keys:** Enabled for referential integrity
- **Indexes:** Optimized for common queries (status, wallet, token, created_at)

**Initialize database:**
```bash
make db-init
# Or manually:
mkdir -p data && sqlite3 data/chimera.db < database/schema.sql
```

---

## Build & Development Commands

### Build
```bash
make build                    # Build all components (operator + web)
make build-operator          # Build Rust operator (release)
make build-operator-debug    # Build Rust operator (debug)
make build-web              # Build web dashboard
```

### Development (Hot Reload)
```bash
make dev                     # Start operator in dev mode (RUST_LOG=debug)
make dev-operator           # Same as above
make dev-web                # Start web dashboard with Vite dev server
make dev-scout              # Run Scout manually (--dry-run mode)
```

### Testing
```bash
make test                   # Run all tests (operator + scout)
make test-operator          # Rust tests only (unit + integration)
make test-scout             # Python pytest
make test-integration       # Operator integration tests
make test-load              # k6 load tests (requires k6 installed)
make test-chaos             # Chaos/resilience tests
make test-e2e               # Playwright web E2E tests
```

**Run single test:**
```bash
# Rust
cd operator && cargo test test_name -- --test-threads=1

# Python
cd scout && python -m pytest tests/test_file.py::test_name -v
```

### Linting & Formatting
```bash
make lint                   # Run all linters (clippy, ruff, eslint)
make lint-operator          # Clippy (Rust)
make lint-scout             # Ruff (Python)
make lint-web               # ESLint (TypeScript)

make fmt                    # Format all code (Rust + TypeScript)
make fmt-operator           # cargo fmt
make fmt-web                # prettier
```

### Security Audits
```bash
make audit                  # Run all security audits
make audit-operator         # cargo audit (Rust dependencies)
make audit-web              # npm audit (JavaScript dependencies)
```

### Deployment
```bash
make preflight              # Pre-deployment verification (NTP, RPC latency, circuit breaker)
make deploy                 # Build + preflight + manual deployment steps
make deploy-rsync SERVER=user@host  # Deploy via rsync
make install-service        # Install systemd service & crons
```

### Database
```bash
make db-init                # Initialize schema
make db-migrate             # Run migrations (placeholder for future use)
make db-backup              # Create backup
make db-shell               # Open sqlite3 interactive shell
```

### HAProxy & Access Control
```bash
# Start HAProxy with access control
docker-compose -f docker-compose-haproxy.yml up -d

# Test GeoIP lookup
curl http://localhost:8001/geoip/8.8.8.8

# Test policy evaluation
curl "http://localhost:8001/geoip/evaluate/1.2.4.8?policy_type=strict"

# View current policies
curl http://localhost:8003/policies

# Update IP whitelist
curl -X PUT "http://localhost:8003/policies/whitelist/ips" \
  -H "Content-Type: application/json" \
  -d '["192.168.1.0/24", "10.0.0.0/8"]'

# Update country blacklist
curl -X PUT "http://localhost:8003/policies/blacklist/countries" \
  -H "Content-Type: application/json" \
  -d '["CN", "RU", "KP", "IR"]'

# Reload policies (triggers HAProxy reload)
curl -X POST http://localhost:8003/policies/reload

# View access control metrics
curl http://localhost:8003/metrics | grep chimera_policy
```

### Utilities
```bash
make check-deps             # Verify dependencies (rust, node, python, sqlite3)
make version                # Show version info for all components
make clean                  # Clean build artifacts
make help                   # Show all available commands
```

---

## Configuration

### Environment Setup
1. **Operator `.env`** (in `operator/` directory):
   ```bash
   cd operator && cp config/.env.example .env
   ```
   
   Key variables:
   - `CHIMERA_SECURITY__WEBHOOK_SECRET`: 64-char hex secret (generate: `openssl rand -hex 32`)
   - `CHIMERA_RPC__PRIMARY_URL`: Helius RPC endpoint with API key
   - `CHIMERA_RPC__FALLBACK_URL`: Optional QuickNode/Triton fallback
   - `TELEGRAM_BOT_TOKEN`: For notifications
   - `TELEGRAM_CHAT_ID`: For notifications
   - `DISCORD_WEBHOOK_URL`: For Discord notifications
   - `CHIMERA_DEV_MODE`: Set to 1 for development (skips validation)

2. **Operator `config.yaml`** (in `operator/config/`):
   - Circuit breaker thresholds
   - Shield/Spear allocation percentages
   - Jito tip settings
   - Token safety thresholds
   - RPC rate limits (default: 40 RPS for Helius Developer Plan)

3. **Scout Configuration:**
   - Edit `scout/config.py` or set environment variables
   - Default WQS thresholds: 60.0 (ACTIVE), 20.0 (CANDIDATE)
   - Helius API key required (via env or config)

4. **Access Control Configuration:**
   - **MaxMind License Key:** Required for GeoIP databases
     ```bash
     export MAXMIND_LICENSE_KEY=your_license_key_here
     ```
   - **Policy Directory:** `docker/haproxy/policies/`
     - `config.yaml` - Main policy configuration
     - `whitelists/` - IP, country, city, ASN whitelists
     - `blacklists/` - IP, country, city, ASN blacklists
   - **Default Policy Mode:** Mixed (blacklist + geographic restrictions)
   - **Policy Management API:** `http://localhost:8003/policies`

### Run Configuration
```bash
# Development with debug logging
RUST_LOG=chimera_operator=debug cargo run --release

# Production
RUST_LOG=info cargo run --release

# Override Jupiter simulation (for devnet testing)
CHIMERA_JUPITER__DEVNET_SIMULATION_MODE=true cargo run
```

---

## Key Implementation Details

### Barbell Strategy
- **Shield (Low-risk):** Copies proven Alpha Hunters with strict stop-losses, $10k+ liquidity requirement
- **Spear (High-reward):** High-conviction signals using Jito bundles, $5k+ liquidity, disabled if RPC unstable

### Wallet Quality Score (WQS) v2
- Rescaled to 0-100 range
- Factors: ROI 7d/30d, win rate, profit factor, drawdown, trade consistency
- Temporal consistency penalty: Recent poor performance weighs more
- Pre-promotion backtesting: Validates historical profitability

### Token Safety (Fast/Slow Path)
- **Fast Path:** Cache-based honeypot detection, freeze/mint authority checks
- **Slow Path:** Deep liquidity validation, simulation via Jupiter/DexScreener
- Configurable thresholds by strategy

### Decimals Caching (Jupiter Price API v3)
- **Purpose:** Eliminate Helius RPC calls for token decimals
- **Implementation:** 
  - Jupiter Price API v3 returns decimals alongside price data
  - Decimals cached separately with 24-hour TTL (immutable for minted tokens)
  - Fast path via `PriceCache::get_decimals()` for sub-microsecond lookups
  - Fallback to Helius RPC for tokens not in Jupiter index
- **API Reference:**
  - `PriceCache::get_decimals(token_address)` - Get decimals from cache
  - `TokenMetadataFetcher::get_decimals_only()` - Fast path with RPC fallback
  - `TokenParser::get_token_decimals()` - Updated to use cache first
- **Benefits:**
  - **Helius Credit Savings:** Eliminates `getAccountInfo` calls for decimals
  - **Latency:** ~1μs cache hit vs ~50ms RPC call
  - **Cache Hit Rate:** High for actively traded tokens

### RPC Interaction
- **Primary:** Helius + Jito Bundle submission for prioritization
- **Dynamic Tips:** Percentile-based Jito tip calculation (configurable)
- **Fallback Logic:** Auto-switch to standard TPU on Helius failures (3+ consecutive)
- **Circuit Breaker:** Stops trading on loss thresholds, enabled by SIGHUP or API

### Security
- **HMAC-SHA256:** Webhook signature verification + timestamp replay prevention
- **Encrypted Vault:** AES-256 encrypted keypair storage
- **Secret Rotation:** Automated with grace period support
- **Rate Limiting:** Tower-Governor per-IP rate limiting (100 req/s default)
- **Admin Wallets:** Solana wallet-based authentication for API access
- **Access Control:** Geographic and IP-based access restrictions

### Geographic & IP-Based Access Control
- **Implementation:** HAProxy ACLs with GeoIP city-level precision
- **Capabilities:**
  - IP range whitelisting/blacklisting
  - Country-based access control (e.g., block CN, RU, KP, IR)
  - City-level access restrictions
  - ASN-based organization filtering
  - Per-endpoint policy overrides
- **Policy Management:** REST API (`http://localhost:8003/policies`)
- **Enforcement Modes:** Whitelist, blacklist, mixed, or off
- **Compliance:** OFAC sanctions compliance, GDPR data locality considerations
- **Configuration:** `docker/haproxy/policies/config.yaml`
- **Files:**
  - `docker/haproxy/policies/whitelists/` - IP, country, city, ASN whitelists
  - `docker/haproxy/policies/blacklists/` - IP, country, city, ASN blacklists
  - `docker/haproxy/haproxy.cfg` - HAProxy access control ACLs
- **Services:**
  - `policy-manager` (port 8003) - Policy management API
  - `geoip-lookup` (port 8001) - GeoIP lookup with city precision
  - `geoip-updater` - Automated MaxMind database updates

### State Recovery
- **Stuck Position Detection:** Automatic recovery for positions stuck in EXITING
- **Database Resilience:** WAL mode + SQL-level merges for lock mitigation
- **Trade Reconciliation:** Daily audit comparing DB state vs on-chain state

---

## Important Files & Directory Structure

```
chimera/
├── operator/                  # Rust hot path
│   ├── src/
│   │   ├── main.rs          # Axum server setup, initialization
│   │   ├── engine/          # Execution engine, state machine
│   │   ├── handlers/        # API route handlers
│   │   ├── middleware/      # HMAC, bearer auth, rate limiting
│   │   ├── token/           # Token safety, parser, cache
│   │   ├── monitoring/      # RPC polling, signal aggregation
│   │   ├── db.rs            # Database operations
│   │   ├── circuit_breaker.rs
│   │   ├── config.rs        # Config loading & validation
│   │   └── vault.rs         # Encrypted keypair storage
│   ├── Cargo.toml           # Rust dependencies
│   └── tests/               # Integration tests
│
├── scout/                     # Python cold path
│   ├── core/
│   │   ├── analyzer.py      # Wallet transaction analysis
│   │   ├── wqs.py           # Wallet Quality Score calculation
│   │   ├── backtester.py    # Pre-promotion validation
│   │   ├── db_writer.py     # Atomic roster writes
│   │   ├── helius_client.py # RPC integration
│   │   └── liquidity.py     # Liquidity provider clients
│   ├── main.py              # Scout entry point
│   ├── requirements.txt      # Python dependencies
│   └── tests/               # pytest suite
│
├── web/                       # TypeScript/React dashboard
│   ├── src/
│   │   ├── main.tsx         # React entry
│   │   ├── App.tsx          # Root component
│   │   ├── pages/           # Page components
│   │   ├── components/      # Reusable components
│   │   ├── api/             # API client code
│   │   ├── stores/          # Zustand state management
│   │   └── types/           # TypeScript types
│   ├── package.json         # Node dependencies
│   └── tsconfig.json        # TypeScript config (strict mode enabled)
│
├── database/
│   ├── schema.sql           # Main schema definition
│   ├── migrations/          # Database migrations
│   └── schema/wallets.sql   # Shared wallets schema
│
├── docs/                      # Comprehensive documentation
│   ├── core/                # PDD, architecture, API specs
│   ├── guides/              # User guides
│   ├── operations/          # Deployment, security, monitoring
│   └── development/         # Status, tests, TODOs
│
├── ops/                       # Operational scripts
│   ├── runbooks/            # Incident response procedures
│   ├── prometheus/          # Monitoring & alerting config
│   ├── grafana/             # Dashboard definitions
│   ├── install-crons.sh     # Schedule Scout & reconciliation
│   ├── preflight-check.sh   # Pre-deployment verification
│   ├── reconcile.sh         # Trade reconciliation script
│   └── rotate-secrets.sh    # Secret rotation
│
├── docker/                    # Docker configurations
│   └── haproxy/             # HAProxy reverse proxy
│       ├── haproxy.cfg      # Main HAProxy configuration
│       ├── policies/         # Access control policies
│       │   ├── config.yaml  # Main policy configuration
│       │   ├── whitelists/  # IP, country, city, ASN whitelists
│       │   └── blacklists/  # IP, country, city, ASN blacklists
│       └── geoip/           # MaxMind GeoLite2 databases
│
├── tools/                    # Operational tools and services
│   ├── geoip-lookup.py      # GeoIP lookup service (port 8001)
│   ├── geoip-updater.py     # Automated GeoIP database updates
│   ├── policy-manager.py    # Access control policy manager (port 8003)
│   ├── security-log-parser.py # Security event processing (port 8000)
│   └── attack-detection.py  # Attack pattern detection (port 8002)
│
├── Makefile                 # All build/test/deploy commands
├── README.md                # Main project documentation
└── config.yaml              # Default runtime configuration
```

---

## Testing Strategy

### Unit & Integration Tests (Rust)
Located in `operator/tests/` and embedded in `src/`:
- Token parser validation (honeypot detection, liquidity checks)
- Circuit breaker state transitions
- WQS calculation (via property-based testing with Hypothesis)
- State machine transitions (PENDING → QUEUED → EXECUTING → etc.)
- Database operations (trades, positions, wallets)

**Run with:**
```bash
cd operator && cargo test
cd operator && cargo test --lib      # Unit only
cd operator && cargo test --test '*' # Integration only
```

### Python Tests (Scout)
Located in `scout/tests/`:
- WQS calculation and wallet metrics
- Pre-promotion backtesting
- Liquidity validation
- Helius client integration
- Database schema validation

**Run with:**
```bash
cd scout && python -m pytest tests/ -v
cd scout && python -m pytest tests/test_file.py::test_name -v
```

### Load Testing
- `tests/load/webhook_flood.js`: k6 script simulating 100 req/sec webhook flood
- Validates queue shedding and circuit breaker behavior

```bash
make test-load    # Requires k6 installed
```

### Chaos/Resilience Tests
- RPC failover scenarios
- Mid-trade failures
- Database lock contention
- Position stuck-state recovery

```bash
cd operator && cargo test --test chaos_tests
```

### E2E Tests (Web)
- Dashboard functionality via Playwright
- Wallet promotion/demotion
- Configuration updates
- Trade ledger export

```bash
cd web && npm run test:e2e
```

### Test Coverage
See `docs/development/test-coverage-summary.md` for detailed coverage report.

---

## Key Metrics & Observability

### Prometheus Metrics (exposed at `/metrics`)
- Queue depth, latency percentiles
- Trade execution metrics (success rate, latency)
- Circuit breaker state (trips, resets)
- RPC health (latency, failure rate)
- Reconciliation metrics
- Secret rotation tracking

### WebSocket Notifications
- Real-time position updates (connected via `/api/v1/ws`)
- Circuit breaker triggers
- Wallet promotions/demotions
- Trade exits and PnL updates

### Grafana Dashboard
Import from `ops/grafana/dashboard.json` for visualization.

### Alertmanager Integration
Configure alerts in `ops/prometheus/alerts.yml` for:
- Queue backpressure
- Trade latency spikes
- Circuit breaker triggers
- Reconciliation discrepancies

---

## Important Notes for Development

### RPC Call Optimization
The bot implements several strategies to minimize Helius credit consumption:

1. **Decimals Caching:** Uses Jupiter Price API v3 for decimals (free)
   - Eliminates `getAccountInfo` calls for token decimals
   - 24-hour cache TTL since decimals are immutable
   - Sub-microsecond cache hits vs ~50ms RPC calls

2. **DexScreener for Liquidity:** Uses DexScreener free API instead of on-chain queries
   - `allow_unlisted_heuristic: false` (strict mode)
   - Returns $0 for unlisted tokens rather than expensive pool enumeration
   - No on-chain `PoolEnumerator` queries for liquidity validation

3. **Metadata Caching:** 1-hour TTL for token metadata (freeze/mint authority)
   - Reduces repeated safety checks for the same token
   - `TokenCache` with LRU eviction

4. **Price Caching:** 30-second TTL with 5-second refresh
   - Background updater for actively tracked tokens
   - Staleness detection for risk calculations

### Decimal Precision
- **Financial fields:** Use `rust_decimal::Decimal` in Rust, `Decimal` class in Python
- **Database:** Store as REAL (SQLite), but validate precision in code
- **JSON:** Serialize as strings to preserve precision
- See `scout/core/decimal_utils.py` for utilities

### Transaction Reconstruction
- Operator reconstructs V0 transaction blockhashes for failed transactions
- Uses `solana-client` for signature checking
- See `monitoring/transaction_parser.rs`

### Roster Merging
- Scout writes to `roster_new.db` atomically
- Operator merges via SIGHUP or `/api/v1/roster/merge` endpoint
- Schema validation ensures Rust and Python consistency
- See CI job `schema-validation` in `.github/workflows/ci.yml`

### Admin Wallet Authentication
- Dashboard uses Solana wallet authentication
- Roles: `admin`, `operator`, `readonly`
- Stored in `admin_wallets` table
- See `middleware/auth.rs`

### Development Mode
- `CHIMERA_DEV_MODE=1` skips token safety validation
- Useful for testing with devnet tokens
- Never enabled in production

### Async/Await Patterns
- Operator uses Tokio for all async operations
- Scout uses asyncio for concurrent wallet analysis
- Avoid blocking calls in hot paths

---

## Deployment Checklist

Before production deployment:
1. Run `make preflight` (checks NTP, RPC latency < 50ms, circuit breaker)
2. Verify server location is US-East or Amsterdam
3. Run all tests: `make test-all`
4. Run security audits: `make audit`
5. Review configuration in `config/config.yaml`
6. Backup existing database
7. Deploy via `make deploy-rsync SERVER=user@host`
8. Verify `/api/v1/health` returns OK
9. Monitor logs: `tail -f /var/log/chimera/operator.log`

See `docs/operations/pre-deployment-checklist.md` for complete procedures.

---

## Troubleshooting

**High RPC latency:** Verify server location (must be US-East or Amsterdam). Check network routing.

**Circuit breaker triggered:** Check `/api/v1/health`. Reset via `/api/v1/config/circuit-breaker/reset` (admin only).

**Database locked errors:** Enable WAL mode. Check for long-running transactions. See `ops/runbooks/sqlite_lock.md`.

**Scout not updating roster:** Verify write permissions. Trigger merge via `kill -HUP <operator-pid>` or API call.

**Access Control Issues:**
- **Legitimate traffic blocked:** Check `docker/haproxy/policies/config.yaml` for restrictive policies
- **GeoIP lookups failing:** Verify GeoIP databases exist in `docker/haproxy/geoip/`
- **Policy reload failed:** Check HAProxy configuration syntax with `docker-compose config haproxy`
- **High latency on lookups:** Redis cache may be disabled - check policy manager health
- **Emergency access:** Add IPs to `emergency_whitelist.lst` and reload policies

**Access Control Testing:**
```bash
# Test from specific country (Chinese IP)
curl -H "X-Forwarded-For: 1.2.4.8" http://localhost/api/v1/health

# Test from whitelisted IP
curl -H "X-Forwarded-For: 192.168.1.100" http://localhost/api/v1/health

# Check policy evaluation
curl "http://localhost:8001/geoip/evaluate/8.8.8.8?policy_type=default"

# View audit log
curl http://localhost:8003/audit/log?limit=50
```

See `ops/runbooks/` for detailed incident response procedures.

---

## References

- **Main README:** Project overview, architecture, quick start
- **Architecture Doc:** `docs/core/architecture.md` - Detailed system design
- **API Reference:** `docs/core/api.md` - REST endpoint documentation
- **Testing Guide:** `docs/guides/testing-guide.md`
- **Scout User Guide:** `docs/guides/scout-user-guide.md`
- **Product Design Doc:** `docs/core/pdd.md` - Complete specification
- **Versioning Policy:** `docs/core/versioning.md` - Version rules, release workflow, changelog
- **Runbooks:** `ops/runbooks/` - Incident response procedures
- **Access Control Guide:** `docs/guides/access-control-guide.md` - Geographic and IP-based access control
- **Security Measurement:** `ops/security-measurement-deployment-guide.md` - Security monitoring implementation

---

## Versioning & Releases

Unified Semantic Versioning across all components. Source of truth = `VERSION` file at repo root.

```bash
make version              # Show current version
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
- See `docs/core/versioning.md` for full policy

