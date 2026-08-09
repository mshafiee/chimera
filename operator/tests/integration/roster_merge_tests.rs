//! Roster Merge Contract Tests (PostgreSQL)
//!
//! SQLite ATTACH-based roster merging is intentionally unsupported on the
//! PostgreSQL backend (see `operator/src/roster.rs`). Scout roster data is
//! ingested via direct SQL imports instead. These tests pin that contract so
//! nobody silently re-introduces a half-working merge path.

use chimera_operator::db_abstraction::{Database, DbPool};
use chimera_operator::roster::{merge_roster, validate_roster};
use sqlx::Pool;
use sqlx::Postgres;
use std::sync::Arc;

#[path = "../common/mod.rs"]
mod common;

fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    common::pg_pool(db)
}

async fn create_test_db() -> (Arc<dyn Database>, common::TestDbGuard) {
    common::create_test_db().await
}

#[tokio::test]
async fn test_merge_roster_rejected_on_postgres() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let roster_path = std::env::temp_dir().join("roster_new.db");

    let result = merge_roster(&pool, &roster_path).await;
    assert!(
        result.is_err(),
        "merge_roster must be rejected on PostgreSQL"
    );
    let msg = result.err().unwrap().to_string();
    assert!(
        msg.contains("not supported with PostgreSQL"),
        "error must pin the PostgreSQL rejection, got: {}",
        msg
    );
}

#[tokio::test]
async fn test_validate_roster_rejected_on_postgres() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let roster_path = std::env::temp_dir().join("roster_new.db");

    let result = validate_roster(&pool, &roster_path).await;
    assert!(
        result.is_err(),
        "validate_roster must be rejected on PostgreSQL"
    );
    let msg = result.err().unwrap().to_string();
    assert!(
        msg.contains("not supported with PostgreSQL"),
        "error must pin the PostgreSQL rejection, got: {}",
        msg
    );
}

#[tokio::test]
async fn test_direct_sql_import_path() {
    // The supported ingestion path on PostgreSQL is direct SQL import: insert a
    // roster row and read it back through the same schema the operator uses.
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);

    let address = "roster-import-wallet-0001";
    sqlx::query("INSERT INTO wallets (address, status, wqs_score) VALUES ($1, 'CANDIDATE', 70.0)")
        .bind(address)
        .execute(&pool)
        .await
        .expect("direct SQL roster import must work");

    let (status,): (String,) = sqlx::query_as("SELECT status FROM wallets WHERE address = $1")
        .bind(address)
        .fetch_one(&pool)
        .await
        .expect("imported roster row must be readable");
    assert_eq!(status, "CANDIDATE");
}
