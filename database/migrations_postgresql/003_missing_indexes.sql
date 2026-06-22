-- Migration 003: Add missing indexes for scan-heavy queries

-- Dead-letter queue retry worker scans the full table for retryable rows
CREATE INDEX IF NOT EXISTS idx_dlq_can_retry ON dead_letter_queue(can_retry);

-- TTL expiration check walks all wallets to find expired entries
CREATE INDEX IF NOT EXISTS idx_wallets_ttl_expires ON wallets(ttl_expires_at);

-- 24-hour PnL window query filters by (created_at, status)
CREATE INDEX IF NOT EXISTS idx_trades_created_status ON trades(created_at DESC, status);

-- Track this migration as applied
INSERT INTO schema_migrations (version, applied_at) VALUES ('003', CURRENT_TIMESTAMP)
ON CONFLICT (version) DO NOTHING;
