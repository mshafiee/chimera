-- Migration: add FK from signal_aggregation.wallet_address to wallets.address
-- Existing rows with orphaned wallet_address values are deleted before the constraint
-- is applied so the migration does not fail on dirty data.

BEGIN TRANSACTION;

-- Pre-flight cleanup: remove orphaned wallet_address rows
DELETE FROM signal_aggregation
WHERE NOT EXISTS (SELECT 1 FROM wallets WHERE wallets.address = signal_aggregation.wallet_address);

-- Pre-flight dedupe: keep only the most recent row per unique key so the
-- unique indexes created below cannot fail on duplicate data.
DELETE FROM signal_aggregation
WHERE signature IS NOT NULL
  AND id NOT IN (
      SELECT MAX(id) FROM signal_aggregation
      WHERE signature IS NOT NULL
      GROUP BY token_address, wallet_address, signature
  );

DELETE FROM signal_aggregation
WHERE signature IS NULL
  AND id NOT IN (
      SELECT MAX(id) FROM signal_aggregation
      WHERE signature IS NULL
      GROUP BY token_address, wallet_address, direction, created_at
  );

-- SQLite does not support ADD CONSTRAINT on existing tables, so we recreate the table.
CREATE TABLE signal_aggregation_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    token_address TEXT NOT NULL,
    wallet_address TEXT NOT NULL,
    direction TEXT NOT NULL CHECK(direction IN ('BUY', 'SELL')),
    amount_sol TEXT NOT NULL,
    signature TEXT,
    is_consensus INTEGER DEFAULT 0,
    consensus_wallet_count INTEGER,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

INSERT INTO signal_aggregation_new (id, token_address, wallet_address, direction, amount_sol, signature, is_consensus, consensus_wallet_count, created_at)
SELECT id, token_address, wallet_address, direction, amount_sol, signature, is_consensus, consensus_wallet_count, created_at
FROM signal_aggregation;
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

COMMIT;
