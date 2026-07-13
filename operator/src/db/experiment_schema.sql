-- Experiment trades table for forward test
-- Records all paper trades, tracer executions, control arms, and execution gaps

CREATE TABLE IF NOT EXISTS experiment_trades (
    -- Primary identification
    trade_uuid TEXT PRIMARY KEY,
    
    -- Trade metadata
    wallet TEXT NOT NULL,
    token TEXT NOT NULL,
    signal_side TEXT NOT NULL,  -- BUY/SELL
    strategy TEXT NOT NULL,     -- Shield/Spear
    
    -- Paper trade results
    paper_fill_price REAL,
    paper_pnl REAL,
    entry_latency_ms INTEGER,
    
    -- Real tracer execution results
    real_fill_price REAL,
    real_pnl REAL,
    execution_gap REAL,
    jito_tip_sol REAL,
    dex_fee_sol REAL,
    is_tracer INTEGER DEFAULT 0,
    
    -- Control arm results
    control_random_pnl REAL,
    sol_bench_pnl REAL,
    
    -- Toxic flow detection
    toxic_flag INTEGER DEFAULT 0,
    
    -- Timestamps
    entry_time TEXT NOT NULL,
    exit_time TEXT,
    
    -- Metadata
    created_at TEXT DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT DEFAULT CURRENT_TIMESTAMP
);

-- Indexes for common queries
CREATE INDEX IF NOT EXISTS idx_experiment_trades_wallet ON experiment_trades(wallet);
CREATE INDEX IF NOT EXISTS idx_experiment_trades_token ON experiment_trades(token);
CREATE INDEX IF NOT EXISTS idx_experiment_trades_entry_time ON experiment_trades(entry_time);
CREATE INDEX IF NOT EXISTS idx_experiment_trades_is_tracer ON experiment_trades(is_tracer);
CREATE INDEX IF NOT EXISTS idx_experiment_trades_toxic_flag ON experiment_trades(toxic_flag);
CREATE INDEX IF NOT EXISTS idx_experiment_trades_exit_time ON experiment_trades(exit_time) WHERE exit_time IS NOT NULL;

-- Experiment manifest table for run metadata
CREATE TABLE IF NOT EXISTS experiment_manifest (
    -- Primary identification
    run_id TEXT PRIMARY KEY,
    
    -- Run metadata
    t0 TEXT NOT NULL,                  -- Experiment start time
    roster_snapshot TEXT,             -- JSON of frozen roster
    
    -- Configuration snapshot
    settings TEXT NOT NULL,           -- JSON of all config settings
    credit_budget REAL,               -- Total budget allocated
    
    -- Experiment state
    status TEXT DEFAULT 'running',    -- running, completed, aborted
    start_time TEXT NOT NULL,
    end_time TEXT,
    
    -- Final verdict
    verdict TEXT,                     -- GO/KILL/INCONCLUSIVE
    verdict_time TEXT,
    verdict_reasons TEXT,             -- JSON array of reasons
    
    -- Statistics
    total_trades INTEGER DEFAULT 0,
    tracer_trades INTEGER DEFAULT 0,
    toxic_wallets INTEGER DEFAULT 0,
    total_wallets INTEGER DEFAULT 0,
    
    -- Metadata
    created_at TEXT DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT DEFAULT CURRENT_TIMESTAMP
);

-- Toxic wallet tracking table
CREATE TABLE IF NOT EXISTS toxic_wallets (
    -- Primary identification
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    
    -- Wallet identification
    wallet_address TEXT NOT NULL,
    
    -- Toxic flow metrics
    selection_roi REAL,              -- Historical ROI at promotion
    post_promotion_roi REAL,         -- Realized ROI after promotion
    local_top_entries INTEGER,       -- Count of local-top entries
    total_entries INTEGER,           -- Total entries for this wallet
    
    -- Toxic detection flags
    is_toxic INTEGER DEFAULT 0,
    toxic_reason TEXT,               -- 'roi_drop' or 'local_top_squeeze'
    detected_at TEXT,
    
    -- Experiment reference
    run_id TEXT,
    
    -- Metadata
    created_at TEXT DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT DEFAULT CURRENT_TIMESTAMP,
    
    FOREIGN KEY (run_id) REFERENCES experiment_manifest(run_id)
);

CREATE INDEX IF NOT EXISTS idx_toxic_wallets_address ON toxic_wallets(wallet_address);
CREATE INDEX IF NOT EXISTS idx_toxic_wallets_run_id ON toxic_wallets(run_id);
CREATE INDEX IF NOT EXISTS idx_toxic_wallets_is_toxic ON toxic_wallets(is_toxic);

-- Credit usage tracking table
CREATE TABLE IF NOT EXISTS experiment_credits (
    -- Primary identification
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    
    -- Time tracking
    timestamp TEXT NOT NULL,
    
    -- Credit usage
    credits_used INTEGER NOT NULL,
    operation_type TEXT NOT NULL,    -- 'discovery', 'monitoring', 'analysis', etc.
    
    -- Budget tracking
    daily_budget REAL,
    monthly_budget REAL,
    projected_total REAL,
    
    -- Experiment reference
    run_id TEXT,
    
    -- Metadata
    created_at TEXT DEFAULT CURRENT_TIMESTAMP,
    
    FOREIGN KEY (run_id) REFERENCES experiment_manifest(run_id)
);

CREATE INDEX IF NOT EXISTS idx_experiment_credits_timestamp ON experiment_credits(timestamp);
CREATE INDEX IF NOT EXISTS idx_experiment_credits_run_id ON experiment_credits(run_id);
CREATE INDEX IF NOT EXISTS idx_experiment_credits_operation ON experiment_credits(operation_type);
