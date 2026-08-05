-- Per-wallet exit profiles derived from on-chain round trips.
-- Raw stats only; effective exit params are computed at read time via
-- Bayesian shrinkage against the global ProfitManagementConfig.
CREATE TABLE IF NOT EXISTS wallet_exit_profiles (
    wallet_address   TEXT PRIMARY KEY REFERENCES wallets(address) ON DELETE CASCADE,
    samples          INTEGER NOT NULL DEFAULT 0,          -- round trips used
    median_hold_secs BIGINT,                              -- buy->sell hold time
    avg_hold_secs    BIGINT,
    win_rate_pct     DOUBLE PRECISION,
    median_win_pct   DOUBLE PRECISION,
    median_loss_pct  DOUBLE PRECISION,
    avg_win_pct      DOUBLE PRECISION,
    avg_loss_pct     DOUBLE PRECISION,
    profit_factor    DOUBLE PRECISION,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_wallet_exit_profiles_samples
    ON wallet_exit_profiles (samples DESC);
