-- Add wqs_confidence column to wallets table
-- This stores the sample confidence 0-1, unbundled from wqs_score
ALTER TABLE wallets ADD COLUMN wqs_confidence REAL;