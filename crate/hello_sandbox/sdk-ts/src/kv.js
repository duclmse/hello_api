// sdk-ts/src/kv.js
// Thin Promise-based wrapper over the kv ops.
// Uses globalThis.__sandbox_ops set by core.js (Deno.core is deleted by then).
const ops = globalThis.__sandbox_ops;

const kv = Object.freeze({
  /** Get a value by key. Returns null if not found. */
  get: (key) => Promise.resolve(ops.op_kv_get(key)),

  /** Set a value. Overwrites any existing value. */
  set: (key, value) => {
    ops.op_kv_set(key, JSON.stringify(value ?? null));
    return Promise.resolve();
  },

  /** Delete a key. No-op if not found. */
  delete: (key) => {
    ops.op_kv_delete(key);
    return Promise.resolve();
  },

  /** List all keys with the given prefix. */
  list: (prefix = "") => Promise.resolve(ops.op_kv_list(prefix)),
});

export { kv };
