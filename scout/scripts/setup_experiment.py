#!/usr/bin/env python3
"""
Setup script for 21-day forward test.

Initializes the experiment environment:
- Verifies database schema
- Freezes roster with chronological anti-look-ahead split
- Sets T0 timestamp
- Creates experiment manifest record
"""

import sqlite3
import json
import argparse
import sys
from datetime import datetime
from pathlib import Path

def setup_experiment(
    db_path: str,
    config_path: str,
    force: bool = False
):
    """Setup experiment environment."""
    
    print("🚀 Setting up 21-day forward test...")
    
    # Connect to database
    conn = sqlite3.connect(db_path)
    cursor = conn.cursor()
    
    # Verify experiment tables exist
    print("✓ Verifying database schema...")
    cursor.execute("""
        SELECT name FROM sqlite_master 
        WHERE type='table' AND name='experiment_trades'
    """)
    
    if cursor.fetchone() is None:
        print("❌ Error: experiment_trades table not found")
        print("Run: sqlite3 {} < operator/src/db/experiment_schema.sql".format(db_path))
        sys.exit(1)
    
    # Check for existing running experiment
    print("✓ Checking for existing experiments...")
    cursor.execute("""
        SELECT run_id, status, start_time FROM experiment_manifest 
        WHERE status = 'running' LIMIT 1
    """)
    
    existing = cursor.fetchone()
    if existing and not force:
        print(f"❌ Error: Experiment already running")
        print(f"Run ID: {existing[0]}")
        print(f"Status: {existing[1]}")
        print(f"Started: {existing[2]}")
        print("Use --force to override")
        sys.exit(1)
    
    # Load experiment configuration
    print("✓ Loading experiment configuration...")
    with open(config_path) as f:
        config = json.load(f)
    
    experiment_config = config.get('experiment', {})
    
    # Set T0 timestamp
    t0 = datetime.utcnow().isoformat() + 'Z'
    print(f"✓ Setting T0: {t0}")
    
    # Freeze roster snapshot
    print("✓ Freezing roster snapshot...")
    cursor.execute("SELECT address, wqs_score, roi FROM wallets ORDER BY wqs_score DESC LIMIT 100")
    roster_snapshot = [
        {'address': row[0], 'wqs_score': row[1], 'roi': row[2]}
        for row in cursor.fetchall()
    ]
    
    # Apply chronological anti-look-ahead split
    print("✓ Applying chronological anti-look-ahead split...")
    t0_wallets = roster_snapshot[:len(roster_snapshot)//2]
    t1_wallets = roster_snapshot[len(roster_snapshot)//2:]
    
    print(f"  T0 wallet pool: {len(t0_wallets)} wallets")
    print(f"  T1 wallet pool: {len(t1_wallets)} wallets")
    
    # Create experiment manifest
    print("✓ Creating experiment manifest...")
    run_id = f"ft-{datetime.utcnow().strftime('%Y%m%d-%H%M%S')}"
    
    cursor.execute("""
        INSERT INTO experiment_manifest (
            run_id, t0, roster_snapshot, settings,
            status, start_time, total_trades, tracer_trades,
            toxic_wallets, total_wallets
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    """, (
        run_id,
        t0,
        json.dumps({
            'roster': roster_snapshot,
            't0_wallets': t0_wallets,
            't1_wallets': t1_wallets
        }),
        json.dumps(experiment_config),
        'running',
        datetime.utcnow().isoformat(),
        0,
        0,
        0,
        len(roster_snapshot)
    ))
    
    conn.commit()
    
    # Initialize credit tracking
    print("✓ Initializing credit tracking...")
    monthly_budget = experiment_config.get('credit_budget', {}).get('monthly', 10000000)
    daily_budget = monthly_budget / 30  # Approximate daily budget
    
    cursor.execute("""
        INSERT INTO experiment_credits (
            timestamp, credits_used, operation_type,
            daily_budget, monthly_budget, projected_total, run_id
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
    """, (
        datetime.utcnow().isoformat(),
        0,
        'initialization',
        daily_budget,
        monthly_budget,
        monthly_budget,  # Initial projection
        run_id
    ))
    
    conn.commit()
    
    # Summary
    print("\n" + "="*60)
    print("✅ EXPERIMENT SETUP COMPLETE")
    print("="*60)
    print(f"Run ID: {run_id}")
    print(f"T0: {t0}")
    print(f"Experiment Duration: {experiment_config.get('experiment_days', 21)} days")
    print(f"Minimum Trades: {experiment_config.get('min_trades', 50)}")
    print(f"Tracer Cap: {experiment_config.get('tracer_cap', 60)}")
    print(f"Sample Rate: {experiment_config.get('tracer_sample_rate', 1.0) * 100}%")
    print(f"Credit Budget: {monthly_budget:,} credits/month")
    print("="*60)
    print("\n🎯 Ready to start experiment")
    print(f"Run: ./run-forward-test.sh")
    print(f"Or: operator/target/release/chimera_operator --config {config_path} --mode paper --experiment-enabled")
    
    return run_id

def main():
    parser = argparse.ArgumentParser(description='Setup 21-day forward test')
    parser.add_argument('--db-path', default='operator/data/chimera.db', help='Path to database')
    parser.add_argument('--config', default='config/experiment.yaml', help='Experiment config file')
    parser.add_argument('--force', action='store_true', help='Force setup even if experiment running')
    
    args = parser.parse_args()
    
    # Check paths exist
    if not Path(args.db_path).exists():
        print(f"❌ Error: Database not found: {args.db_path}")
        sys.exit(1)
    
    if not Path(args.config).exists():
        print(f"❌ Error: Config not found: {args.config}")
        sys.exit(1)
    
    try:
        setup_experiment(
            db_path=args.db_path,
            config_path=args.config,
            force=args.force
        )
    except Exception as e:
        print(f"❌ Error: {e}")
        sys.exit(1)

if __name__ == '__main__':
    main()
