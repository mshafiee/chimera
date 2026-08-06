-- Shadow Paper Trader: trade every signal for later evaluation.
-- Tracks positions under 5 parallel exit strategies to measure
-- false positives, lost profits, and fine-tune selection gates.

-- One row per BUY signal received (admitted or rejected by the main system).
CREATE TABLE IF NOT EXISTS shadow_positions (
    id                  BIGSERIAL PRIMARY KEY,
    shadow_id           TEXT NOT NULL UNIQUE,
    decision_id         TEXT,
    run_id              TEXT,
    wallet_address      TEXT NOT NULL,
    token_address       TEXT NOT NULL,
    token_symbol        TEXT,
    strategy            TEXT,
    main_admitted       BOOLEAN NOT NULL,
    main_rejection_code TEXT,
    main_rejection_reason TEXT,
    entry_amount_sol    NUMERIC(30,18) NOT NULL DEFAULT 0.1,
    entry_price_usd     NUMERIC(30,18),
    entry_sol_price_usd NUMERIC(30,18),
    wqs                 DOUBLE PRECISION,
    quality_score       DOUBLE PRECISION,
    liquidity_usd       NUMERIC(30,18),
    consensus_wallet_count INTEGER,
    ingress             TEXT NOT NULL,
    opened_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    fully_closed        BOOLEAN NOT NULL DEFAULT FALSE,
    closed_at           TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_shadow_pos_token ON shadow_positions (token_address);
CREATE INDEX IF NOT EXISTS idx_shadow_pos_wallet ON shadow_positions (wallet_address);
CREATE INDEX IF NOT EXISTS idx_shadow_pos_open ON shadow_positions (opened_at DESC);
CREATE INDEX IF NOT EXISTS idx_shadow_pos_active ON shadow_positions (fully_closed) WHERE fully_closed = FALSE;
CREATE INDEX IF NOT EXISTS idx_shadow_pos_decision ON shadow_positions (decision_id) WHERE decision_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_shadow_pos_wallet_token ON shadow_positions (wallet_address, token_address);

-- One row per (shadow_position, exit_strategy) pair.
-- UNIQUE constraint ensures each strategy fires at most once per position.
CREATE TABLE IF NOT EXISTS shadow_exits (
    id                  BIGSERIAL PRIMARY KEY,
    shadow_id           TEXT NOT NULL REFERENCES shadow_positions(shadow_id) ON DELETE CASCADE,
    exit_strategy       TEXT NOT NULL,
    exit_price_usd      NUMERIC(30,18),
    exit_sol_price_usd  NUMERIC(30,18),
    pnl_pct             NUMERIC(20,10),
    pnl_sol             NUMERIC(30,18),
    exit_reason         TEXT,
    hold_duration_secs  BIGINT,
    exited_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(shadow_id, exit_strategy)
);

CREATE INDEX IF NOT EXISTS idx_shadow_exits_shadow ON shadow_exits (shadow_id);
CREATE INDEX IF NOT EXISTS idx_shadow_exits_strategy ON shadow_exits (exit_strategy);
CREATE INDEX IF NOT EXISTS idx_shadow_exits_exited ON shadow_exits (exited_at DESC);

-- Comparison view: joins shadow positions with decision metadata and
-- classifies each exit as correct_admission, false_positive, lost_profit,
-- or correct_rejection.
CREATE OR REPLACE VIEW shadow_comparison AS
SELECT
    sp.shadow_id,
    sp.decision_id,
    sp.wallet_address,
    sp.token_address,
    sp.token_symbol,
    sp.strategy,
    sp.main_admitted,
    sp.main_rejection_code,
    sp.wqs,
    sp.entry_amount_sol,
    sp.entry_price_usd,
    sp.opened_at,
    se.exit_strategy,
    se.exit_price_usd,
    se.pnl_pct,
    se.pnl_sol,
    se.exit_reason,
    se.hold_duration_secs,
    se.exited_at,
    CASE
        WHEN sp.main_admitted  AND se.pnl_sol >= 0 THEN 'correct_admission'
        WHEN sp.main_admitted  AND se.pnl_sol <  0 THEN 'false_positive'
        WHEN NOT sp.main_admitted AND se.pnl_sol >  0 THEN 'lost_profit'
        WHEN NOT sp.main_admitted AND se.pnl_sol <= 0 THEN 'correct_rejection'
    END AS classification
FROM shadow_positions sp
JOIN shadow_exits se ON sp.shadow_id = se.shadow_id;

-- Aggregated summary by rejection gate (or ADMITTED for signals the
-- main system traded), broken down by exit strategy.
CREATE OR REPLACE VIEW shadow_summary_by_gate AS
SELECT
    COALESCE(sp.main_rejection_code, 'ADMITTED') AS gate,
    se.exit_strategy,
    COUNT(*) AS signal_count,
    COUNT(*) FILTER (WHERE se.pnl_sol > 0) AS winners,
    COUNT(*) FILTER (WHERE se.pnl_sol <= 0) AS losers,
    AVG(se.pnl_pct) AS avg_pnl_pct,
    SUM(se.pnl_sol) AS total_pnl_sol,
    AVG(se.hold_duration_secs) AS avg_hold_secs
FROM shadow_positions sp
JOIN shadow_exits se ON sp.shadow_id = se.shadow_id
GROUP BY COALESCE(sp.main_rejection_code, 'ADMITTED'), se.exit_strategy
ORDER BY gate, se.exit_strategy;
