-- B3: Toxic wallet tracking for post-promotion performance monitoring.
-- Stores ToxicFlowDetector state so demotion decisions survive restarts.

CREATE TABLE IF NOT EXISTS toxic_wallets (
    wallet_address       TEXT PRIMARY KEY,
    selection_roi        DOUBLE PRECISION,
    post_promotion_roi   DOUBLE PRECISION,
    local_top_entries    INTEGER      NOT NULL DEFAULT 0,
    total_entries        INTEGER      NOT NULL DEFAULT 0,
    is_toxic             BOOLEAN      NOT NULL DEFAULT FALSE,
    toxic_reason         TEXT,
    detected_at          TIMESTAMPTZ,
    run_id               TEXT,
    created_at           TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at           TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_toxic_wallets_is_toxic ON toxic_wallets (is_toxic) WHERE is_toxic;
