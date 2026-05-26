// sdk-ts/src/sqlite.js
// Thin synchronous wrapper over the SQLite ops.
// Uses globalThis.__sandbox_ops set by core.js (Deno.core is deleted by then).
const ops = globalThis.__sandbox_ops;

const db = Object.freeze({
  /**
   * Execute a SELECT (or any row-returning statement) and return the results
   * as an array of row arrays.
   * @param {string} sql - SQL statement (positional ? placeholders).
   * @param {...unknown} params - Bind parameters in order.
   * @returns {unknown[][]} Array of rows; each row is an array of column values.
   */
  query: (sql, ...params) => ops.op_db_query(sql, JSON.stringify(params)),

  /**
   * Execute an INSERT, UPDATE, DELETE, CREATE, or other DML/DDL statement.
   * @param {string} sql - SQL statement (positional ? placeholders).
   * @param {...unknown} params - Bind parameters in order.
   * @returns {number} Number of rows affected.
   */
  execute: (sql, ...params) => ops.op_db_execute(sql, JSON.stringify(params)),
});

export { db };
