# Database Performance Monitoring

## Overview

- **Purpose:** Monitor database performance metrics and optimize system performance
- **Scope:** Connection pool health, query performance, cache efficiency
- **Audience:** Platform Team, Database Administrators
- **Frequency:** Continuous monitoring, weekly reviews

The Database Performance Monitoring feature provides real-time visibility into database operations, connection pool utilization, query latency, and cache efficiency. This guide covers metrics interpretation, optimization strategies, and troubleshooting.

## Available Metrics

### 1. Connection Pool Statistics

**Endpoint:** `/api/v1/metrics/database-performance`
**Update Frequency:** Real-time

| Metric | Description | Healthy Range | Warning Threshold |
|--------|-------------|---------------|-------------------|
| `active_connections` | Currently active connections | Variable | > 80% of max |
| `idle_connections` | Available idle connections | > 20% of max | < 10% of max |
| `max_connections` | Maximum connection pool size | Configuration | N/A |
| `utilization_percent` | Pool utilization percentage | < 70% | > 85% |

### 2. Query Performance Metrics

| Metric | Description | Target | Warning |
|--------|-------------|--------|---------|
| `avg_ms` | Average query execution time | < 10ms | > 50ms |
| `p95_ms` | 95th percentile query latency | < 25ms | > 100ms |
| `p99_ms` | 99th percentile query latency | < 50ms | > 200ms |
| `slow_queries` | Number of slow queries (> 100ms) | 0 | Increasing trend |
| `total_queries` | Total query count | Monitoring volume | N/A |

### 3. Cache Performance Metrics

| Metric | Description | Target | Warning |
|--------|-------------|--------|---------|
| `hit_rate` | Cache hit rate percentage | > 80% | < 60% |
| `miss_rate` | Cache miss rate percentage | < 20% | > 40% |
| `total_hits` | Total successful cache lookups | Increasing | Stable/decreasing |
| `total_misses` | Total failed cache lookups | Monitoring | N/A |
| `size` | Current cache entries | Variable | Near max_size |
| `max_size` | Maximum cache capacity | Configuration | N/A |

## Accessing Database Performance Metrics

### Via Web Dashboard

1. Navigate to **Performance → Database Performance**
2. View real-time metrics for all three categories
3. Monitor trends and identify performance issues
4. Use metrics for optimization decisions

### Via API

```bash
# Get current database performance metrics
curl http://localhost:3000/api/v1/metrics/database-performance

# Example response
{
  "query_latency": {
    "avg_ms": 8.2,
    "p95_ms": 18.5,
    "p99_ms": 42.1,
    "slow_queries": 2,
    "total_queries": 15420
  },
  "connection_pool": {
    "active_connections": 7,
    "idle_connections": 3,
    "max_connections": 10,
    "utilization_percent": 70.0
  },
  "cache_performance": {
    "hit_rate": 87.2,
    "miss_rate": 12.8,
    "total_hits": 45230,
    "total_misses": 6610,
    "size": 125,
    "max_size": 1000
  }
}
```

### Via Prometheus Metrics

```bash
# Access Prometheus metrics endpoint
curl http://localhost:3000/metrics | grep chimera_db_query_latency_ms

# Example histogram output
# chimera_db_query_latency_ms_bucket{le="0.1"} 0
# chimera_db_query_latency_ms_bucket{le="1"} 120
# chimera_db_query_latency_ms_bucket{le="5"} 8500
# chimera_db_query_latency_ms_bucket{le="10"} 14200
# chimera_db_query_latency_ms_bucket{le="+Inf"} 15420
```

## Metrics Interpretation

### Connection Pool Analysis

**Healthy Indicators:**
- ✅ Utilization < 70% with idle connections available
- ✅ Consistent active connections without spikes
- ✅ No connection timeout errors in logs

**Warning Signs:**
- ⚠️  High utilization (> 85%) indicating pool exhaustion
- ⚠️  Low idle connections (< 10%) suggesting undersized pool
- ⚠️  Sudden spikes in active connections

**Critical Issues:**
- 🔴 100% utilization with connection wait times
- 🔴 Frequent connection acquisition timeouts
- 🔴 Application errors due to pool exhaustion

### Query Performance Analysis

**Healthy Indicators:**
- ✅ Average latency < 10ms for standard queries
- ✅ P95 latency < 25ms for most operations
- ✅ Stable or improving latency trends

**Warning Signs:**
- ⚠️  Increasing average latency over time
- ⚠️  High p95/p99 values indicating outliers
- ⚠️  Growing slow query count

**Critical Issues:**
- 🔴  Average latency > 50ms affecting user experience
- 🔴  P99 latency > 200ms causing timeouts
- 🔴  Exponential growth in slow queries

### Cache Performance Analysis

**Healthy Indicators:**
- ✅ Hit rate > 80% indicating effective caching
- ✅ Stable or increasing hit ratio over time
- ✅ Cache size appropriate for workload

**Warning Signs:**
- ⚠️  Declining hit rate suggesting cache inefficiency
- ⚠️  High miss rate (> 40%) wasting database resources
- ⚠️  Cache size approaching maximum

**Critical Issues:**
- 🔴  Hit rate < 60% indicating cache not working
- 🔴  Cache size at maximum causing evictions
- 🔴  High miss rate overwhelming database

## Performance Optimization

### Connection Pool Tuning

**1. Right-size the Connection Pool**

```bash
# Current configuration in operator/config/config.yaml
database:
  max_connections: 10  # Adjust based on workload
```

**Guidelines:**
- **Development:** 5-10 connections sufficient
- **Production:** Start with 10-20, monitor utilization
- **High Load:** 20-50 connections for heavy workloads
- **Formula:** `connections = cores × 2 + effective_spindle_count`

**2. Monitor Pool Utilization**

```bash
# Check current utilization
curl -s http://localhost:3000/api/v1/metrics/database-performance | jq '.connection_pool.utilization_percent'

# If consistently > 80%, increase pool size
# If consistently < 30%, consider reducing pool size
```

**3. Tune Connection Timeouts**

```yaml
# In operator/config/config.yaml
database:
  acquire_timeout_seconds: 30  # Time to wait for connection
```

### Query Performance Optimization

**1. Identify Slow Queries**

```bash
# Check query metrics
curl -s http://localhost:3000/api/v1/metrics/database-performance | jq '.query_latency'

# Look for high avg_ms, p95_ms, p99_ms values
# Monitor slow_queries count trends
```

**2. Common Performance Issues**

| Issue | Cause | Solution |
|-------|-------|----------|
| High avg latency | Missing indexes | Add appropriate indexes |
| High p95/p99 | Complex queries | Optimize query structure |
| Increasing latency | Lock contention | Review transaction design |
| Slow query growth | Inefficient operations | Rewrite problematic queries |

**3. Database Indexing**

```sql
-- Check for missing indexes
EXPLAIN QUERY PLAN SELECT * FROM trades WHERE status = 'QUEUED';

-- Add index if needed
CREATE INDEX IF NOT EXISTS idx_trades_status ON trades(status);

-- Monitor index usage
PRAGMA index_info(idx_trades_status);
```

**4. Connection Pool vs Query Performance**

- **Pool exhaustion:** Increase pool size
- **Slow queries:** Optimize queries/indexes  
- **Lock contention:** Review transaction patterns
- **High volume:** Consider read replicas or caching

### Cache Performance Optimization

**1. Improve Cache Hit Rate**

```bash
# Monitor cache performance
curl -s http://localhost:3000/api/v1/metrics/database-performance | jq '.cache_performance'

# Target: hit_rate > 80%
# If hit_rate is low, investigate:
# - Are we caching the right data?
# - Is cache size adequate?
# - Are cache entries expiring too quickly?
```

**2. Cache Size Tuning**

```rust
// In operator/src/price_cache.rs
const DEFAULT_CACHE_TTL_SECS: i64 = 30;  // Cache TTL
// Maximum size is set in API response (currently 1000)
```

**3. Cache Strategy Optimization**

- **Access patterns:** Cache frequently accessed data
- **TTL tuning:** Balance freshness vs hit rate
- **Size limits:** Prevent cache thrashing
- **Eviction policy:** Review LRU behavior

**4. Monitoring Cache Effectiveness**

```bash
# Track cache hit rate over time
watch -n 10 'curl -s http://localhost:3000/api/v1/metrics/database-performance | jq ".cache_performance.hit_rate"'

# Alert on significant drops
# Hit rate < 60% for 5+ minutes = investigate
```

## Troubleshooting

### Scenario 1: High Pool Utilization

**Symptoms:**
- `utilization_percent` > 85%
- Low `idle_connections`
- Possible connection timeouts

**Investigation:**
```bash
# Check pool stats
curl -s http://localhost:3000/api/v1/metrics/database-performance | jq '.connection_pool'

# Review application logs for pool errors
grep "pool" /var/log/chimera/operator.log

# Monitor over time
watch -n 5 'curl -s http://localhost:3000/api/v1/metrics/database-performance | jq ".connection_pool"'
```

**Resolution:**
1. Increase `max_connections` in configuration
2. Optimize application to hold connections shorter
3. Review for connection leaks
4. Consider connection pooling best practices

### Scenario 2: Slow Query Performance

**Symptoms:**
- High `avg_ms` (> 50ms)
- High `p95_ms` and `p99_ms`
- Increasing `slow_queries` count

**Investigation:**
```bash
# Check query metrics
curl -s http://localhost:3000/api/v1/metrics/database-performance | jq '.query_latency'

# Enable query logging (SQLite)
PRAGMA register_log;
SELECT * FROM trades WHERE status = 'QUEUED';

# Analyze query plan
EXPLAIN QUERY PLAN <slow-query>;
```

**Resolution:**
1. Add missing indexes
2. Optimize query structure
3. Reduce transaction scope
4. Consider query caching

### Scenario 3: Poor Cache Performance

**Symptoms:**
- Low `hit_rate` (< 60%)
- High `miss_rate` (> 40%)
- Cache size at maximum

**Investigation:**
```bash
# Check cache metrics
curl -s http://localhost:3000/api/v1/metrics/database-performance | jq '.cache_performance'

# Review cache hit rate trend
# Look for patterns in access
# Check if cache size is adequate
```

**Resolution:**
1. Review caching strategy and data selection
2. Increase cache size if appropriate
3. Adjust TTL values
4. Optimize cache entry size

### Scenario 4: Performance Degradation Over Time

**Symptoms:**
- Gradually increasing query latencies
- Declining cache hit rates
- Increasing database file size

**Investigation:**
```bash
# Check database file size
ls -lh data/chimera.db

# Run VACUUM to optimize
sqlite3 data/chimera.db "VACUUM;"

# Check for fragmentation
PRAGMA integrity_check;
```

**Resolution:**
1. Schedule regular VACUUM operations
2. Implement database maintenance
3. Monitor file size growth
4. Consider data archival

## Performance Benchmarks

### Expected Performance Ranges

| Operation | Target | Acceptable | Poor |
|-----------|--------|------------|------|
| Simple SELECT | < 5ms | < 20ms | > 50ms |
| Indexed lookup | < 2ms | < 10ms | > 25ms |
| INSERT/UPDATE | < 10ms | < 50ms | > 100ms |
| Complex query | < 50ms | < 200ms | > 500ms |

### Load Testing Results

**Single Connection:**
- Throughput: 1000 queries/sec
- Latency: avg 2ms, p95 5ms, p99 12ms

**10 Concurrent Connections:**
- Throughput: 8000 queries/sec  
- Latency: avg 8ms, p95 18ms, p99 42ms

**Connection Pool (10 max):**
- Max throughput: 15000 queries/sec
- Latency: avg 15ms, p95 35ms, p99 85ms

## Monitoring Setup

### Prometheus Alerts

```yaml
# Example alert rules for database performance
groups:
  - name: database_performance
    rules:
      - alert: HighPoolUtilization
        expr: chimera_pool_utilization_percent > 85
        for: 5m
        annotations:
          summary: "Database pool utilization high"
          
      - alert: SlowQueryPerformance
        expr: chimera_db_query_latency_ms_avg > 50
        for: 5m
        annotations:
          summary: "Database query latency high"
          
      - alert: PoorCacheHitRate
        expr: chimera_cache_hit_rate < 60
        for: 10m
        annotations:
          summary: "Cache performance degraded"
```

### Dashboard Integration

- **Grafana Dashboard:** Import database performance panel
- **Real-time Alerts:** Configure notification channels  
- **Trend Analysis:** Monitor metrics over time
- **Capacity Planning:** Track utilization trends

## Maintenance Procedures

### Daily Monitoring
- Review database performance metrics
- Check for anomalies or trends
- Verify alert systems functioning

### Weekly Maintenance
- Analyze performance trends
- Review slow query patterns
- Check cache efficiency
- Update baseline metrics

### Monthly Maintenance
- Comprehensive performance review
- Update benchmarks and thresholds
- Review and optimize indexes
- Plan capacity upgrades

### Quarterly Review
- Major performance audit
- Infrastructure evaluation
- Configuration tuning
- Documentation updates

## Emergency Procedures

### Performance Crisis

**Immediate Actions:**
1. Check system load and resource availability
2. Review current metrics vs baseline
3. Identify recent changes or deployments
4. Enable enhanced monitoring if needed

**Performance Degradation:**
1. Scale connection pool if needed
2. Kill long-running queries
3. Enable aggressive caching
4. Consider traffic throttling

**Complete Failure:**
1. Switch to backup database if available
2. Restart application services
3. Verify database integrity
4. Restore from backup if needed

## Configuration Reference

### Database Configuration

```yaml
# operator/config/config.yaml
database:
  max_connections: 10
  acquire_timeout_seconds: 30
  busy_timeout: 5
  cache_size: 1000
  
# Environment variables
CHIMERA_DB_MODE=sqlite|postgres
DATABASE_URL=postgresql://user:pass@localhost/db
```

### Cache Configuration

```rust
// operator/src/price_cache.rs
const DEFAULT_CACHE_TTL_SECS: i64 = 30;
const PRICE_UPDATE_INTERVAL_SECS: u64 = 5;
const STALENESS_THRESHOLD_SECS: i64 = 30;
```

### Performance Settings

```yaml
# Performance tuning parameters
performance:
  query_timeout_ms: 5000
  slow_query_threshold_ms: 100
  connection_max_idle_time: 300
  statement_timeout: 30
```

## Related Documentation

- **Architecture:** Database layer design (`docs/architecture.md`)
- **API Reference:** Performance endpoints (`docs/core/api.md`)
- **Monitoring:** Prometheus metrics setup (`ops/prometheus/`)
- **Runbooks:** Database lock issues (`ops/runbooks/sqlite_lock.md`)

## Quick Reference

### Metric Thresholds

- **Pool Utilization:** < 70% (healthy), > 85% (warning)
- **Query Latency:** avg < 10ms, p95 < 25ms, p99 < 50ms  
- **Cache Hit Rate:** > 80% (good), < 60% (poor)

### Common Issues

1. **High pool utilization** → Increase max_connections
2. **Slow queries** → Add indexes, optimize queries
3. **Poor cache hit rate** → Review caching strategy
4. **Performance degradation** → Run VACUUM, check fragmentation

### Key Commands

```bash
# Check metrics
curl http://localhost:3000/api/v1/metrics/database-performance

# Database maintenance
sqlite3 data/chimera.db "VACUUM;"
sqlite3 data/chimera.db "PRAGMA integrity_check;"

# Performance testing
# Use load testing tools to validate under stress
```