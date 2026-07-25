-- C1: Run-scoped immutable decision records.
-- Every decide_buy / decide_sell call produces a persisted row with full
-- inputs + outputs, keyed by decision_id. Rejected decisions keep
-- trade_uuid = NULL so the full admission funnel (not just admitted trades)
-- is reconstructible per run.

CREATE TABLE IF NOT EXISTS decision_records (
    decision_id          TEXT PRIMARY KEY,
    run_id               TEXT NOT NULL,
    trade_uuid           TEXT,                -- NULL for rejected decisions
    ingress              TEXT NOT NULL,       -- 'webhook' | 'helius'
    wallet_address       TEXT NOT NULL,
    token_address        TEXT NOT NULL,
    action               TEXT NOT NULL,       -- 'BUY' | 'SELL'
    strategy             TEXT,                -- 'SHIELD' | 'SPEAR' | 'EXIT'
    admitted             BOOLEAN NOT NULL,
    rejection_code       TEXT,
    rejection_reason     TEXT,
    size_sol             NUMERIC(30,18),
    source_amount_sol    NUMERIC(30,18) NOT NULL,
    wqs                  DOUBLE PRECISION,
    wqs_confidence       DOUBLE PRECISION,
    quality_score        DOUBLE PRECISION,
    consensus_wallet_count INTEGER,
    regime_multiplier    NUMERIC(20,10),
    token_age_hours      DOUBLE PRECISION,
    liquidity_usd        NUMERIC(30,18),
    volume_24h_usd       NUMERIC(30,18),
    price_impact_pct     NUMERIC(20,10),
    quote_json           JSONB,               -- Jupiter quote at decision time (NULL for rejected)
    source_slot          BIGINT,
    source_block_time    TIMESTAMPTZ,
    received_at          TIMESTAMPTZ NOT NULL,
    decided_at           TIMESTAMPTZ NOT NULL,
    code_revision        TEXT NOT NULL,       -- git commit hash
    config_hash          TEXT NOT NULL,       -- SelectionConfig::hash()
    roster_hash          TEXT NOT NULL DEFAULT '',
    simulated_fill_model_version TEXT,        -- C3: e.g. 'v1-delayed-requote'
    created_at           TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_decision_records_run_id ON decision_records (run_id);
CREATE INDEX IF NOT EXISTS idx_decision_records_wallet ON decision_records (wallet_address);
CREATE INDEX IF NOT EXISTS idx_decision_records_trade ON decision_records (trade_uuid) WHERE trade_uuid IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_decision_records_admitted ON decision_records (admitted);
CREATE INDEX IF NOT EXISTS idx_decision_records_decided ON decision_records (decided_at);
CREATE INDEX IF NOT EXISTS idx_decision_records_ingress ON decision_records (ingress);
