-- Price-path reconstruction for the copy-engine replay harness (Phase 2A/2B).
--
-- `price_path_points` caches the reconstructed per-token price series (payable
-- SOL per token) from on-chain swaps, so Helius is not re-fetched 16k times per
-- run. `price_path_fidelity` records the cross-check agreement between the
-- reconstructed path and an external OHLCV provider, so a grid-search only
-- trusts positions whose path passes the fidelity gate.

-- One row per (token, ts) reconstructed price point.
CREATE TABLE IF NOT EXISTS price_path_points (
    id          BIGSERIAL PRIMARY KEY,
    token_address TEXT NOT NULL,
    ts_unix     BIGINT NOT NULL,
    payable_sol NUMERIC(30,18) NOT NULL,
    UNIQUE(token_address, ts_unix)
);

CREATE INDEX IF NOT EXISTS idx_price_path_token_ts ON price_path_points (token_address, ts_unix);

-- One row per token: fidelity of the reconstructed path vs an OHLCV provider.
CREATE TABLE IF NOT EXISTS price_path_fidelity (
    token_address   TEXT PRIMARY KEY,
    provider        TEXT,                -- e.g. 'birdeye' | 'geckoterminal'
    provider_n      INT,                 -- matched candle points
    pearson_corr    DOUBLE PRECISION,
    mape            DOUBLE PRECISION,    -- mean absolute % error
    pass            BOOLEAN NOT NULL DEFAULT FALSE,
    checked_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_price_path_fid_pass ON price_path_fidelity (pass);
