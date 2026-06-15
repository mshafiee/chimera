-- Migration: add FK from signal_aggregation.wallet_address to wallets.address
-- Existing rows with orphaned wallet_address values are deleted before the constraint
-- is applied so the migration does not fail on dirty data.

DELETE FROM signal_aggregation
WHERE wallet_address NOT IN (SELECT address FROM wallets);

-- SQLite does not support ADD CONSTRAINT on existing tables, so we recreate the table.
CREATE TABLE signal_aggregation_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    token_address TEXT NOT NULL,
    wallet_address TEXT NOT NULL,
    direction TEXT NOT NULL CHECK(direction IN ('BUY', 'SELL')),
    amount_sol REAL NOT NULL,
    signature TEXT,
    is_consensus INTEGER DEFAULT 0,
    consensus_wallet_count INTEGER,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

INSERT INTO signal_aggregation_new SELECT * FROM signal_aggregation;
DROP TABLE signal_aggregation;
ALTER TABLE signal_aggregation_new RENAME TO signal_aggregation;

CREATE UNIQUE INDEX IF NOT EXISTS idx_signal_aggregation_unique_with_sig
    ON signal_aggregation(token_address, wallet_address, signature)
    WHERE signature IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_signal_aggregation_unique_no_sig
    ON signal_aggregation(token_address, wallet_address, direction, created_at)
    WHERE signature IS NULL;

CREATE INDEX IF NOT EXISTS idx_signal_aggregation_token_time
    ON signal_aggregation(token_address, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_signal_aggregation_consensus
    ON signal_aggregation(is_consensus) WHERE is_consensus = 1;
