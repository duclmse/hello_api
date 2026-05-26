//! Phase 14 — SQLite Pack integration tests.
//!
//! Verifies:
//! - CREATE TABLE / INSERT / SELECT round-trip.
//! - Positional bind parameters (?, typed values).
//! - `execute` returns correct row-changed count.
//! - `query` returns correct result rows.
//! - In-memory DB persists across sequential runs on the same slot.
//! - In-memory DB is cleared when a slot is recycled.
//! - SQL errors propagate as `SandboxError::Runtime`.
//! - Multiple column types (integer, real, text, null) survive round-trips.
//! - File-backed mode creates and opens the database.
//!
//! All pool tests use `pool_size = 1` (V8 single-thread constraint).

use std::collections::HashMap;

use hello_sandbox::loader::AllowlistModuleLoaderBuilder;
use hello_sandbox::sdk::core_sdk::CorePack;
use hello_sandbox::sdk::sqlite_sdk::SqlitePack;
use hello_sandbox::sdk::SdkRegistry;
use hello_sandbox::{PoolConfig, SandboxConfig, SandboxError};
use serde_json::json;
use tokio::task::LocalSet;

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn one_slot() -> PoolConfig {
    PoolConfig {
        pool_size: 1,
        ..PoolConfig::default()
    }
}

fn sqlite_pool() -> hello_sandbox::RuntimePool {
    let sdk = SdkRegistry::empty().register(CorePack).register(SqlitePack::new());
    hello_sandbox::RuntimePool::new(
        one_slot(),
        SandboxConfig::trusted(),
        AllowlistModuleLoaderBuilder::default(),
        sdk,
    )
}

fn run_sync<F, T>(f: F) -> T
where
    F: std::future::Future<Output = T>,
{
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = LocalSet::new();
    local.block_on(&rt, f)
}

// ─── Basic DDL and DML ───────────────────────────────────────────────────────

#[test]
fn create_table_and_insert() {
    run_sync(async {
        let pool = sqlite_pool();

        let result = pool
            .run(
                r#"
                import { db } from "sandbox:sqlite";
                db.execute("CREATE TABLE greet (name TEXT)");
                const n = db.execute("INSERT INTO greet VALUES (?)", "Alice");
                return n;
                "#,
                HashMap::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.value, json!(1), "INSERT should report 1 changed row");
    });
}

#[test]
fn query_returns_inserted_rows() {
    run_sync(async {
        let pool = sqlite_pool();

        let result = pool
            .run(
                r#"
                import { db } from "sandbox:sqlite";
                db.execute("CREATE TABLE nums (v INTEGER)");
                db.execute("INSERT INTO nums VALUES (?)", 10);
                db.execute("INSERT INTO nums VALUES (?)", 20);
                db.execute("INSERT INTO nums VALUES (?)", 30);
                return db.query("SELECT v FROM nums ORDER BY v");
                "#,
                HashMap::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.value, json!([[10], [20], [30]]));
    });
}

#[test]
fn multiple_columns_returned() {
    run_sync(async {
        let pool = sqlite_pool();

        let result = pool
            .run(
                r#"
                import { db } from "sandbox:sqlite";
                db.execute("CREATE TABLE person (id INTEGER, name TEXT, score REAL)");
                db.execute("INSERT INTO person VALUES (?, ?, ?)", 1, "Bob", 9.5);
                return db.query("SELECT id, name, score FROM person");
                "#,
                HashMap::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.value, json!([[1, "Bob", 9.5]]));
    });
}

#[test]
fn null_bind_param_roundtrips() {
    run_sync(async {
        let pool = sqlite_pool();

        let result = pool
            .run(
                r#"
                import { db } from "sandbox:sqlite";
                db.execute("CREATE TABLE t (x TEXT)");
                db.execute("INSERT INTO t VALUES (?)", null);
                return db.query("SELECT x FROM t");
                "#,
                HashMap::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.value, json!([[null]]));
    });
}

#[test]
fn boolean_bind_param_stored_as_integer() {
    run_sync(async {
        let pool = sqlite_pool();

        let result = pool
            .run(
                r#"
                import { db } from "sandbox:sqlite";
                db.execute("CREATE TABLE t (flag INTEGER)");
                db.execute("INSERT INTO t VALUES (?)", true);
                db.execute("INSERT INTO t VALUES (?)", false);
                return db.query("SELECT flag FROM t ORDER BY flag");
                "#,
                HashMap::new(),
            )
            .await
            .unwrap();

        // true → 1, false → 0
        assert_eq!(result.value, json!([[0], [1]]));
    });
}

// ─── Persistence across runs on same slot ────────────────────────────────────

#[test]
fn db_persists_across_sequential_runs_on_same_slot() {
    run_sync(async {
        let pool = sqlite_pool();

        // Run 1: create table and insert a row.
        pool.run(
            r#"
            import { db } from "sandbox:sqlite";
            db.execute("CREATE TABLE counter (n INTEGER)");
            db.execute("INSERT INTO counter VALUES (1)");
            return "ok";
            "#,
            HashMap::new(),
        )
        .await
        .unwrap();

        // Run 2: table and data are still present; insert another row.
        let result = pool
            .run(
                r#"
                import { db } from "sandbox:sqlite";
                db.execute("INSERT INTO counter VALUES (2)");
                return db.query("SELECT n FROM counter ORDER BY n");
                "#,
                HashMap::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.value, json!([[1], [2]]), "data from run 1 must be visible in run 2");
    });
}

// ─── Slot recycle clears the DB ───────────────────────────────────────────────

#[test]
fn db_cleared_after_slot_recycle() {
    run_sync(async {
        // max_runs_per_slot = 1 forces a recycle after the first run.
        let sdk = SdkRegistry::empty().register(CorePack).register(SqlitePack::new());
        let pool = hello_sandbox::RuntimePool::new(
            PoolConfig {
                pool_size: 1,
                max_runs_per_slot: 1,
                ..PoolConfig::default()
            },
            SandboxConfig::trusted(),
            AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        // Run 1: create table and insert.
        pool.run(
            r#"
            import { db } from "sandbox:sqlite";
            db.execute("CREATE TABLE t (v INTEGER)");
            db.execute("INSERT INTO t VALUES (99)");
            return "ok";
            "#,
            HashMap::new(),
        )
        .await
        .unwrap();

        // After max_runs_per_slot=1 the slot is recycled, so the second run
        // gets a fresh in-memory DB — the table no longer exists.
        let err = pool
            .run(
                r#"
                import { db } from "sandbox:sqlite";
                return db.query("SELECT v FROM t");
                "#,
                HashMap::new(),
            )
            .await
            .unwrap_err();

        match err {
            SandboxError::Runtime(_) => {}, // "no such table: t" → runtime error
            other => panic!("expected Runtime error after recycle, got: {other:?}"),
        }
    });
}

// ─── execute() row-count ──────────────────────────────────────────────────────

#[test]
fn execute_returns_correct_changed_count() {
    run_sync(async {
        let pool = sqlite_pool();

        let result = pool
            .run(
                r#"
                import { db } from "sandbox:sqlite";
                db.execute("CREATE TABLE t (x INTEGER)");
                db.execute("INSERT INTO t VALUES (1)");
                db.execute("INSERT INTO t VALUES (2)");
                db.execute("INSERT INTO t VALUES (3)");
                // UPDATE all 3 rows.
                const changed = db.execute("UPDATE t SET x = x + 10");
                return changed;
                "#,
                HashMap::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.value, json!(3));
    });
}

// ─── SQL error propagates ─────────────────────────────────────────────────────

#[test]
fn sql_error_returns_runtime_error() {
    run_sync(async {
        let pool = sqlite_pool();

        let err = pool
            .run(
                r#"
                import { db } from "sandbox:sqlite";
                db.query("SELECT * FROM nonexistent_table");
                return "ok";
                "#,
                HashMap::new(),
            )
            .await
            .unwrap_err();

        match err {
            SandboxError::Runtime(_) => {}, // expected: "no such table"
            other => panic!("expected Runtime error, got: {other:?}"),
        }
    });
}

// ─── Parameterised queries (WHERE clause) ────────────────────────────────────

#[test]
fn parameterised_where_clause() {
    run_sync(async {
        let pool = sqlite_pool();

        let result = pool
            .run(
                r#"
                import { db } from "sandbox:sqlite";
                db.execute("CREATE TABLE items (name TEXT, qty INTEGER)");
                db.execute("INSERT INTO items VALUES (?, ?)", "apple", 5);
                db.execute("INSERT INTO items VALUES (?, ?)", "banana", 2);
                db.execute("INSERT INTO items VALUES (?, ?)", "cherry", 8);
                return db.query("SELECT name FROM items WHERE qty > ? ORDER BY name", 4);
                "#,
                HashMap::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.value, json!([["apple"], ["cherry"]]));
    });
}

// ─── File-backed mode ─────────────────────────────────────────────────────────

#[test]
fn file_backed_db_persists_across_pool_instances() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    run_sync(async {
        // Pool 1: create table and insert.
        {
            let sdk =
                SdkRegistry::empty().register(CorePack).register(SqlitePack::new_file(&db_path));
            let pool = hello_sandbox::RuntimePool::new(
                PoolConfig {
                    pool_size: 1,
                    ..PoolConfig::default()
                },
                SandboxConfig::trusted(),
                AllowlistModuleLoaderBuilder::default(),
                sdk,
            );

            pool.run(
                r#"
                import { db } from "sandbox:sqlite";
                db.execute("CREATE TABLE IF NOT EXISTS kv (k TEXT, v TEXT)");
                db.execute("INSERT INTO kv VALUES (?, ?)", "hello", "world");
                return "ok";
                "#,
                HashMap::new(),
            )
            .await
            .unwrap();
        } // pool1 dropped — file remains

        // Pool 2: open the same file, data should still be there.
        {
            let sdk =
                SdkRegistry::empty().register(CorePack).register(SqlitePack::new_file(&db_path));
            let pool = hello_sandbox::RuntimePool::new(
                PoolConfig {
                    pool_size: 1,
                    ..PoolConfig::default()
                },
                SandboxConfig::trusted(),
                AllowlistModuleLoaderBuilder::default(),
                sdk,
            );

            let result = pool
                .run(
                    r#"
                    import { db } from "sandbox:sqlite";
                    return db.query("SELECT v FROM kv WHERE k = ?", "hello");
                    "#,
                    HashMap::new(),
                )
                .await
                .unwrap();

            assert_eq!(result.value, json!([["world"]]));
        }
    });
}

// ─── Aggregate query ──────────────────────────────────────────────────────────

#[test]
fn aggregate_query_count_and_sum() {
    run_sync(async {
        let pool = sqlite_pool();

        let result = pool
            .run(
                r#"
                import { db } from "sandbox:sqlite";
                db.execute("CREATE TABLE sales (amount REAL)");
                db.execute("INSERT INTO sales VALUES (?)", 10.0);
                db.execute("INSERT INTO sales VALUES (?)", 20.5);
                db.execute("INSERT INTO sales VALUES (?)", 5.25);
                return db.query("SELECT COUNT(*), SUM(amount) FROM sales");
                "#,
                HashMap::new(),
            )
            .await
            .unwrap();

        let rows = result.value.as_array().unwrap();
        assert_eq!(rows.len(), 1);
        let row = rows[0].as_array().unwrap();
        assert_eq!(row[0], json!(3)); // COUNT(*)
        assert!((row[1].as_f64().unwrap() - 35.75).abs() < 1e-9); // SUM
    });
}
