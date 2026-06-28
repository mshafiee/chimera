# Test Harness Guide

This guide explains how to use the backend-agnostic test harness for testing Chimera Operator against both SQLite and PostgreSQL backends.

## Overview

The test harness provides a unified interface for creating test databases and running tests against both SQLite and PostgreSQL backends. This is critical for validating that the 66 PostgreSQL port methods (Phase 2) work identically to their SQLite counterparts.

## Location

The test harness is located at `operator/tests/common/mod.rs`.

## Functions

### `create_test_db()`

Creates a SQLite test database with migrations applied.

```rust
use common::create_test_db;

#[tokio::test]
async fn my_test() {
    let (db, _temp_dir) = create_test_db().await;
    // db is Arc<dyn Database> - use Database trait methods
}
```

**Returns:** `(Arc<dyn Database>, TempDir)` - The temp directory is automatically cleaned up when dropped.

### `create_test_pg_db()`

Creates a PostgreSQL test database with migrations applied.

**Requirements:**
- `postgres` feature must be enabled: `--features postgres`
- `TEST_DATABASE_URL` environment variable must be set

```rust
#[cfg(feature = "postgres")]
#[tokio::test]
async fn my_pg_test() {
    let (db, _temp_dir) = common::create_test_pg_db().await;
    // db is Arc<dyn Database> - use Database trait methods
}
```

**Returns:** `(Arc<dyn Database>, TempDir)` - The temp directory is automatically cleaned up when dropped.

### `create_test_db_from_env()`

Auto-selects backend based on environment. Returns SQLite by default, or PostgreSQL if `TEST_DATABASE_URL` is set and the `postgres` feature is enabled.

```rust
#[tokio::test]
async fn my_parametrized_test() {
    let (db, _temp_dir, backend) = common::create_test_db_from_env().await;
    // backend is "sqlite" or "postgres"
    println!("Running on {} backend", backend);
}
```

**Returns:** `(Arc<dyn Database>, TempDir, String)` - The string indicates which backend was used.

### `sqlite_pool()`

Extracts the raw SQLite pool from a `Arc<dyn Database>` for direct SQL queries.

```rust
use common::sqlite_pool;
use sqlx::Pool;
use sqlx::Sqlite;

#[tokio::test]
async fn direct_sql_test() {
    let (db, _temp_dir) = common::create_test_db().await;
    let pool = sqlite_pool(&db);
    
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades")
        .fetch_one(&pool)
        .await
        .unwrap();
}
```

### `pg_pool()`

Extracts the raw PostgreSQL pool from a `Arc<dyn Database>` for direct SQL queries.

```rust
#[cfg(feature = "postgres")]
use common::pg_pool;

#[tokio::test]
#[ignore] // Requires TEST_DATABASE_URL
async fn direct_pg_test() {
    let (db, _temp_dir) = common::create_test_pg_db().await;
    let pool = pg_pool(&db);
    
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades")
        .fetch_one(&pool)
        .await
        .unwrap();
}
```

## Usage Examples

### Basic test (SQLite only)

```rust
mod common;

use chimera_operator::db_abstraction::Database;
use rust_decimal::Decimal;

#[tokio::test]
async fn test_wallet_operations() {
    let (db, _temp_dir) = common::create_test_db().await;
    
    // Use Database trait methods
    db.upsert_wallet(
        "test-wallet",
        Some(Decimal::from_str("55.0").unwrap()),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .expect("upsert_wallet should succeed");
}
```

### Parameterized test (both backends)

```rust
mod common;

#[tokio::test]
async fn test_wallet_operations() {
    let (db, _temp_dir, backend) = common::create_test_db_from_env().await;
    
    // Test runs against both backends depending on env
    let result = db.upsert_wallet("test-wallet", None, None, None, None, None, None, None, None).await;
    
    assert!(result.is_ok(), "Should work on {} backend", backend);
}
```

### Postgres-specific test

```rust
mod common;

#[tokio::test]
#[ignore] // Requires TEST_DATABASE_URL
async fn test_postgres_specific() {
    let (db, _temp_dir, backend) = common::create_test_db_from_env().await;
    
    assert_eq!(backend, "postgres", "This test only runs against Postgres");
    
    // Postgres-specific queries or logic
}
```

## Running Tests

### Run SQLite tests (default)

```bash
cargo test
# or specifically
cargo test --test integration_tests
cargo test --test phase0_validation_tests
```

### Run Postgres tests

```bash
# Set environment variable and enable postgres feature
TEST_DATABASE_URL="postgresql://user:pass@localhost/test_db" cargo test --test phase0_validation_tests --features postgres

# Run ignored Postgres-only tests
TEST_DATABASE_URL="postgresql://user:pass@localhost/test_db" cargo test --test phase0_validation_tests --features postgres -- --ignored

# Run all tests with Postgres
TEST_DATABASE_URL="postgresql://user:pass@localhost/test_db" cargo test --features postgres
```

### CI Integration

For CI, you can use Docker to spin up a Postgres instance:

```bash
# Start Postgres
docker run -d -p 5432:5432 -e POSTGRES_PASSWORD=postgres postgres:16

# Run tests
export TEST_DATABASE_URL="postgresql://postgres:postgres@localhost/postgres"
cargo test --features postgres
```

## Migrations

All test databases automatically run migrations via `db.run_migrations().await.unwrap()`. This ensures that the test database has the same schema as production.

## Cleanup

The `TempDir` returned by `create_test_db()` and `create_test_pg_db()` automatically cleans up the temporary database when it goes out of scope (Rust's Drop trait).

## Important Notes

1. **No Manual Cleanup Required:** The `TempDir` handles automatic cleanup
2. **Backend-Agnostic:** Tests should use the `Database` trait, not backend-specific methods
3. **Decimal Precision:** Use `rust_decimal::Decimal` for all financial values (AGENTS.md rule)
4. **Concurrent Tests:** Each test gets its own isolated database
5. **Feature Gates:** Postgres tests require the `postgres` feature to be enabled

## Phase 0 Validation

The test harness is validated by `operator/tests/phase0_validation_tests.rs`. These tests verify:

1. ✅ SQLite tests pass by default
2. ✅ Postgres tests pass when `TEST_DATABASE_URL` is set
3. ✅ Migrations run successfully on both backends
4. ✅ Database trait methods work identically on both backends
5. ✅ Decimal values round-trip correctly

Run the validation tests:

```bash
# SQLite
cargo test --test phase0_validation_tests

# Postgres
TEST_DATABASE_URL="postgresql://localhost/test" cargo test --test phase0_validation_tests --features postgres
```

## Troubleshooting

### Error: `TEST_DATABASE_URL must be set for Postgres tests`

**Solution:** Set the environment variable:
```bash
export TEST_DATABASE_URL="postgresql://user:pass@localhost/db_name"
```

### Error: `postgres feature is not enabled`

**Solution:** Enable the feature when running tests:
```bash
cargo test --features postgres
```

### Error: `Failed to connect to Postgres server`

**Solution:** Ensure Postgres is running and accessible:
```bash
# Check if Postgres is running
docker ps | grep postgres

# Or check connectivity
psql "postgresql://user:pass@localhost/db_name" -c "SELECT 1"
```