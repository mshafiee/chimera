-- WQS-to-Actual-PnL correlation table (Phase 3a)
-- Written by the Rust Operator when it closes copy-trade positions,
-- read by the Python Scout to compute WQS predictive power.
-- Financial PnL values stored as TEXT (Decimal strings).

CREATE TABLE IF NOT EXISTS wqs_pnl_correlation (
    wallet_address TEXT PRIMARY KEY,
    wqs_score_at_promotion REAL NOT NULL,
    actual_copy_pnl_7d_sol TEXT,
    actual_copy_pnl_30d_sol TEXT,
    actual_copy_pnl_all_sol TEXT,
    copy_trade_count_7d INTEGER DEFAULT 0,
    copy_trade_count_30d INTEGER DEFAULT 0,
    copy_trade_count_all INTEGER DEFAULT 0,
    strategy TEXT NOT NULL DEFAULT 'SHIELD'
        CHECK(strategy IN ('SHIELD', 'SPEAR')),
    wqs_components_json TEXT,  -- JSON blob of component scores at promotion time
    promoted_at TEXT NOT NULL,
    last_updated_at TEXT NOT NULL
);
