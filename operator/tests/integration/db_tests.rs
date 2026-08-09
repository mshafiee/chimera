//! Database Integration Tests (PostgreSQL)
//!
//! SQLite-era WAL/PRAGMA tests were removed when SQLite was decommissioned.
//! These tests cover PostgreSQL-native pool behavior: concurrent reads and
//! serialized concurrent writes through the shared pool.

use chimera_operator::db_abstraction::Database;
use sqlx::Pool;
use sqlx::Postgres;
use std::sync::Arc;

#[path = "../common/mod.rs"]
mod common;

fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    common::pg_pool(db)
}

#[tokio::test]
async fn test_concurrent_reads() {
    // Each test gets its own isolated database (dropped on teardown), so
    // concurrent tests never share or race on schema/state.
    let (db, _guard) = common::create_test_db().await;
    let pool = pg_pool(&db);

    // Spawn multiple concurrent read queries through the shared pool.
    let mut handles = Vec::new();
    for _ in 0..8 {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            let row: (i32,) = sqlx::query_as("SELECT 1").fetch_one(&pool).await.unwrap();
            row.0
        }));
    }

    for handle in handles {
        let value = handle.await.unwrap();
        assert_eq!(value, 1);
    }
}

#[tokio::test]
async fn test_concurrent_writes_serialized() {
    let (db, _guard) = common::create_test_db().await;
    let pool = pg_pool(&db);

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS concurrency_counter (id INT PRIMARY KEY, value INT NOT NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();
    // Reset the counter explicitly so the test starts from a known state even
    // if a previous (failed) run left a row behind.
    sqlx::query("INSERT INTO concurrency_counter (id, value) VALUES (1, 0) ON CONFLICT (id) DO UPDATE SET value = 0")
        .execute(&pool)
        .await
        .unwrap();

    // Concurrent increments: PostgreSQL serializes row updates; final count
    // must equal the number of writers (no lost updates).
    let mut handles = Vec::new();
    for _ in 0..10 {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            sqlx::query("UPDATE concurrency_counter SET value = value + 1 WHERE id = 1")
                .execute(&pool)
                .await
                .unwrap();
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }

    let (value,): (i32,) = sqlx::query_as("SELECT value FROM concurrency_counter WHERE id = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(value, 10, "concurrent increments must not be lost");

    sqlx::query("DROP TABLE IF EXISTS concurrency_counter")
        .execute(&pool)
        .await
        .unwrap();
}
