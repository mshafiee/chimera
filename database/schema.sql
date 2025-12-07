-- Chimera v7.1 Schema
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;
PRAGMA foreign_keys = ON;

CREATE TABLE trades (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trade_uuid TEXT NOT NULL UNIQUE,
    wallet_address TEXT NOT NULL,
    token_address TEXT NOT NULL,
    strategy TEXT NOT NULL CHECK(strategy IN ('SHIELD', 'SPEAR', 'EXIT')), 
    side TEXT NOT NULL CHECK(side IN ('BUY', 'SELL')),
    amount_sol REAL NOT NULL,
    status TEXT NOT NULL DEFAULT 'PENDING',
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
-- (Full schema from PDD v7.1 should be applied here)
