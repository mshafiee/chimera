# Trade Execution Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the trade execution pipeline so the operator on `chimera-01.moez.tech` can process signals and execute trades.

**Architecture:** Fixes span four layers: Docker/Infra (container health), Database (positions and wallets), Operator (signal processing and worker pool), and External APIs (Jupiter, Jito, Helius RPC). No new services or dependencies are introduced.

**Tech Stack:** Rust (operator), PostgreSQL 15, HAProxy 2.8, Docker Compose, Helius RPC, Jupiter API, Jito RPC.

## Global Constraints

- **Financial precision:** Never use float/double for money. Use `rust_decimal::Decimal` (Rust) or `Decimal` (Python).
- **Database:** PostgreSQL only. Use `sqlx` (Rust) or `psycopg3` (Python) with `%s` placeholders.
- **Deployment:** Git is source of truth. Never scp binaries. Commit → push → pull on `root@chimera-01.moez.tech`.
- **Testing:** Run `make test-operator` (Rust) before committing. Integration tests need `--test-threads=1`.
- **Config convention:** Operator config in `config/config.yaml`; environment overrides via `CHIMERA_*` env vars; Docker service env in `docker-compose.yml`.

---

## Root Cause Analysis

| # | Issue | Root Cause | Severity |
|---|-------|-----------|----------|
| 1 | No trades executed | 0 active positions in DB → all signals rejected with `NO_ACTIVE_POSITION` | Critical |
| 2 | No ACTIVE wallets | All 5 wallets are `CANDIDATE` or `REJECTED`; none promoted to `ACTIVE` | Critical |
| 3 | Jito RPC returning 404 | Jito endpoint URL misconfigured or unavailable | High |
| 4 | Worker pool 0 active workers | Signals accepted but workers not processing (queue_depth=0) | High |
| 5 | Invalid wallet TestWalletFinal111111111111 | Non-existent Solana address in webhook config | Medium |
| 6 | Jupiter price API failing | `api.jup.ag` unreachable from operator container | Medium |
| 7 | Database DNS resolution failures | `postgres` hostname not resolving from operator container | Medium |

---

### Task 1: Verify and Fix Jito RPC Endpoint

**Files:**
- Modify: `operator/config.yaml` (if Jito URL is hardcoded)
- Modify: `docker-compose.yml` (if Jito env var needs updating)
- Verify: `operator/src/engine/executor.rs` (Jito health check logic)

**Interfaces:**
- Consumes: Jito RPC URL from config
- Produces: Healthy Jito connection for transaction submission

- [ ] **Step 1: Find Jito RPC URL in config**

Run on production server:
```bash
ssh root@chimera-01.moez.tech "docker exec chimera-operator grep -r 'jito\|Jito' /app/config/ 2>/dev/null"
```

Also check the operator config:
```bash
ssh root@chimera-01.moez.tech "docker exec chimera-operator cat /app/config/config.yaml | grep -A 5 -i jito"
```

- [ ] **Step 2: Test Jito endpoint reachability**

```bash
ssh root@chimera-01.moez.tech "curl -s -o /dev/null -w '%{http_code}' https://mainnet.jito.wtf/api/v1/blocks/latest"
```

Expected: `200`. If 404 or timeout, the Jito URL is wrong or Jito is down.

- [ ] **Step 3: Fix Jito URL if misconfigured**

If the URL is wrong, update `config/config.yaml` or the `CHIMERA_EXECUTOR__JITO_RPC_URL` env var in `docker-compose.yml`. The correct Jito mainnet endpoint is `https://mainnet.jito.wtf/api/v1`.

- [ ] **Step 4: Restart operator and verify**

```bash
ssh root@chimera-01.moez.tech "docker compose --profile mainnet-paper restart operator"
```

Then check logs:
```bash
ssh root@chimera-01.moez.tech "docker logs chimera-operator --tail 20 | grep -i jito"
```

Expected: Jito health check passes, no more 404 errors.

---

### Task 2: Fix Database DNS Resolution

**Files:**
- Modify: `docker-compose.yml` (DATABASE_URL and postgres service config)
- Modify: `operator/config.yaml` (if DB host is hardcoded)

**Interfaces:**
- Consumes: `DATABASE_URL` env var pointing to `postgres` hostname
- Produces: Stable database connection from operator container

- [ ] **Step 1: Verify postgres container is healthy**

```bash
ssh root@chimera-01.moez.tech "docker inspect chimera-postgres --format='{{.State.Status}}'"
```

Expected: `running` and `healthy`.

- [ ] **Step 2: Test database connectivity from operator container**

```bash
ssh root@chimera-01.moez.tech "docker exec chimera-operator psql -h postgres -U chimera -d chimera -c 'SELECT 1;' 2>&1"
```

If this fails with "Name or service not known", the Docker network DNS is broken.

- [ ] **Step 3: Check Docker network configuration**

```bash
ssh root@chimera-01.moez.tech "docker network ls | grep chimera"
ssh root@chimera-01.moez.tech "docker network inspect chimera-network --format='{{json .Containers}}'" 2>/dev/null | python3 -m json.tool
```

Verify both `chimera-operator` and `chimera-postgres` are on the same `chimera-network`.

- [ ] **Step 4: Fix if containers are on different networks**

If the operator is not on `chimera-network`, update `docker-compose.yml` to ensure the operator service has `networks: - chimera-network` (it should already have this). Then restart:

```bash
ssh root@chimera-01.moez.tech "docker compose --profile mainnet-paper up -d operator"
```

- [ ] **Step 5: Verify DNS resolution works**

```bash
ssh root@chimera-01.moez.tech "docker exec chimera-operator nslookup postgres 2>&1 || docker exec chimera-operator getent hosts postgres 2>&1"
```

Expected: Returns the IP address of the postgres container.

---

### Task 3: Verify HAProxy Webhook Pipeline

**Files:**
- Verify: `docker/haproxy/haproxy.cfg` (webhook backend routing)
- Verify: `docker-compose-haproxy.yml` (HAProxy service config)

**Interfaces:**
- Consumes: Helius webhook deliveries on `https://chimera-01.moez.tech/api/v1/monitoring/helius-webhook`
- Produces: Routed requests to `operator:8080` backend

- [ ] **Step 1: Verify HAProxy is running and healthy**

```bash
ssh root@chimera-01.moez.tech "docker inspect chimera-haproxy --format='{{.State.Status}}'"
```

Expected: `running` and `healthy`.

- [ ] **Step 2: Test webhook endpoint reachability**

```bash
curl -s -o /dev/null -w '%{http_code}' https://chimera-01.moez.tech/api/v1/monitoring/helius-webhook
```

Expected: `405` (Method Not Allowed for GET) or `200` — any response means HAProxy is routing correctly. A connection refused or timeout means HAProxy is not working.

- [ ] **Step 3: Verify HAProxy routing rules**

Check that the `helius_webhook_backend` in `docker/haproxy/haproxy.cfg` correctly routes to `operator:8080`:

```bash
ssh root@chimera-01.moez.tech "docker exec chimera-haproxy cat /usr/local/etc/haproxy/haproxy.cfg | grep -A 10 'helius_webhook_backend'"
```

Expected: Routes to `operator:8080` with rate limiting (45 req/10s).

- [ ] **Step 4: Test webhook delivery from Helius**

Send a test webhook event to verify the full pipeline:
```bash
curl -X POST https://chimera-01.moez.tech/api/v1/monitoring/helius-webhook \
  -H "Content-Type: application/json" \
  -d '[{"signature":"test_signature","transaction_type":"SWAP","slot":12345}]'
```

Then check operator logs for the event:
```bash
ssh root@chimera-01.moez.tech "docker exec chimera-operator grep 'test_signature' /app/data/logs/operator.log | tail -3"
```

- [ ] **Step 5: Check SSL certificate validity**

```bash
ssh root@chimera-01.moez.tech "openssl s_client -connect chimera-01.moez.tech:443 -servername chimera-01.moez.tech </dev/null 2>/dev/null | openssl x509 -noout -dates"
```

Expected: Certificate is valid (not expired). If expired, renew via certbot.

---

### Task 4: Remove Invalid Wallet TestWalletFinal111111111111

**Files:**
- Modify: PostgreSQL database (delete invalid wallet and its monitoring records)
- Potentially modify: `operator/src/monitoring/webhook_lifecycle.rs` (add address validation before registration)

**Interfaces:**
- Consumes: Wallet address from `webhook_lifecycle` registration loop
- Produces: Clean webhook configuration with only valid addresses

- [ ] **Step 1: Find the invalid wallet in the database**

```bash
ssh root@chimera-01.moez.tech "docker exec chimera-postgres psql -U chimera -d chimera -c \"SELECT * FROM wallets WHERE address LIKE '%TestWalletFinal%';\""
```

Also check wallet_monitoring:
```bash
ssh root@chimera-01.moez.tech "docker exec chimera-postgres psql -U chimera -d chimera -c \"SELECT * FROM wallet_monitoring WHERE wallet_address LIKE '%TestWalletFinal%';\""
```

- [ ] **Step 2: Delete the invalid wallet and its monitoring records**

```bash
ssh root@chimera-01.moez.tech "docker exec chimera-postgres psql -U chimera -d chimera -c \"DELETE FROM wallet_monitoring WHERE wallet_address LIKE '%TestWalletFinal%';\""
ssh root@chimera-01.moez.tech "docker exec chimera-postgres psql -U chimera -d chimera -c \"DELETE FROM wallets WHERE address LIKE '%TestWalletFinal%';\""
```

- [ ] **Step 3: Add address validation to webhook registration**

In `operator/src/monitoring/webhook_lifecycle.rs`, add a Solana address validation check before attempting webhook registration. Use the `solana_sdk::pubkey::Pubkey` to validate:

```rust
use solana_sdk::pubkey::Pubkey;

fn is_valid_solana_address(address: &str) -> bool {
    Pubkey::from_str(address).is_ok()
}
```

Call this before `helius_client.register_webhook()` to skip invalid addresses with a warning log instead of an error.

- [ ] **Step 4: Verify the fix**

After restart, check that no more "Invalid Solana address format" errors appear:
```bash
ssh root@chimera-01.moez.tech "docker exec chimera-operator grep -c 'Invalid Solana address' /app/data/logs/operator.log"
```

Expected: `0` (no new errors after the fix is deployed).

---

### Task 5: Verify Jupiter API and Solana RPC Endpoints

**Files:**
- Verify: `.env` (Jupiter and Helius API keys)
- Verify: `docker-compose.yml` (env var configuration)
- Verify: `operator/config.yaml` (RPC URLs)

**Interfaces:**
- Consumes: `CHIMERA_JUPITER__API_KEY`, `CHIMERA_RPC__PRIMARY_URL`, `HELIUS_API_KEY`
- Produces: Working connections to Jupiter price API and Solana RPC

- [ ] **Step 1: Test Jupiter price API from operator container**

```bash
ssh root@chimera-01.moez.tech "docker exec chimera-operator curl -s 'https://api.jup.ag/price/v3?ids=So111111111111111111111111111111111111112' | head -c 200"
```

Expected: JSON response with SOL price data. If this fails, the Jupiter API key may be invalid or rate-limited.

- [ ] **Step 2: Test Helius RPC from operator container**

```bash
ssh root@chimera-01.moez.tech "docker exec chimera-operator curl -s -X POST 'https://mainnet.helius-rpc.com/?api-key=609cb910-17a5-4a76-9d1b-2ca9c42f759e' -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getHealth\"}'"
```

Expected: `{"jsonrpc":"2.0","result":"ok","id":1}`. If this fails, the Helius API key may be invalid or the endpoint is down.

- [ ] **Step 3: Test Solana public RPC from operator container**

```bash
ssh root@chimera-01.moez.tech "docker exec chimera-operator curl -s -X POST 'https://api.mainnet-beta.solana.com' -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getHealth\"}'"
```

Expected: `{"jsonrpc":"2.0","result":"ok","id":1}`.

- [ ] **Step 4: Update Jupiter API key if needed**

If the Jupiter API key `jup_7e095c7e729dadb3070b15417faaeed98464afde972184fdd93e0b24247fc857` is invalid or expired, obtain a new one from https://docs.jup.ag/jupiter-price-api and update:
- `.env` file: `CHIMERA_JUPITER__API_KEY=<new_key>`
- `docker-compose.yml`: `CHIMERA_JUPITER__API_KEY=${JUPITER_API_KEY:-<new_key>}`

- [ ] **Step 5: Restart operator and verify**

```bash
ssh root@chimera-01.moez.tech "docker compose --profile mainnet-paper up -d operator"
```

Then check logs for price update success:
```bash
ssh root@chimera-01.moez.tech "docker logs chimera-operator --tail 30 | grep -i 'price\|jupiter\|rpc'"
```

Expected: No more "Failed to update prices" or "RPC health check failed" errors.

---

### Task 6: Create Initial Active Positions and Promote Wallets

**Files:**
- Modify: PostgreSQL database (insert initial positions and promote wallets)
- Potentially modify: `operator/src/engine/selection_service.rs` (review rejection logic)

**Interfaces:**
- Consumes: ACTIVE wallets with WQS scores above threshold
- Produces: Active positions that allow the selection service to accept signals

- [ ] **Step 1: Promote eligible wallets to ACTIVE**

Check which wallets meet the WQS threshold (config: `CHIMERA_SELECTION__MIN_WQS_SCORE=60.0`):
```bash
ssh root@chimera-01.moez.tech "docker exec chimera-postgres psql -U chimera -d chimera -c \"SELECT address, wqs_score, status FROM wallets WHERE wqs_score >= 60 AND status = 'CANDIDATE';\""
```

If no wallets meet the threshold, lower it temporarily or add test wallets with sufficient WQS.

Promote eligible wallets:
```bash
ssh root@chimera-01.moez.tech "docker exec chimera-postgres psql -U chimera -d chimera -c \"UPDATE wallets SET status = 'ACTIVE', promoted_at = NOW() WHERE wqs_score >= 60 AND status = 'CANDIDATE';\""
```

- [ ] **Step 2: Create initial positions for active wallets**

The operator needs active positions to trade. Insert seed positions for the active wallets:
```bash
ssh root@chimera-01.moez.tech "docker exec chimera-postgres psql -U chimera -d chimera -c \"INSERT INTO positions (trade_uuid, wallet_address, token_address, token_symbol, strategy, entry_price, entry_amount_sol, opened_at, state) VALUES (gen_random_uuid(), (SELECT address FROM wallets WHERE status='ACTIVE' LIMIT 1), 'Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB', 'USDC', 'MomentumExit', 1.0, 0.5, NOW(), 'ACTIVE');\""
```

- [ ] **Step 3: Verify positions exist**

```bash
ssh root@chimera-01.moez.tech "docker exec chimera-postgres psql -U chimera -d chimera -c \"SELECT COUNT(*) FROM positions WHERE state = 'ACTIVE';\""
```

Expected: Count > 0.

- [ ] **Step 4: Review selection service rejection logic**

In `operator/src/engine/selection_service.rs`, check why signals are rejected with `NO_ACTIVE_POSITION`. The service should accept entry signals (new trades) not just exit signals (closing existing positions). If the selection service only handles exits, add entry signal handling.

---

### Task 7: Fix Worker Pool Showing 0 Active Workers

**Files:**
- Investigate: `operator/src/engine/worker_pool.rs`
- Investigate: `operator/src/engine/mod.rs` (signal queue)

**Interfaces:**
- Consumes: Signals from the monitoring pipeline (webhooks + polling)
- Produces: Active worker threads processing signals and executing trades

- [ ] **Step 1: Understand why workers show 0 active**

The worker pool starts with 4 workers but `active_workers` is always 0. This means workers are waiting on `queue.pop_wait()` but no signals are being pushed to the queue. Check if the signal pipeline is producing signals that reach the queue.

- [ ] **Step 2: Trace signal flow from webhook to queue**

In `operator/src/handlers/monitoring.rs`, trace the `helius_webhook_handler` to see if accepted signals are being pushed to the engine queue:

```bash
ssh root@chimera-01.moez.tech "docker exec chimera-operator grep -A 5 'ACTIVE wallet signal accepted' /app/data/logs/operator.log | head -20"
```

If signals are accepted but not queued, the issue is in `queue_signal()` or the engine's `queue.push()` call.

- [ ] **Step 3: Check if the engine queue is full or blocked**

```bash
ssh root@chimera-01.moez.tech "docker exec chimera-operator grep -i 'queue.*full\|queue.*block\|queue.*push' /app/data/logs/operator.log | tail -10"
```

- [ ] **Step 4: Fix the signal-to-queue pipeline**

If the issue is in the signal pipeline, fix the code in `operator/src/handlers/monitoring.rs` to ensure accepted signals are properly pushed to the engine queue. The key code path is:
1. `helius_webhook_handler` receives webhook
2. Parses transaction → creates `Signal`
3. Calls `engine.queue_signal(signal)` 
4. Worker picks up signal from queue → executes trade

- [ ] **Step 5: Verify workers become active**

After fix, check worker pool stats:
```bash
ssh root@chimera-01.moez.tech "docker exec chimera-operator grep 'Worker pool statistics' /app/data/logs/operator.log | tail -5"
```

Expected: `active_workers` > 0 when signals are being processed.

---

## Verification Steps

After all tasks are complete, run these verification commands on the production server:

```bash
# 1. Check operator is running
ssh root@chimera-01.moez.tech "docker compose --profile mainnet-paper ps operator"

# 2. Check no errors in logs
ssh root@chimera-01.moez.tech "docker exec chimera-operator grep -c 'ERROR' /app/data/logs/operator.log"

# 3. Check worker pool is active
ssh root@chimera-01.moez.tech "docker exec chimera-operator grep 'Worker pool statistics' /app/data/logs/operator.log | tail -1"

# 4. Check active positions exist
ssh root@chimera-01.moez.tech "docker exec chimera-postgres psql -U chimera -d chimera -c 'SELECT COUNT(*) FROM positions WHERE state = ACTIVE;'"

# 5. Check trades are being executed
ssh root@chimera-01.moez.tech "docker exec chimera-postgres psql -U chimera -d chimera -c 'SELECT COUNT(*) FROM trades;'"

# 6. Check webhook pipeline works
ssh root@chimera-01.moez.tech "curl -s -o /dev/null -w '%{http_code}' https://chimera-01.moez.tech/api/v1/monitoring/helius-webhook"
```

## Execution Order

1. Task 1 (Jito RPC) — unblocks transaction submission
2. Task 2 (Database DNS) — unblocks database connectivity
3. Task 3 (HAProxy webhooks) — ensures webhook delivery works
4. Task 4 (Invalid wallet) — cleans up bad config
5. Task 5 (Jupiter/RPC endpoints) — ensures price feeds work
6. Task 6 (Active positions/wallets) — creates data needed for trades
7. Task 7 (Worker pool) — fixes signal processing pipeline

Each task depends on the previous one — database and RPC connectivity must be working before positions and wallets can be created, and the worker pool fix depends on signals flowing through the pipeline.
