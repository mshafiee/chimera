-- C2: Append-only promotion audit trail.
-- Every promotion decision (promoted or near-threshold shadow) appends an
-- immutable episode row. Unlike wqs_pnl_correlation (which is upserted and
-- keeps only the latest promotion), this table preserves the full history
-- so WQS-to-PnL feedback is honest and re-evaluation never erases the
-- original promotion-time features.
--
-- episode_id defaults to uuid_generate_v4() (uuid-ossp is enabled in 0001;
-- pgcrypto/gen_random_uuid is not, so we reuse the already-loaded extension).

CREATE TABLE IF NOT EXISTS promotion_episodes (
    episode_id           TEXT PRIMARY KEY DEFAULT uuid_generate_v4()::text,
    wallet_address       TEXT NOT NULL,
    promoted_at          TIMESTAMPTZ NOT NULL,
    wqs                  DOUBLE PRECISION NOT NULL,
    wqs_confidence       DOUBLE PRECISION,
    components_json      JSONB,
    decision             TEXT NOT NULL DEFAULT 'promoted',  -- 'promoted' | 'shadow'
    policy_version       TEXT NOT NULL,
    code_revision        TEXT NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_promotion_episodes_wallet ON promotion_episodes (wallet_address);
CREATE INDEX IF NOT EXISTS idx_promotion_episodes_promoted ON promotion_episodes (promoted_at);
CREATE INDEX IF NOT EXISTS idx_promotion_episodes_decision ON promotion_episodes (decision);
