# Scout Module Deployment Guide

**Version:** 1.0.0  
**Date:** 2025-12-06

---

## Pre-Deployment Checklist

### ✅ Code Review

- [x] All code changes reviewed
- [x] Unit tests written and passing
- [x] Integration tests written and passing
- [x] Documentation updated
- [x] No breaking changes identified

### ✅ Database

- [x] `historical_liquidity` table exists in schema
- [x] Database path configured correctly
- [x] Write permissions verified

### ✅ Dependencies

- [x] Python dependencies installed
- [x] Helius API key configured
- [x] Database connection tested

---

## Deployment Steps

### 1. Pre-Deployment Testing

```bash
# Run all tests
cd scout
pytest tests/ -v

# Run Scout in dry-run mode
python main.py --dry-run --verbose

# Verify output
ls -lh ../data/roster_new.db
```

### 2. Backup Current State

```bash
# Backup current roster
cp data/roster_new.db data/roster_new.db.backup.$(date +%Y%m%d)

# Backup database
cp data/chimera.db data/chimera.db.backup.$(date +%Y%m%d)
```

### 3. Deploy Code Changes

#### Option A: Docker Deployment

```bash
# Rebuild Scout container
cd /path/to/chimera
docker-compose build scout

# Restart Scout service
docker-compose restart scout

# Verify logs
docker-compose logs -f scout
```

#### Option B: Direct Deployment

```bash
# Pull latest code
git pull origin main

# Install/update dependencies
cd scout
pip install -r requirements.txt

# Verify installation
python -c "from scout.core import *; print('OK')"
```

### 4. Verify Deployment

```bash
# Run Scout manually
cd scout
python main.py --dry-run --verbose

# Check for errors
# Verify historical liquidity collection
# Verify metric calculations
# Verify WQS scores
```

### 5. Monitor Initial Run

```bash
# Watch logs
tail -f /var/log/chimera/scout.log

# Or with Docker
docker-compose logs -f scout
```

**Monitor for:**
- Historical liquidity collection
- Fallback rate (should decrease over time)
- Backtest validation results
- Any errors or warnings

---

## Post-Deployment Verification

### 1. Verify Historical Liquidity

```bash
# Check database
sqlite3 data/chimera.db "SELECT COUNT(*) FROM historical_liquidity;"

# Check recent entries
sqlite3 data/chimera.db "SELECT * FROM historical_liquidity ORDER BY timestamp DESC LIMIT 10;"
```

**Expected:** Historical liquidity entries should be accumulating.

### 2. Verify Metric Calculations

```bash
# Run Scout with verbose output
cd scout
python main.py --dry-run --verbose

# Check output for:
# - ROI calculations (should be from price changes)
# - Win rate (should be from actual PnL)
# - Drawdown (should be calculated, not hardcoded)
```

### 3. Verify WQS Scores

```bash
# Check WQS scores
# Scores should start from 0, not 50
# Run Scout and verify score distribution
```

### 4. Verify Backtest Results

```bash
# Check backtest pass rate
# Review backtest failure reasons
# Verify historical liquidity is being used
```

---

## Rollback Plan

### If Issues Arise

#### Option 1: Revert Code

```bash
# Revert to previous version
git checkout <previous-commit>

# Rebuild/restart
docker-compose build scout
docker-compose restart scout
```

#### Option 2: Disable Features

```bash
# Run with skip-backtest (uses old validation)
python main.py --skip-backtest

# Or modify code to use old methods
# (Old methods are kept as wrappers)
```

#### Option 3: Database Rollback

```bash
# Restore backup
cp data/chimera.db.backup.YYYYMMDD data/chimera.db
cp data/roster_new.db.backup.YYYYMMDD data/roster_new.db
```

---

## Monitoring

### Key Metrics

Monitor these metrics post-deployment:

1. **Historical Liquidity Usage**
   ```sql
   SELECT 
       COUNT(*) as total_trades,
       SUM(CASE WHEN source LIKE '%fallback%' THEN 1 ELSE 0 END) as fallback_count,
       100.0 * SUM(CASE WHEN source LIKE '%fallback%' THEN 1 ELSE 0 END) / COUNT(*) as fallback_rate
   FROM historical_liquidity;
   ```

2. **Backtest Pass Rate**
   - Check Scout logs for backtest results
   - Monitor pass/fail ratio

3. **Performance**
   - Time per wallet analysis
   - Database query performance
   - Memory usage

### Alerts

Set up alerts for:

- High fallback rate (> 50%)
- Backtest failures (> 80%)
- Performance degradation
- Database errors

---

## Configuration

### Environment Variables

```bash
# Required
HELIUS_API_KEY=your-api-key
CHIMERA_DB_PATH=/path/to/chimera.db

# Optional
BIRDEYE_API_KEY=your-birdeye-key  # For historical liquidity
```

### Docker Configuration

Update `docker-compose.yml`:

```yaml
scout:
  environment:
    - HELIUS_API_KEY=${HELIUS_API_KEY}
    - CHIMERA_DB_PATH=/app/data/chimera.db
  volumes:
    - ./data:/app/data
```

---

## Troubleshooting

### Issue: Historical Liquidity Not Available

**Symptoms:**
- All trades use fallback liquidity
- High fallback rate in logs

**Solution:**
1. Verify database path is correct
2. Check `historical_liquidity` table exists
3. Run Scout to collect data
4. Wait for data to accumulate (may take several runs)

### Issue: Slow Performance

**Symptoms:**
- Scout takes too long to analyze wallets
- High database query times

**Solution:**
1. Check database indexes
2. Reduce `max_wallets` limit
3. Use `--skip-backtest` for faster runs
4. Consider database optimization

### Issue: Incorrect Metrics

**Symptoms:**
- ROI seems wrong
- Win rate doesn't match expectations

**Solution:**
1. Verify price data is available
2. Check trade PnL data
3. Review metric calculation logs
4. Compare with manual calculations

### Issue: Backtest Failures

**Symptoms:**
- Many wallets failing backtest
- Low pass rate

**Solution:**
1. Review failure reasons in logs
2. Check historical liquidity data
3. Adjust liquidity thresholds if needed
4. Verify slippage/fee calculations

---

## Performance Tuning

### Database Optimization

```sql
-- Create indexes for historical liquidity queries
CREATE INDEX IF NOT EXISTS idx_historical_liquidity_token_time 
    ON historical_liquidity(token_address, timestamp DESC);

CREATE INDEX IF NOT EXISTS idx_historical_liquidity_timestamp 
    ON historical_liquidity(timestamp DESC);
```

### Batch Operations

Scout automatically batches liquidity storage. For manual optimization:

```python
# Batch store liquidity snapshots
provider.store_liquidity_batch(liquidity_snapshots)
```

### Caching

Consider adding caching for:
- Frequently accessed historical liquidity
- Wallet metrics
- Current liquidity data

---

## Maintenance

### Regular Tasks

1. **Monitor Historical Liquidity Collection**
   - Check data quality
   - Verify accumulation rate

2. **Review Backtest Results**
   - Analyze failure patterns
   - Adjust thresholds if needed

3. **Database Maintenance**
   - Clean old historical liquidity data (> 90 days)
   - Vacuum database periodically

4. **Performance Monitoring**
   - Track query times
   - Monitor memory usage
   - Optimize slow queries

### Database Cleanup

```sql
-- Remove old historical liquidity (> 90 days)
DELETE FROM historical_liquidity 
WHERE timestamp < datetime('now', '-90 days');
```

---

## Support

### Logs

Scout logs are available at:

- **Docker:** `docker-compose logs scout`
- **Systemd:** `journalctl -u chimera-scout`
- **File:** `/var/log/chimera/scout.log`

### Debug Mode

Run Scout with verbose output:

```bash
python main.py --verbose --dry-run
```

### Common Issues

See [Troubleshooting](#troubleshooting) section above.

---

## Success Criteria

Deployment is successful when:

- ✅ All tests pass
- ✅ Historical liquidity collection working
- ✅ Metric calculations accurate
- ✅ WQS scores start from 0
- ✅ Backtest validation working
- ✅ No errors in logs
- ✅ Performance acceptable (< 2s per wallet)

---

## Next Steps

After successful deployment:

1. Monitor for 24-48 hours
2. Review initial results
3. Adjust thresholds if needed
4. Document any issues
5. Plan future enhancements

---

**Deployment Date:** ___________  
**Deployed By:** ___________  
**Status:** ___________  

---

**For questions or issues, refer to:**
- [User Guide](./scout-user-guide.md)
- [Code Review](./scout-code-review.md)
- [Implementation Plan](./scout-gaps-fix-plan.md)




