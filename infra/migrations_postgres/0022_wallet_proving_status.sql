-- PROVING wallet status (2026-08-28, candidate-proving lane).
--
-- Discovery produces thousands of CANDIDATE wallets, but the operator only
-- processed ACTIVE wallets — shadow evidence (the promotion currency) could
-- never accrue for a candidate, so the promotion pipeline only recycled
-- previously-ACTIVE books (measured 2026-08-28: 10,954 candidates, 70 with
-- evidence, 4 promotable). PROVING wallets are processed by the operator in
-- shadow-only mode (decisions + shadow forks, never queued live), giving the
-- promoter fresh trailing evidence to judge.
ALTER TABLE wallets DROP CONSTRAINT wallets_status_check;
ALTER TABLE wallets ADD CONSTRAINT wallets_status_check
    CHECK (status IN ('ACTIVE', 'PROVING', 'CANDIDATE', 'REJECTED'));
