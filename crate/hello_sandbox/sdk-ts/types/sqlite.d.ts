// sandbox:sqlite

/**
 * Per-slot SQLite database.
 *
 * In-memory by default — the database is cleared when the slot is recycled.
 * File-backed mode is available via `SqlitePack::new_file(path)`.
 *
 * Both `query` and `execute` accept positional `?` bind parameters.
 *
 * @example
 * ```ts
 * import { db } from "sandbox:sqlite";
 *
 * db.execute("CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY, body TEXT)");
 * db.execute("INSERT INTO notes (body) VALUES (?)", "hello");
 * const rows = db.query("SELECT id, body FROM notes");
 * // rows: [[1, "hello"]]
 * ```
 */
export declare const db: {
  /**
   * Execute a SELECT (or any row-returning statement) and return all results.
   *
   * Each element of the returned array is one row; each element of a row is a
   * column value (number, string, null, or base64 string for BLOBs).
   *
   * @param sql    SQL statement with optional positional `?` placeholders.
   * @param params Positional bind parameters.
   * @returns      Array of rows (each row is an array of column values).
   */
  query(sql: string, ...params: unknown[]): unknown[][];

  /**
   * Execute an INSERT, UPDATE, DELETE, CREATE, DROP, or other DML/DDL
   * statement.
   *
   * @param sql    SQL statement with optional positional `?` placeholders.
   * @param params Positional bind parameters.
   * @returns      Number of rows changed by the statement.
   */
  execute(sql: string, ...params: unknown[]): number;
};
