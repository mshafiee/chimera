//! Simple binary to trigger roster merge
//! Usage: cargo run --bin merge_roster [--roster-path /path/to/roster_new.db] [--db-path /path/to/chimera.db]

use chimera_operator::db;
use chimera_operator::roster;
use std::path::PathBuf;

fn parse_args() -> (PathBuf, PathBuf) {
    let args: Vec<String> = std::env::args().collect();
    let mut roster_path = PathBuf::from("data/roster_new.db");
    let mut db_path = PathBuf::from("data/chimera.db");

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--roster-path" => {
                if i + 1 < args.len() {
                    roster_path = PathBuf::from(&args[i + 1]);
                    i += 2;
                } else {
                    eprintln!("ERROR: --roster-path requires a value");
                    std::process::exit(1);
                }
            }
            "--db-path" => {
                if i + 1 < args.len() {
                    db_path = PathBuf::from(&args[i + 1]);
                    i += 2;
                } else {
                    eprintln!("ERROR: --db-path requires a value");
                    std::process::exit(1);
                }
            }
            "--help" | "-h" => {
                println!("Usage: merge_roster [--roster-path PATH] [--db-path PATH]");
                println!("  --roster-path PATH  Path to roster_new.db (default: data/roster_new.db)");
                println!("  --db-path PATH      Path to chimera.db (default: data/chimera.db)");
                std::process::exit(0);
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    (roster_path, db_path)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    let (roster_path, db_path) = parse_args();

    println!("=== Chimera Roster Merge ===");
    println!("Roster file: {}", roster_path.display());
    println!("Database: {}", db_path.display());
    println!();

    // Check if roster file exists
    if !roster_path.exists() {
        eprintln!("ERROR: Roster file not found at {}", roster_path.display());
        std::process::exit(1);
    }

    // Initialize database pool
    let db_config = db::DatabaseConfig {
        path: db_path.clone(),
        max_connections: 5,
    };
    let pool = db::init_pool(&db_config).await?;

    // Perform merge
    println!("Starting roster merge...");
    match roster::merge_roster(&pool, &roster_path).await {
        Ok(result) => {
            println!("✓ Merge completed successfully!");
            println!("  Wallets merged: {}", result.wallets_merged);
            println!("  Wallets removed: {}", result.wallets_removed);
            println!("  Integrity check: {}", if result.integrity_ok { "PASSED" } else { "FAILED" });
            
            if !result.warnings.is_empty() {
                println!("  Warnings:");
                for warning in &result.warnings {
                    println!("    - {}", warning);
                }
            }
            
            Ok(())
        }
        Err(e) => {
            eprintln!("ERROR: Roster merge failed: {}", e);
            std::process::exit(1);
        }
    }
}
