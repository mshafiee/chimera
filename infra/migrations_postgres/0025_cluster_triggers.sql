-- Cluster trigger capture (Gate-1 retest forward window, 2026-09-06).
--
-- Records every cluster-trigger event observed in the pre-admission
-- smart_money_signals flow: >= 3 distinct tracked wallets buying the same
-- token within a trailing 12h window, dispersion-valid (some i<j<k with
-- span >= 180s and both gaps >= 5s — the held plan's subset rule).
-- The capture script (scout/scripts/cluster_trigger_capture.py) is
-- idempotent per (token_address, trigger_at).

CREATE TABLE IF NOT EXISTS cluster_triggers (
    id BIGSERIAL PRIMARY KEY,
    token_address TEXT NOT NULL,
    trigger_at TIMESTAMPTZ NOT NULL,
    wallet_count INT NOT NULL,
    arrival_signatures TEXT[] NOT NULL,
    entry_price_sol NUMERIC,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_cluster_trigger UNIQUE (token_address, trigger_at)
);

CREATE INDEX IF NOT EXISTS idx_cluster_triggers_time
    ON cluster_triggers (trigger_at DESC);
