-- Migration 003: Add missing indexes for scan-heavy queries
-- Apply with: sqlite3 data/chimera.db < database/migrations_sqlite/003_missing_indexes.sql

BEGIN;

-- Dead-letter queue retry worker scans the full table for retryable rows
CREATE INDEX IF NOT EXISTS idx_dlq_can_retry ON dead_letter_queue(can_retry) WHERE can_retry = 1;

-- TTL expiration check walks all wallets to find expired entries
CREATE INDEX IF NOT EXISTS idx_wallets_ttl_expires ON wallets(ttl_expires_at);

-- 24-hour PnL window query filters by (status, created_at) — equality-first order lets the
-- planner narrow by status and then scan the time range
CREATE INDEX IF NOT EXISTS idx_trades_created_status ON trades(status, created_at DESC);

-- Track this migration as applied
INSERT OR IGNORE INTO schema_migrations (version) VALUES ('003');

COMMIT;
