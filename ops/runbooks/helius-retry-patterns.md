# Helius Retry Patterns & Best Practices

## Overview

This document describes Chimera's implementation of Helius retry logic and rate limiting, following official Helius best practices.

## Helius Rate Limits by Plan

| Plan | RPC Rate Limit | DAS & Enhanced APIs | Webhooks |
|------|----------------|---------------------|----------|
| **Free** | 10 req/s | 2 req/s | 5 webhooks |
| **Developer** | 50 req/s | 10 req/s | 50 webhooks |
| **Business** | 200 req/s | 50 req/s | 50 webhooks |
| **Professional** | 500 req/s | 100 req/s | 50 webhooks |

### Special Rate Limits

**Sending Transactions:**
- `sendTransaction`: 5 req/s (Developer Plan)
- `sendBundle`: Not available (Business+)
- `simulateBundle`: 50 req/s (Developer Plan)

**Complex Calls:**
- `getProgramAccounts`: 25 req/s (Developer Plan)

## Retry Strategy

Chimera implements Helius-recommended retry strategy:

### Backoff Pattern
- **Initial delay:** 1 second
- **Pattern:** Exponential backoff (1s → 2s → 4s → 8s → 16s)
- **Jitter:** ±25% random variation to prevent synchronized retries
- **Maximum backoff:** 30 seconds
- **Max attempts:** 5 retries

### Error Classification

**Retryable Errors:**
| Status | Description | Behavior |
|--------|-------------|----------|
| 408 | Request Timeout | Retry with backoff |
| 429 | Rate Limit Exceeded | Wait `Retry-After`, then retry with backoff |
| 500 | Internal Server Error | Retry with backoff |
| 502 | Bad Gateway | Retry with backoff |
| 503 | Service Unavailable | Retry with backoff |
| 504 | Gateway Timeout | Retry with backoff |

**Non-Retryable Errors:**
| Status | Description | Behavior |
|--------|-------------|----------|
| 400 | Bad Request | Fail immediately |
| 401 | Unauthorized | Fail immediately |
| 403 | Forbidden | Fail immediately |
| 404 | Not Found | Fail immediately |
| 409 | Conflict | Fail immediately |
| 422 | Validation Error | Fail immediately |

### Implementation Locations

**Python (`scout/core/helius_client.py`):**
- `_retry_with_backoff()`: Core retry logic with exponential backoff
- `_is_retryable_error()`: Error classification
- `_rate_limit_async()`: Rate limiting with ±10% jitter

**Rust (`operator/src/retry.rs`):**
- `retry_with_backoff()`: Generic retry utility
- `calculate_backoff()`: Backoff calculation with jitter
- `is_retryable_status()`: Status code classification

## Rate Limiting

### Configuration

**Current Settings (`config.yaml`):**
```yaml
rpc:
  rate_limit_per_second: 40  # 50 RPS limit - 20% buffer
```

### Adaptive Rate Limiting (Python)

The Python client implements adaptive rate limiting:
- Monitors latency samples (up to 100 samples)
- Tracks success/failure ratio
- Dynamically adjusts delay based on conditions:

```python
# Slow down if:
# - Average latency > 200ms
# - Success rate < 95%

# Speed up if:
# - Average latency < 50ms
# - Success rate > 99%
```

### Priority Queue (Rust)

Request prioritization:
1. **Exit signals** (highest) - Reduced wait time
2. **Entry signals** (medium) - Standard wait
3. **Polling operations** (lowest) - Full wait

Request weighting:
- Standard request: 1 credit
- Simulation request: 5 credits

## Circuit Breakers

### Thresholds

**Python Circuit Breaker:**
- Threshold: 5 consecutive failures (configurable)
- Reset time: 60 seconds (configurable)
- Behavior: Stops requests when open

**Rust Circuit Breaker:**
- Configurable via `CircuitBreakerConfig`
- Monitors RPC health
- Auto-recovers every 5 minutes

### Monitoring Circuit Breaker State

Check circuit breaker status via health endpoint:
```bash
curl http://localhost:8080/api/v1/health
```

## Troubleshooting

### High Rate Limit Errors

**Symptoms:** Frequent 429 errors

**Solutions:**
1. Check configured rate limit vs plan limit
2. Verify adaptive rate limiting is enabled
3. Reduce concurrent requests
4. Consider upgrading Helius plan

### Slow Response Times

**Symptoms:** Latency > 200ms consistently

**Solutions:**
1. Check server location (should be US-East or Amsterdam)
2. Verify network routing
3. Monitor adaptive rate limiter stats
4. Check for circuit breaker trips

### Connection Pool Issues

**Symptoms:** Connection pool exhaustion

**Solutions:**
1. Verify connection pool configuration:
   - `limit=100` (total connections)
   - `limit_per_host=50` (per host)
2. Check for connection leaks
3. Monitor session reuse

## Metrics & Monitoring

### Key Metrics

**Rate Limiter Metrics (`/metrics`):**
```prometheus
chimera_rate_limit_requests_per_second
chimera_rate_limit_total_credits_used
chimera_rate_limit_current_credits
chimera_rate_limit_max_credits
```

**Helius Metrics:**
```prometheus
chimera_helius_cache_hits
chimera_helius_cache_misses
chimera_helius_successful_requests
chimera_helius_retried_requests
chimera_helius_failed_requests
```

### Monitoring Commands

```bash
# Check rate limit stats (Python)
cd scout
python -c "
from core.helius_client import HeliusClient
client = HeliusClient()
print(client.get_rate_limit_stats())
"

# Check health endpoint
curl http://localhost:8080/api/v1/health

# View Prometheus metrics
curl http://localhost:8080/metrics
```

## Testing

### Running Tests

**Rust Retry Tests:**
```bash
cd operator
cargo test --test helius_retry_tests
```

**Python 429 Tests:**
```bash
cd scout
python -m pytest tests/test_helius_429_handling.py -v
```

**Load Testing:**
```bash
k6 run tests/load/helius_rate_limit.js
```

## References

- [Helius Rate Limits Documentation](https://www.helius.dev/docs/billing/rate-limits)
- [Helius RPC Optimization Guide](https://www.helius.dev/docs/rpc/optimization-techniques)
- [Helius API Reference](https://www.helius.dev/docs/api-reference)

## Changelog

### v7.1.1 (2025-01-17)
- Added Rust retry utility with exponential backoff
- Fixed Python 429 handling to apply backoff after Retry-After
- Added connection pooling configuration to Python client
- Added metrics export for rate limiter
- Created comprehensive test coverage for retry logic

### Previous Versions
- Python client already had proper retry implementation
- Rust client used HTTP calls without retry logic
