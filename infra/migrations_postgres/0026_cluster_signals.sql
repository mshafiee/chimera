-- Hydra Phase 2.2: cluster admission signals (temporal buying clusters).
--
-- One row per detected cluster event: >=3 co-funded wallets buying the same
-- mint within <=120s with collective volume >= $7,500 (or >=40 SOL).
-- Written by scout/core/cluster_detector.py, consumed by operator selection
-- (consensus_wallet_count >= 3 gate). Idempotent on (cluster_id).

CREATE TABLE IF NOT EXISTS cluster_signals (
    signal_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    token_mint VARCHAR(64) NOT NULL,
    cluster_id UUID NOT NULL,
    wallet_count INT NOT NULL CHECK (wallet_count >= 3),
    total_cluster_sol NUMERIC(18, 4) NOT NULL,
    detected_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    status VARCHAR(32) NOT NULL DEFAULT 'PENDING'
        CHECK (status IN ('PENDING', 'ADMITTED', 'REJECTED', 'EXPIRED')),
    CONSTRAINT uq_cluster_signal UNIQUE (cluster_id)
);

CREATE INDEX IF NOT EXISTS idx_cluster_signals_mint_time
    ON cluster_signals (token_mint, detected_at DESC);
CREATE INDEX IF NOT EXISTS idx_cluster_signals_status
    ON cluster_signals (status) WHERE status = 'PENDING';
