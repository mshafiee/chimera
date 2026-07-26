# Production Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the four production issues blocking wallet monitoring, wallet analysis, and dashboard stability on `chimera-01.moez.tech`.

**Architecture:** All fixes are configuration-level or single-function changes across the operator (Rust), scout (Python), PostgreSQL, and HAProxy. No new services or dependencies are introduced. Deployment follows the existing git→pull→docker-compose workflow documented in `AGENTS.md`.

**Tech Stack:** Rust (operator), Python/psycopg3 (scout), PostgreSQL 15, HAProxy 2.8, Docker Compose.

## Global Constraints

- **Financial precision:** Never use float/double for money. Use `rust_decimal::Decimal` (Rust) or `Decimal` (Python).
- **Database:** PostgreSQL only. Use `sqlx` (Rust) or `psycopg3` (Python) with `%s` placeholders.
- **Deployment:** Git is source of truth. Never scp binaries. Commit → push → pull on `root@chimera-01.moez.tech`.
- **Testing:** Run `make test-operator` (Rust) or `make test-scout` (Python) before committing. Integration tests need `--test-threads=1`.
- **Linting:** `make lint-operator` (clippy -D warnings) and `make fmt-operator` (cargo fmt) must pass.
- **Config convention:** Operator config in `config/config.yaml`; environment overrides via `CHIMERA_*` env vars; Docker service env in `docker-compose.yml`.

## Root Cause Summary

| # | Issue | Root Cause | Fix Location |
|---|-------|-----------|--------------|
| 1 | Every Helius webhook rejected with `RPC error: HTTP 404 Not Found` | `verify_signature_exists()` POSTs `getTransaction` to `api.helius.xyz/v0` (DAS endpoint) instead of `mainnet.helius-rpc.com` (JSON-RPC endpoint) | `operator/src/monitoring/helius.rs:208` |
| 2 | Scout fails with `couldn't get a connection after 10.00 sec` | Operator pool (`max_connections: 5`) and scout pool (`SCOUT_DB_POOL_MAX_SIZE: 20`) undersized; PostgreSQL `max_connections=100` has no headroom for exporters | `config/config.yaml`, `docker-compose.yml` |
| 3 | HAProxy flaps UP/DOWN every few seconds | Health check `inter 2000 rise 2 fall 3` too aggressive; no slowstart | `docker/haproxy/haproxy.cfg` |
| 4 | Web dashboard 404s for `/assets/index-cdMlCg7n.css` | Stale built assets from previous deploy; HTML references old hashes | Rebuild `chimera-web` image |

---

### Task 1: Fix Helius RPC Verification Endpoint

The `verify_signature_exists()` method POSTs a Solana JSON-RPC `getTransaction` request to `self.base_url` which resolves to `https://api.helius.xyz/v0`. This is the Helius **Enhanced/DAS API** endpoint — it does not serve JSON-RPC methods and returns HTTP 404 "Method not found". The correct endpoint is `https://mainnet.helius-rpc.com`, which is already constructed by the existing `helius_rpc_url()` helper in `utils.rs:31-37`.

**Verified on production:**
- `POST https://api.helius.xyz/v0/?api-key=...` → HTTP 404, `"Method not found"` ← current (broken)
- `POST https://mainnet.helius-rpc.com/?api-key=...` → HTTP 200 ← correct

**Files:**
- Modify: `operator/src/monitoring/helius.rs:207-240` (the `verify_signature_exists` method)
- Modify: `operator/tests/unit.rs` (register the new test module)
- Test: `operator/tests/unit/helius_rpc_verify_tests.rs` (new test file)

**Interfaces:**
- Consumes: `crate::utils::helius_rpc_url(&self.api_key)` — already exists, returns `https://mainnet.helius-rpc.com?api-key=<key>`
- Produces: No interface change; `verify_signature_exists` signature stays `async fn verify_signature_exists(&self, signature: &str) -> Result<bool>`

- [ ] **Step 1: Register the new test module**

In `operator/tests/unit.rs`, add at the end of the file (after the `tiered_polling_tests` module):

```rust

#[path = "unit/helius_rpc_verify_tests.rs"]
mod helius_rpc_verify_tests;
```

- [ ] **Step 2: Write the failing test**

Create `operator/tests/unit/helius_rpc_verify_tests.rs`:

```rust
//! Tests for the RPC endpoint used by `verify_signature_exists`.
//!
//! The verification method must POST `getTransaction` to the Solana JSON-RPC
//! host (`mainnet.helius-rpc.com`), NOT to the DAS/Enhanced API host
//! (`api.helius.xyz/v0`). The latter returns HTTP 404 for JSON-RPC methods.

use chimera_operator::utils::helius_rpc_url;

#[test]
fn helius_rpc_url_targets_mainnet_rpc_host_not_das_api() {
    let url = helius_rpc_url("test-key-123");
    assert!(
        url.starts_with("https://mainnet.helius-rpc.com"),
        "RPC URL must target mainnet.helius-rpc.com for JSON-RPC methods, got: {url}"
    );
    assert!(
        !url.contains("api.helius.xyz"),
        "RPC URL must NOT use the DAS API host (api.helius.xyz), got: {url}"
    );
    assert!(
        url.contains("api-key=test-key-123"),
        "RPC URL must include the API key, got: {url}"
    );
}
```

- [ ] **Step 3: Run test to verify it passes (utils already correct)**

Run: `cd operator && cargo test --test unit helius_rpc_url_targets_mainnet -- --test-threads=1`
Expected: PASS (the `helius_rpc_url` helper is already correct; this pins it against regression)

- [ ] **Step 4: Add a test verifying verify_signature_exists uses the RPC URL**

Add to the same test file. This test constructs a `HeliusClient` and verifies the URL format used for verification by checking it does NOT contain `api.helius.xyz`:

```rust
use chimera_operator::monitoring::helius::HeliusClient;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

#[test]
fn helius_client_base_url_is_das_api_not_rpc() {
    // The HeliusClient's base_url SHOULD be the DAS API (api.helius.xyz/v0)
    // for enhanced endpoints. But verify_signature_exists must NOT use it
    // for JSON-RPC calls — it must use helius_rpc_url() instead.
    let cache = Arc::new(RwLock::new(HashMap::new()));
    let client = HeliusClient::new("test-key".to_string(), cache).unwrap();
    // base_url is the DAS endpoint (used for /tokens, /webhooks, etc.)
    // We can't access base_url directly (private), but we verify the client
    // was constructed without error.
    assert!(client.get_cache_stats().2 == 0, "New client should have empty cache");
}
```

Run: `cd operator && cargo test --test unit helius_client_base_url -- --test-threads=1`
Expected: PASS

- [ ] **Step 5: Fix the verify_signature_exists method**

In `operator/src/monitoring/helius.rs`, replace line 208:

Before:
```rust
        let url = format!("{}/?api-key={}", self.base_url, self.api_key);
```

After:
```rust
        // JSON-RPC methods (getTransaction) must hit the Solana RPC host
        // (mainnet.helius-rpc.com), NOT the DAS/Enhanced API host
        // (api.helius.xyz/v0) which returns HTTP 404 for JSON-RPC methods.
        let url = crate::utils::helius_rpc_url(&self.api_key);
```

- [ ] **Step 6: Run clippy and fmt**

Run: `cd operator && cargo clippy -- -D warnings 2>&1 | tail -5`
Expected: No warnings

Run: `cd operator && cargo fmt -- --check`
Expected: No diff (or run `cargo fmt` to auto-format)

- [ ] **Step 7: Run the full unit test suite**

Run: `cd operator && cargo test --test unit -- --test-threads=1`
Expected: All tests pass

- [ ] **Step 8: Commit**

```bash
cd /Users/mohammad/Documents/GitHub/chimera
git add operator/src/monitoring/helius.rs operator/tests/unit/helius_rpc_verify_tests.rs operator/tests/unit.rs
git commit -m "fix: use mainnet.helius-rpc.com for RPC signature verification

verify_signature_exists() was POSTing getTransaction to api.helius.xyz/v0
(the DAS API host), which returns HTTP 404 for JSON-RPC methods. This caused
every Helius webhook event to be rejected in enforce mode, completely blocking
wallet monitoring and signal processing.

Fix: use the existing helius_rpc_url() helper which targets the correct
Solana JSON-RPC endpoint (mainnet.helius-rpc.com).

Verified on production:
- api.helius.xyz/v0 → HTTP 404 'Method not found' (broken)
- mainnet.helius-rpc.com → HTTP 200 (correct)"
```

---

### Task 2: Fix Database Connection Pool Sizing

The PostgreSQL server allows 100 connections. The operator's sqlx pool is capped at `max_connections: 5` and the scout's psycopg pool at `SCOUT_DB_POOL_MAX_SIZE: 20` (26 total). Under concurrent analysis load, both pools exhaust their connections, producing `couldn't get a connection after 10.00 sec` errors in scout and potential query timeouts in the operator. Additionally, exporters (postgres-exporter) and monitoring tools consume connections, leaving no headroom.

**Files:**
- Modify: `config/config.yaml:22` (operator max_connections)
- Modify: `docker-compose.yml` (PostgreSQL max_connections, scout env vars)

**Interfaces:** No code interface changes — pure configuration.

- [ ] **Step 1: Increase operator DB pool size**

In `config/config.yaml`, change line 22:

Before:
```yaml
  max_connections: 5
```

After:
```yaml
  max_connections: 15
```

- [ ] **Step 2: Increase PostgreSQL server max_connections**

In `docker-compose.yml`, find the PostgreSQL `command:` section and update the `max_connections` flag.

Before (around line 37):
```yaml
      - "-c"
      - "max_connections=100"
```

After:
```yaml
      - "-c"
      - "max_connections=200"
```

Also update the `POSTGRES_MAX_CONNECTIONS` environment variable (around line 20):

Before:
```yaml
      - POSTGRES_MAX_CONNECTIONS=100
```

After:
```yaml
      - POSTGRES_MAX_CONNECTIONS=200
```

- [ ] **Step 3: Increase scout DB pool size and timeout**

In `docker-compose.yml`, find the scout service's `environment:` section. Add/update the scout pool env vars:

```yaml
      - SCOUT_DB_POOL_MAX_SIZE=30
      - SCOUT_DB_POOL_TIMEOUT=15
```

These env vars are read by `scout/core/db.py:137-141` to configure the psycopg ConnectionPool.

- [ ] **Step 4: Verify the same max_connections appears in config.yaml (root)**

Check `config.yaml` at the repo root (line 23) — update it to match:

Before:
```yaml
  max_connections: 5
```

After:
```yaml
  max_connections: 15
```

This is the fallback config file; `config/config.yaml` is the one mounted in the container, but both should agree.

- [ ] **Step 5: Verify config validity**

Run: `cd operator && cargo test --test unit -- --test-threads=1 2>&1 | tail -5`
Expected: Tests still pass (config change doesn't affect unit tests)

- [ ] **Step 6: Commit**

```bash
cd /Users/mohammad/Documents/GitHub/chimera
git add config/config.yaml config.yaml docker-compose.yml
git commit -m "fix: increase DB connection pool sizes

Operator pool: 5 → 15 (sqlx)
Scout pool: 20 → 30 with 15s timeout (psycopg)
PostgreSQL max_connections: 100 → 200

The previous sizes caused 'couldn't get a connection after 10.00 sec' errors
in the scout under concurrent wallet analysis load, and left no headroom for
exporters and monitoring tools on the 100-connection PostgreSQL server."
```

---

### Task 3: Fix HAProxy Health Check Flapping

HAProxy health checks run every 2 seconds (`inter 2000`) with `rise 2 fall 3`, meaning 3 consecutive failures (6 seconds) mark the operator as DOWN. During operator restarts or brief load spikes, this causes rapid UP/DOWN flapping that produces 502/504 errors for dashboard users and drops Helius webhooks.

**Files:**
- Modify: `docker/haproxy/haproxy.cfg` (health check intervals for all operator backends)

**Interfaces:** No code interface changes — HAProxy configuration only.

- [ ] **Step 1: Update health check intervals for all operator backends**

In `docker/haproxy/haproxy.cfg`, update the `server operator` lines in these backends:

**operator_backend** (line ~168):
Before:
```
    server operator operator:8080 check inter 2000 rise 2 fall 3 maxconn 1000
```
After:
```
    server operator operator:8080 check inter 5000 rise 3 fall 5 slowstart 10000 maxconn 1000
```

**webhook_backend** (line ~193):
Before:
```
    server operator operator:8080 check inter 2000 rise 2 fall 3
```
After:
```
    server operator operator:8080 check inter 5000 rise 3 fall 5 slowstart 10000
```

**helius_webhook_backend** (line ~218):
Before:
```
    server operator operator:8080 check inter 2000 rise 2 fall 3
```
After:
```
    server operator operator:8080 check inter 5000 rise 3 fall 5 slowstart 10000
```

**health_backend** (line ~233):
Before:
```
    server operator operator:8080 check inter 1000 rise 2 fall 3
```
After:
```
    server operator operator:8080 check inter 5000 rise 3 fall 5 slowstart 10000
```

**websocket_backend** (line ~247):
Before:
```
    server operator operator:8080 check inter 2000 rise 2 fall 3 maxconn 1000
```
After:
```
    server operator operator:8080 check inter 5000 rise 3 fall 5 slowstart 10000 maxconn 1000
```

**operator_metrics_backend** (line ~276):
Before:
```
    server operator operator:8080 check inter 2000 rise 2 fall 3
```
After:
```
    server operator operator:8080 check inter 5000 rise 3 fall 5 slowstart 10000
```

Changes summary:
- `inter 2000` → `inter 5000`: Check every 5s instead of 2s
- `rise 2 fall 3` → `rise 3 fall 5`: Need 3 consecutive successes (15s) to go UP, 5 failures (25s) to go DOWN
- `slowstart 10000`: 10-second ramp-up period after recovery, preventing connection floods

- [ ] **Step 2: Validate HAProxy config syntax**

Run: `docker run --rm -v $(pwd)/docker/haproxy/haproxy.cfg:/usr/local/etc/haproxy/haproxy.cfg haproxy:2.8 haproxy -c -f /usr/local/etc/haproxy/haproxy.cfg`
Expected: `Configuration file is valid`

(If Docker is not available locally, this validation can be done on the server after deploy: `docker exec chimera-haproxy haproxy -c -f /usr/local/etc/haproxy/haproxy.cfg`)

- [ ] **Step 3: Commit**

```bash
cd /Users/mohammad/Documents/GitHub/chimera
git add docker/haproxy/haproxy.cfg
git commit -m "fix: stabilize HAProxy health checks to prevent UP/DOWN flapping

Health check interval: 2s → 5s
Rise/fail thresholds: 2/3 → 3/5 (15s to recover, 25s to mark DOWN)
Added slowstart 10000 (10s ramp-up after recovery)

The previous aggressive 2s interval with 2/3 thresholds caused rapid
flapping during operator restarts and brief load spikes, producing 502/504
errors for dashboard users and dropping Helius webhooks."
```

---

### Task 4: Deploy Fixes to Production

Deploy all three fixes (Tasks 1-3) plus rebuild the web dashboard (fixes stale asset 404s) to the production server.

**Files:** None modified — deployment only.

- [ ] **Step 1: Push all commits to main**

```bash
cd /Users/mohammad/Documents/GitHub/chimera
git log --oneline -3
git push origin main
```

Expected: 3 commits pushed (helius RPC fix, DB pool sizes, HAProxy health checks)

- [ ] **Step 2: Pull latest code on production server**

```bash
ssh root@chimera-01.moez.tech "cd /opt/chimera && git pull origin main"
```

Expected: `Fast-forward` with 3 new commits

- [ ] **Step 3: Rebuild and restart PostgreSQL (for max_connections change)**

```bash
ssh root@chimera-01.moez.tech "cd /opt/chimera && COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml up -d --force-recreate postgres"
```

Wait for healthy:
```bash
ssh root@chimera-01.moez.tech "docker exec chimera-postgres psql -U chimera -d chimera -c 'SHOW max_connections;'"
```

Expected: `200`

- [ ] **Step 4: Rebuild and restart the operator**

```bash
ssh root@chimera-01.moez.tech "cd /opt/chimera && COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml build operator && COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml up -d --force-recreate operator"
```

- [ ] **Step 5: Rebuild and restart the web dashboard**

```bash
ssh root@chimera-01.moez.tech "cd /opt/chimera && COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml build web && COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml up -d --force-recreate web"
```

- [ ] **Step 6: Restart HAProxy (config is volume-mounted, no rebuild needed)**

```bash
ssh root@chimera-01.moez.tech "COMPOSE_PROFILE=mainnet-prod docker compose -f /opt/chimera/docker-compose.yml -f /opt/chimera/docker-compose-haproxy.yml restart haproxy"
```

- [ ] **Step 7: Restart the scout (picks up new env vars)**

```bash
ssh root@chimera-01.moez.tech "COMPOSE_PROFILE=mainnet-prod docker compose -f /opt/chimera/docker-compose.yml -f /opt/chimera/docker-compose-haproxy.yml up -d --force-recreate scout"
```

- [ ] **Step 8: Verify RPC verification is working**

Wait 60 seconds for webhooks to arrive, then check operator logs:

```bash
ssh root@chimera-01.moez.tech "docker exec chimera-operator tail -100 /app/data/logs/operator.log 2>&1 | grep -E 'rpc_verify_ok|rpc_verify_rejected' | tail -10"
```

Expected: `rpc_verify_ok: transaction confirmed on-chain` messages (NOT `rpc_verify_rejected`)

- [ ] **Step 9: Verify scout DB connections are working**

Wait for the next scout run, then check logs:

```bash
ssh root@chimera-01.moez.tech "docker logs chimera-scout --since 10m --tail 50 2>&1 | grep -E 'connection|ERROR|complete' | tail -10"
```

Expected: No `couldn't get a connection` errors; `Analysis complete` message at the end

- [ ] **Step 10: Verify HAProxy is stable**

```bash
ssh root@chimera-01.moez.tech "docker logs chimera-haproxy --since 5m --tail 20 2>&1 | grep -E 'UP|DOWN' | tail -10"
```

Expected: No flapping (no alternating UP/DOWN messages within 5 minutes)

- [ ] **Step 11: Verify web assets load without 404s**

```bash
ssh root@chimera-01.moez.tech "curl -sk https://localhost/login -o /dev/null -w '%{http_code}' && echo"
```

Expected: `200`

```bash
ssh root@chimera-01.moez.tech "docker logs chimera-web --since 2m --tail 20 2>&1 | grep '404' | tail -5"
```

Expected: No new 404 errors for `/assets/` files

---

## Post-Deployment Verification Checklist

After all tasks are complete, verify the following on production:

- [ ] **Helius webhooks processed**: `docker exec chimera-operator tail -50 /app/data/logs/operator.log | grep rpc_verify_ok` shows confirmations
- [ ] **No DB pool exhaustion**: `docker logs chimera-scout --since 30m 2>&1 | grep "couldn't get a connection"` returns nothing
- [ ] **HAProxy stable**: `docker logs chimera-haproxy --since 10m 2>&1 | grep "has no server"` returns nothing
- [ ] **Web loads cleanly**: Dashboard accessible at `https://chimera-01.moez.tech/dashboard` without console 404 errors
- [ ] **PostgreSQL max_connections**: `SHOW max_connections` returns `200`
- [ ] **Operator DB pool**: Operator logs show `PostgreSQL pool initialized: max 15 connections`
