-- Add remaining_fraction column to exit_targets for persisted tiered exit tracking.
-- When the operator restarts, previously tracked partial-exit fractions are lost
-- without this column (profit_targets.rs resets to 1.0 on fresh state creation).
ALTER TABLE exit_targets ADD COLUMN remaining_fraction REAL NOT NULL DEFAULT 1.0;
