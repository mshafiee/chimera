-- Migration 003: Add missing indexes for scan-heavy queries
-- Apply with: sqlite3 data/chimera.db < database/migrations/003_missing_indexes.sql

-- Dead-letter queue retry worker scans the full table for retryable rows
CREATE INDEX IF NOT EXISTS idx_dlq_can_retry ON dead_letter_queue(can_retry);

-- TTL expiration check walks all wallets to find expired entries
CREATE INDEX IF NOT EXISTS idx_wallets_ttl_expires ON wallets(ttl_expires_at);

-- 24-hour PnL window query filters by (created_at, status) — most selective order is
-- created_at DESC first so the planner can use the index for the time range scan
CREATE INDEX IF NOT EXISTS idx_trades_created_status ON trades(created_at DESC, status);

-- Track this migration as applied
INSERT OR IGNORE INTO schema_migrations (version) VALUES ('003');
