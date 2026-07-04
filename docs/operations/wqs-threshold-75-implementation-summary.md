# WQS Threshold 75.0 Implementation Summary

**Date**: 2026-07-04
**Status**: ✅ Configuration Complete, Migration Ready
**Version**: 1.0

---

## Overview

Successfully implemented aggressive wallet roster downsizing by updating the default WQS ACTIVE threshold from 60.0-65.0 to 75.0 across all configuration files and code. This change limits active monitored wallets to extremely high-conviction traders only, reducing operational overhead and improving signal quality.

---

## Changes Implemented

### 1. Configuration Files Updated ✅

All WQS thresholds remain **fully configurable via environment variables** - only the default values were changed.

#### **scout/config.py**
- `get_min_wqs_active()`: 60.0 → **75.0**
- `get_min_wqs_candidate()`: 15.0 → **50.0** (raised to maintain gap)
- `get_min_wqs_whale()`: 55.0 → **70.0** (5 points below ACTIVE)
- `get_min_wqs_swing()`: 58.0 → **72.0** (3 points below ACTIVE)

#### **scout/main.py**
- `DEFAULT_MIN_WQS_ACTIVE`: 65.0 → **75.0**
- `DEFAULT_MIN_WQS_CANDIDATE`: 15.0 → **50.0**

#### **scout/core/validator.py**
- `PromotionCriteria.min_wqs_score`: 65.0 → **75.0**
- `PromotionCriteria.min_wqs_whale`: 55.0 → **70.0**
- `PromotionCriteria.min_wqs_swing`: 58.0 → **72.0**

#### **scout/core/wqs.py**
- `classify_wallet()` default `active_threshold`: 65.0 → **75.0**
- `classify_wallet()` default `candidate_threshold`: 20.0 → **50.0**

#### **docker-compose.yml**
- `SCOUT_MIN_WQS_ACTIVE`: 40.0 → **75.0**
- `SCOUT_MIN_WQS_CANDIDATE`: 15.0 → **50.0**

#### **docker-compose.prod.yml**
- `SCOUT_MIN_WQS_ACTIVE`: 60.0 → **75.0**
- `SCOUT_MIN_WQS_CANDIDATE`: 30.0 → **50.0**

#### **scout/.env.example**
- `SCOUT_MIN_WQS_ACTIVE`: 60.0 → **75.0**
- `SCOUT_MIN_WQS_CANDIDATE`: 30.0 → **50.0**

---

### 2. Database Migration Created ✅

**File**: `operator/migrations/20260704000000_downsize_active_roster.sql`

**Features**:
- ✅ Creates backup table `wallets_backup_20260704` for safe rollback
- ✅ Demotes ACTIVE wallets with WQS < 75 to CANDIDATE status
- ✅ Adds descriptive notes to demoted wallets
- ✅ Includes post-migration verification queries
- ✅ Provides two rollback options (full restore or selective restore)

**Testing Results**:
- Migration tested on backup database: ✅ **PASSED**
- 3/6 ACTIVE wallets correctly demoted (WQS: 55.0, 68.0, 74.9)
- 3/6 ACTIVE wallets retained (WQS: 75.0, 78.5, 85.0)
- Rollback procedure tested: ✅ **PASSED**

---

### 3. Testing Completed ✅

#### Configuration Validation
- ✅ ScoutConfig returns correct default values (75.0, 50.0, 70.0, 72.0)
- ✅ Environment variable overrides work correctly
- ✅ Wallet classification uses new thresholds properly

#### Wallet Classification Tests
All test cases **PASSED**:
- ✅ WQS 74.9 → CANDIDATE (below threshold)
- ✅ WQS 75.0 → ACTIVE (exactly at threshold)
- ✅ WQS 75.1 → ACTIVE (above threshold)
- ✅ WQS 76.0, conf 0.65 → CANDIDATE (low confidence)
- ✅ WQS 76.0, conf 0.70 → ACTIVE (sufficient confidence)
- ✅ WQS 49.9 → REJECTED (below CANDIDATE threshold)
- ✅ WQS 50.0 → CANDIDATE (exactly at CANDIDATE threshold)

#### Environment Variable Override Tests
- ✅ `SCOUT_MIN_WQS_ACTIVE=80.0` correctly overrides default
- ✅ `SCOUT_MIN_WQS_CANDIDATE=55.0` correctly overrides default
- ✅ Other thresholds maintain defaults when not overridden

---

## Deployment Procedure

### Pre-Deployment Checklist
- [x] All configuration files updated
- [x] Database migration script created and tested
- [x] Configuration validation tests passed
- [x] Migration tested on backup database
- [ ] Backup production database
- [ ] Notify team of expected signal volume reduction (60-75%)

### Deployment Steps

1. **Backup Database**
   ```bash
   cp data/chimera.db data/chimera.db.backup.$(date +%Y%m%d_%H%M%S)
   ```

2. **Run Migration**
   ```bash
   # Migration is now applied automatically by SQLx on operator startup
   # No manual application needed
   ```

3. **Verify Migration**
   ```bash
   sqlite3 data/chimera.db "
   SELECT COUNT(*) FROM wallets WHERE status = 'ACTIVE';
   SELECT COUNT(*) FROM wallets WHERE status = 'ACTIVE' AND wqs_score < 75.0;
   "
   ```

4. **Deploy Code Changes**
   ```bash
   # Choose deployment method based on infrastructure
   docker-compose down && docker-compose up -d --build
   # OR
   make build-operator && systemctl restart chimera-operator
   ```

5. **Verify Deployment**
   ```bash
   # Check health
   curl http://localhost:3000/api/v1/health

   # Verify Scout uses new thresholds
   cd scout && python main.py --dry-run --discovery-hours 1
   ```

---

## Expected Impact

### Before (Current State)
- ACTIVE threshold: 60.0-65.0
- Estimated ACTIVE wallets: ~40-60% of discovered wallets
- Signal volume: High (many low-conviction signals)

### After (Proposed State)
- ACTIVE threshold: **75.0**
- Estimated ACTIVE wallets: ~10-25% of discovered wallets (top decile)
- Signal volume: **60-75% reduction** (only elite traders)
- Monitoring load: **Proportionally reduced** (fewer wallets to poll)

---

## Rollback Procedure

If issues arise post-deployment:

### Quick Rollback (Selective)
```bash
sqlite3 data/chimera.db "
UPDATE wallets
SET status = 'ACTIVE',
    notes = REPLACE(notes, '; Auto-demoted: WQS < 75 (roster downsizing 2026-07-04)', ''),
    updated_at = (SELECT updated_at FROM wallets_backup_20260704 WHERE wallets.address = wallets_backup_20260704.address)
WHERE notes LIKE '%Auto-demoted: WQS < 75 (roster downsizing 2026-07-04)%';
"
```

### Full Rollback (Complete Restore)
```bash
sqlite3 data/chimera.db "
DROP TABLE wallets;
CREATE TABLE wallets AS SELECT * FROM wallets_backup_20260704;
"
```

### Code Rollback
```bash
git checkout <previous-commit-tag>
make build-operator
systemctl restart chimera-operator
```

---

## Monitoring & Validation

### Key Metrics to Monitor (First 7 Days)

1. **Wallet Metrics**
   - ACTIVE wallet count (should stabilize at new lower level)
   - CANDIDATE wallet count (should increase significantly)
   - Wallet promotion/demotion rate (should stabilize within 2-3 Scout cycles)

2. **Signal Metrics**
   - BUY signal volume (expected **60-75% reduction**)
   - BUY signal rejection rate (expected increase for CANDIDATE wallets)
   - SELL/EXIT signal volume (should remain unchanged)

3. **Performance Metrics**
   - RPC polling rate (should decrease proportionally)
   - Queue depth (should decrease with fewer signals)
   - Trade execution latency (should remain stable or improve)

4. **Financial Metrics**
   - Total PnL (monitor for impact of reduced signal volume)
   - Win rate (should improve with higher-conviction signals)
   - Profit factor (should improve with higher-quality signals)

### Alert Thresholds

⚠️ **Consider Adjusting Threshold If**:
- Signal volume drops > **85%** (consider lowering to 70.0)
- Signal rejection rate > **50%** (consider lowering to 70.0)
- PnL drops > **30%** for 3 consecutive days (consider rollback)
- Error rate increases > **10%** (investigate immediately)

---

## Configuration Reference

### Environment Variables (All Optional)

```bash
# Base thresholds
SCOUT_MIN_WQS_ACTIVE=75.0              # Default: 75.0 (elite traders only)
SCOUT_MIN_WQS_CANDIDATE=50.0           # Default: 50.0 (maintains gap)

# Archetype-specific thresholds
SCOUT_MIN_WQS_WHALE=70.0               # Default: 70.0 (5 points below ACTIVE)
SCOUT_MIN_WQS_SWING=72.0                # Default: 72.0 (3 points below ACTIVE)

# Dynamic adjustment (without code changes)
# Example: Lower threshold during discovery phase
SCOUT_MIN_WQS_ACTIVE=70.0

# Example: Raise threshold for extreme quality filtering
SCOUT_MIN_WQS_ACTIVE=80.0
```

### Threshold Logic

Wallets are classified as:
- **ACTIVE**: WQS >= 75.0 AND confidence >= 0.70
- **CANDIDATE**: WQS >= 50.0 OR (WQS >= 75.0 but confidence < 0.70)
- **REJECTED**: WQS < 50.0

Archetype-specific thresholds (WHALE, SWING) provide slightly lower entry points for specific trading patterns, while maintaining high overall quality standards.

---

## Files Modified

1. ✅ `scout/config.py` - Default threshold values
2. ✅ `scout/main.py` - Application defaults
3. ✅ `scout/core/validator.py` - Promotion criteria dataclass
4. ✅ `scout/core/wqs.py` - Classification function defaults
5. ✅ `docker-compose.yml` - Development environment
6. ✅ `docker-compose.prod.yml` - Production environment
7. ✅ `scout/.env.example` - Environment variable examples
8. ✅ `operator/migrations/20260704000000_downsize_active_roster.sql` - **NEW FILE**

---

## Next Steps

1. **Review this summary** with team before deployment
2. **Schedule deployment window** (preferably during low-activity period)
3. **Backup production database** immediately before migration
4. **Monitor metrics** closely for first 7 days post-deployment
5. **Adjust thresholds** via environment variables if needed (no code changes required)
6. **Clean up backup table** after 14 days: `DROP TABLE wallets_backup_20260704;`

---

## Success Criteria

Deployment is successful when:
- ✅ All configuration uses 75.0 as ACTIVE threshold
- ✅ Database has no ACTIVE wallets with WQS < 75
- ✅ Scout classifies wallets correctly with new thresholds
- ✅ Operator rejects BUY signals from CANDIDATE wallets
- ✅ Signal volume decreases by 60-75% (expected)
- ✅ No increase in error rates

---

## References

- **Plan Document**: `/Users/mohammad/.claude/plans/aggressive-wallet-roster-downsizing-toasty-lantern.md`
- **Migration Script**: `operator/migrations/20260704000000_downsize_active_roster.sql`
- **WQS Documentation**: `scout/core/wqs.py` (lines 1-100)
- **Wallet Classification Logic**: `scout/core/wqs.py` (lines 855-869)
- **Operator Wallet Enforcement**: `operator/src/handlers/webhook.rs` (lines 258-324)
