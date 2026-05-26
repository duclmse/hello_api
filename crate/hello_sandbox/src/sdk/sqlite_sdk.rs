//! SQLite pack — per-slot in-memory (or file-backed) SQL database.
//!
//! Exposes `sandbox.db.query(sql, ...params)` and
//! `sandbox.db.execute(sql, ...params)` to scripts.
//!
//! The database is opened once per pool slot in [`SqlitePack::inject_op_state`]
//! and lives for the lifetime of that slot.  When a slot is recycled the
//! in-memory database is automatically discarded (a fresh `Connection` is
//! opened for the replacement slot).
//!
//! # Modes
//!
//! - **In-memory** (`SqlitePack::new()`) — fresh empty database per slot;
//!   state is isolated between slots and cleared on recycle.
//! - **File-backed** (`SqlitePack::new_file(path)`) — all slots share the
//!   same on-disk database; the file persists across restarts.
//!   **Note:** file-backed mode will fail on `IsolationLevel::Untrusted`
//!   because the seccomp filter blocks `open()` syscalls.

use std::path::PathBuf;

use deno_core::{op2, OpDecl, OpState};
use deno_error::JsErrorBox;
use serde_json::Value;

use crate::sdk::SdkExtension;

// ─── Per-slot store ───────────────────────────────────────────────────────────

/// Per-slot SQLite state: a single open `Connection`.
///
/// Opened in [`SqlitePack::inject_op_state`] immediately after the runtime
/// slot is constructed.  For in-memory mode each slot gets an independent
/// empty database; for file-backed mode all slots share the same file.
pub struct SqliteStore {
    pub conn: rusqlite::Connection,
}

// ─── Type conversion helpers ──────────────────────────────────────────────────

/// Convert a [`serde_json::Value`] to a [`rusqlite::types::Value`] for use as
/// a bound parameter.
///
/// Objects and arrays are serialised to a JSON string so they round-trip
/// safely through the TEXT affinity column.
fn json_to_sql(v: &Value) -> rusqlite::types::Value {
    match v {
        Value::Null => rusqlite::types::Value::Null,
        Value::Bool(b) => rusqlite::types::Value::Integer(*b as i64),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                rusqlite::types::Value::Integer(i)
            } else {
                rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0))
            }
        },
        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        // Arrays and objects → compact JSON string.
        other => rusqlite::types::Value::Text(other.to_string()),
    }
}

/// Convert a [`rusqlite::types::Value`] from a query result back to
/// [`serde_json::Value`].
///
/// BLOBs are base64-encoded so they survive JSON serialisation.
fn sql_to_json(v: rusqlite::types::Value) -> Value {
    match v {
        rusqlite::types::Value::Null => Value::Null,
        rusqlite::types::Value::Integer(i) => Value::Number(i.into()),
        rusqlite::types::Value::Real(f) => {
            serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null)
        },
        rusqlite::types::Value::Text(s) => Value::String(s),
        rusqlite::types::Value::Blob(b) => {
            use base64::Engine;
            Value::String(base64::engine::general_purpose::STANDARD.encode(b))
        },
    }
}

// ─── Ops ─────────────────────────────────────────────────────────────────────

/// Execute a SELECT (or any statement that returns rows) and return the
/// results as a JSON array of row arrays.
///
/// Each element of the outer array is one row; each element of the inner
/// array is a column value in column-declaration order.
///
/// Parameters are passed as a JSON-encoded array of positional bind values.
#[op2]
#[serde]
fn op_db_query(
    state: &mut OpState,
    #[string] sql: String,
    #[string] params_json: String,
) -> Result<Vec<Vec<Value>>, JsErrorBox> {
    let params: Vec<Value> = serde_json::from_str(&params_json).unwrap_or_default();
    let sql_params: Vec<rusqlite::types::Value> = params.iter().map(json_to_sql).collect();
    let sql_param_refs: Vec<&dyn rusqlite::types::ToSql> =
        sql_params.iter().map(|v| v as &dyn rusqlite::types::ToSql).collect();

    let store = state.borrow::<SqliteStore>();
    let mut stmt = store.conn.prepare(&sql).map_err(|e| JsErrorBox::generic(e.to_string()))?;

    let col_count = stmt.column_count();
    let rows = stmt
        .query_map(sql_param_refs.as_slice(), |row| {
            (0..col_count)
                .map(|i| row.get::<_, rusqlite::types::Value>(i))
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|e| JsErrorBox::generic(e.to_string()))?
        .map(|r| {
            r.map(|row| row.into_iter().map(sql_to_json).collect::<Vec<_>>())
                .map_err(|e| JsErrorBox::generic(e.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// Execute an INSERT, UPDATE, DELETE, CREATE, or any DML/DDL statement and
/// return the number of rows changed.
///
/// Parameters are passed as a JSON-encoded array of positional bind values.
#[op2(fast)]
fn op_db_execute(
    state: &mut OpState,
    #[string] sql: String,
    #[string] params_json: String,
) -> Result<u32, JsErrorBox> {
    let params: Vec<Value> = serde_json::from_str(&params_json).unwrap_or_default();
    let sql_params: Vec<rusqlite::types::Value> = params.iter().map(json_to_sql).collect();
    let sql_param_refs: Vec<&dyn rusqlite::types::ToSql> =
        sql_params.iter().map(|v| v as &dyn rusqlite::types::ToSql).collect();

    let store = state.borrow::<SqliteStore>();
    let changed = store
        .conn
        .execute(&sql, sql_param_refs.as_slice())
        .map_err(|e| JsErrorBox::generic(e.to_string()))?;

    Ok(changed as u32)
}

// ─── Pack ─────────────────────────────────────────────────────────────────────

/// Backing storage mode for [`SqlitePack`].
enum SqliteStorage {
    /// In-process in-memory database (`:memory:`).
    Memory,
    /// File-backed database at the given path.
    File(PathBuf),
}

/// SQLite SDK pack — per-slot SQL database (`query`, `execute`).
///
/// In-memory mode (default via `SqlitePack::new()`) gives each pool slot an
/// independent empty database that is cleared when the slot is recycled.
///
/// File-backed mode (`SqlitePack::new_file(path)`) opens the same on-disk
/// file from every slot, enabling shared persistent storage.
///
/// # Examples
///
/// ```rust,ignore
/// // In-memory (fresh per slot):
/// SandboxBuilder::new().sdk(SqlitePack::new())
///
/// // File-backed (shared across slots, persists across restarts):
/// SandboxBuilder::new().sdk(SqlitePack::new_file("/var/db/sandbox.db"))
/// ```
pub struct SqlitePack {
    storage: SqliteStorage,
}

impl SqlitePack {
    /// Create a `SqlitePack` with an in-memory database.
    ///
    /// Each pool slot gets its own independent, isolated SQLite database.
    /// The database is discarded when the slot is recycled.
    pub fn new() -> Self {
        Self {
            storage: SqliteStorage::Memory,
        }
    }

    /// Create a `SqlitePack` backed by a file at `path`.
    ///
    /// All pool slots open the same file, providing shared persistent storage.
    ///
    /// **Warning:** this mode fails silently on `IsolationLevel::Untrusted`
    /// because the seccomp filter blocks the `open()` syscall required to
    /// access the file.  Use in-memory mode for untrusted scripts.
    pub fn new_file(path: impl Into<PathBuf>) -> Self {
        Self {
            storage: SqliteStorage::File(path.into()),
        }
    }
}

impl Default for SqlitePack {
    fn default() -> Self {
        Self::new()
    }
}

impl SdkExtension for SqlitePack {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    fn ops(&self) -> Vec<OpDecl> {
        vec![op_db_query(), op_db_execute()]
    }

    fn esm_files(&self) -> Vec<(&'static str, &'static str)> {
        vec![("sandbox:sqlite", include_str!("../../sdk-ts/src/sqlite.js"))]
    }

    fn ts_declarations(&self) -> &'static str {
        include_str!("../../sdk-ts/types/sqlite.d.ts")
    }

    fn inject_op_state(&self, op_state: &mut deno_core::OpState) {
        let conn = match &self.storage {
            SqliteStorage::Memory => {
                rusqlite::Connection::open_in_memory().expect("SQLite in-memory connection failed")
            },
            SqliteStorage::File(path) => {
                rusqlite::Connection::open(path).expect("SQLite file connection failed")
            },
        };
        op_state.put(SqliteStore { conn });
    }
}
