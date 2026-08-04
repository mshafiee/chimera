-- Rejection-rate wallet mute: tracks wallets whose BUY signals are
-- overwhelmingly rejected for hard, structural reasons (non-speculative,
-- unsafe, illiquid pump.fun). Mirrors toxic_wallets persistence pattern.

CREATE TABLE IF NOT EXISTS muted_wallets (
    wallet_address      TEXT PRIMARY KEY,
    is_muted            BOOLEAN      NOT NULL DEFAULT FALSE,
    muted_at            TIMESTAMPTZ,
    muted_until         TIMESTAMPTZ,
    window_size         INTEGER      NOT NULL DEFAULT 0,
    run_id              TEXT,
    created_at          TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_muted_wallets_is_muted
    ON muted_wallets (is_muted) WHERE is_muted;
