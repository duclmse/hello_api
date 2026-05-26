# SqlitePack — Embedded SQLite

`src/sdk/sqlite_sdk.rs` + `sdk-ts/src/sqlite.js`

SqlitePack embeds a SQLite database (via `rusqlite` with the bundled feature) into each pool slot. Scripts can execute SQL queries and mutations.

---

## Registration

```rust
use hello_sandbox::SqlitePack;

// In-memory database (default) — fresh empty DB per slot
let sandbox = SandboxBuilder::new()
    .sdk(SqlitePack::new())
    .build()?;

// File-backed database — all slots share the same on-disk file
let sandbox = SandboxBuilder::new()
    .sdk(SqlitePack::new_file("/var/db/sandbox.db"))
    .build()?;
```

---

## Storage Modes

### In-Memory (`SqlitePack::new()`)

Each pool slot gets an independent, isolated SQLite database (`:memory:`). The database is discarded when the slot is recycled. No persistence across restarts. Use for per-run scratch space.

### File-Backed (`SqlitePack::new_file(path)`)

All pool slots open the same on-disk database file. Data persists across process restarts. Suitable for shared persistent storage.

**Warning:** File-backed mode fails silently under `IsolationLevel::Untrusted` because seccomp blocks the `open()` syscall. Use in-memory mode for untrusted scripts.

---

## JavaScript API

```js
import { db } from "sandbox:sqlite";

// Execute a SELECT query — returns rows as arrays of values
const rows = await db.query(
    "SELECT id, name FROM users WHERE active = ?",
    [1]
);
// → [[1, "Alice"], [3, "Bob"]]

// Execute DML/DDL — returns number of affected rows
const affected = await db.execute(
    "INSERT INTO users (name, active) VALUES (?, ?)",
    ["Charlie", 1]
);
// → 1

await db.execute("CREATE TABLE IF NOT EXISTS logs (ts INTEGER, msg TEXT)");
await db.execute("INSERT INTO logs VALUES (?, ?)", [Date.now(), "started"]);
```

### Parameters

Positional parameters (`?`) are passed as the second argument array:

```js
await db.query("SELECT * FROM t WHERE a = ? AND b = ?", [valueA, valueB]);
```

Supported parameter types: `null`, boolean (stored as 0/1), number (integer or float), string.

Objects and arrays are serialized to JSON strings when used as parameters.

---

## Type Mapping

| JavaScript | SQLite | JavaScript |
|------------|--------|------------|
| `null` | NULL | `null` |
| `boolean` | INTEGER (0/1) | `number` |
| integer | INTEGER | `number` |
| float | REAL | `number` |
| `string` | TEXT | `string` |
| `object`/`array` | TEXT (JSON) | `string` |
| — | BLOB | `string` (base64) |

BLOB results are returned as base64-encoded strings.

---

## Per-Slot State

`SqliteStore { conn: rusqlite::Connection }` is stored in `OpState` per pool slot. The connection is opened once in `SqlitePack::inject_op_state()` and reused across all runs in that slot.

---

## Ops

| Op | Description |
|----|-------------|
| `op_db_query(sql, params_json)` | Execute SELECT, return `Vec<Vec<Value>>` |
| `op_db_execute(sql, params_json)` | Execute DML/DDL, return rows changed (`u32`) |

---

## TypeScript Declarations

```typescript
// sdk-ts/types/sqlite.d.ts
export declare const db: {
    query(sql: string, params?: unknown[]): Promise<unknown[][]>;
    execute(sql: string, params?: unknown[]): Promise<number>;
};
```

---

## Source

`src/sdk/sqlite_sdk.rs`
`sdk-ts/src/sqlite.js`
`sdk-ts/types/sqlite.d.ts`
