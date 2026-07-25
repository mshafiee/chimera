//! Roster Merge Contract Tests (PostgreSQL)
//!
//! SQLite ATTACH-based roster merging is intentionally unsupported on the
//! PostgreSQL backend (see `operator/src/roster.rs`). Scout roster data is
//! ingested via direct SQL imports instead. These tests pin that contract so
//! nobody silently re-introduces a half-working merge path.

use chimera_operator::db_abstraction::{create_database, Database, DatabaseConfig, DbPool};
use chimera_operator::roster::{merge_roster, validate_roster};
use sqlx::Pool;
use sqlx::Postgres;
use std::sync::Arc;
use tempfile::TempDir;

fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    match db.pool() {
        DbPool::PostgreSQL(pool) => pool,
        _ => panic!("test requires PostgreSQL backend"),
    }
}

async fn create_test_db() -> (Arc<dyn Database>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let config = DatabaseConfig::postgres(
        std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set"),
    );
    let db = create_database(&config).await.unwrap();
    db.run_migrations().await.unwrap();
    (db, temp_dir)
}

#[tokio::test]
async fn test_merge_roster_rejected_on_postgres() {
    let (db, temp_dir) = create_test_db().await;
    let pool = pg_pool(&db);
    let roster_path = temp_dir.path().join("roster_new.db");

    let result = merge_roster(&pool, &roster_path).await;
    assert!(result.is_err(), "merge_roster must be rejected on PostgreSQL");
    let msg = result.err().unwrap().to_string();
    assert!(
        msg.contains("not supported"),
        "error must explain the PostgreSQL rejection, got: {}",
        msg
    );
}

#[tokio::test]
async fn test_validate_roster_rejected_on_postgres() {
    let (db, temp_dir) = create_test_db().await;
    let pool = pg_pool(&db);
    let roster_path = temp_dir.path().join("roster_new.db");

    let result = validate_roster(&pool, &roster_path).await;
    assert!(
        result.is_err(),
        "validate_roster must be rejected on PostgreSQL"
    );
    let msg = result.err().unwrap().to_string();
    assert!(
        msg.contains("not supported"),
        "error must explain the PostgreSQL rejection, got: {}",
        msg
    );
}
