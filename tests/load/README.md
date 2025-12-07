# Load Tests

Performance and load testing for Chimera Operator.

## Tests

### 1. Webhook Flood Test (`webhook_flood.js`)

Tests the system under high webhook load:
- 100 requests/second constant load
- 200 requests/second burst test
- Measures p95/p99 latency
- Verifies rate limiting behavior

**Requirements:**
- [k6](https://k6.io/docs/getting-started/installation/) load testing tool

**Usage:**
```bash
# Basic test
k6 run webhook_flood.js

# Custom target
k6 run -e BASE_URL=http://localhost:8080 -e WEBHOOK_SECRET=your-secret webhook_flood.js

# Extended duration
k6 run --duration 60s webhook_flood.js
```

**Thresholds (from PDD):**
- p95 latency < 500ms
- Acceptance rate > 80%
- Rate limiting < 30%

### 2. Queue Saturation Test

Tests the priority queue load shedding behavior:
- Fill queue to 80% capacity
- Verify Spear signals are dropped
- Verify Shield/Exit signals still accepted

**Usage:**
```bash
cd operator
cargo test --test queue_saturation_test
```

## Running All Load Tests

```bash
make load-test
```

## Interpreting Results

### Webhook Flood Results

| Metric | Target | Meaning |
|--------|--------|---------|
| p95 latency | < 500ms | 95% of requests complete in under 500ms |
| p99 latency | < 1000ms | 99% of requests complete in under 1s |
| Acceptance rate | > 80% | At least 80% of valid requests accepted |
| Rate limit rate | < 30% | Less than 30% of requests rate limited |

### Queue Saturation Results

| Check | Expected |
|-------|----------|
| Spear rejected at 80% capacity | Yes |
| Shield accepted at 80% capacity | Yes |
| Exit accepted at 80% capacity | Yes |

## Troubleshooting

### High Latency

1. Check RPC endpoint latency: `ping <helius-endpoint>`
2. Check database locks: `sqlite3 chimera.db ".tables"`
3. Check memory usage: `ps aux | grep chimera`

### Low Acceptance Rate

1. Check rate limiter config in `config.yaml`
2. Check circuit breaker status
3. Verify webhook signature generation

### Server Not Reachable

1. Verify server is running: `curl http://localhost:8080/health`
2. Check firewall rules
3. Verify correct port in BASE_URL

