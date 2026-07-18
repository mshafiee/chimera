-- Ensure circuit_breaker_state table has the singleton row
-- This fixes cases where the initial seed failed or the row was deleted
INSERT INTO circuit_breaker_state (id, state, updated_at)
VALUES (1, 'ACTIVE', NOW())
ON CONFLICT (id) DO NOTHING;