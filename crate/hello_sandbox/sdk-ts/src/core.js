// sdk-ts/src/core.js
// Bootstrap shim -- installs console + sandbox globals, freezes prototypes.
//
// This file is the esm_entry_point of the sandbox extension.
// It exports NOTHING. Importing it always yields an empty namespace.
//
// SECURITY CONTRACT:
//   1. ops captured in block scope BEFORE any freeze or delete.
//   2. console and sandbox installed on globalThis.
//   3. All built-in prototypes frozen to block prototype pollution.
//   4. globalThis.Deno deleted so scripts cannot reach Deno internals.
//   5. globalThis frozen last so scripts cannot add new globals.

const { ops } = Deno.core;

// -- console ------------------------------------------------------------------

const _fmt = (...args) => {
  return args //
    .map(a => (typeof a === "object" ? JSON.stringify(a) : String(a)))
    .join(" ");
};

globalThis.console = Object.freeze({
  log: (...a) => ops.op_sandbox_print(_fmt(...a), false),
  info: (...a) => ops.op_sandbox_print("[INFO] " + _fmt(...a), false),
  warn: (...a) => ops.op_sandbox_print("[WARN] " + _fmt(...a), false),
  error: (...a) => ops.op_sandbox_print("[ERROR] " + _fmt(...a), false),
  debug: (...a) => ops.op_sandbox_print("[DEBUG] " + _fmt(...a), false),
});

// -- sandbox ------------------------------------------------------------------

globalThis.sandbox = Object.freeze({
  /** Read a named value provided by the host. Returns null if not set. */
  readInput: key => ops.op_read_input(key),

  /**
   * Push a named event with an arbitrary JSON payload to the host channel.
   * @param {string} name
   * @param {unknown} [payload]
   */
  emit: (name, payload) => ops.op_emit(name, JSON.stringify(payload ?? null)),

  /**
   * Read the per-run tags attached by the host via RunCapabilities.tags.
   * Returns a frozen Record<string, string>. Empty object if no tags were set.
   */
  tags: () => Object.freeze(ops.op_read_tags()),
});

// -- Freeze built-in prototypes (prototype pollution guard) -------------------

[
  Object.prototype,
  Function.prototype,
  Array.prototype,
  String.prototype,
  Number.prototype,
  Boolean.prototype,
  RegExp.prototype,
  Error.prototype,
  TypeError.prototype,
  RangeError.prototype,
  ReferenceError.prototype,
  SyntaxError.prototype,
  URIError.prototype,
  EvalError.prototype,
  Promise.prototype,
  Map.prototype,
  Set.prototype,
  WeakMap.prototype,
  WeakSet.prototype,
  Date.prototype,
  Symbol.prototype,
  ArrayBuffer.prototype,
  DataView.prototype,
  Uint8Array.prototype,
  Int8Array.prototype,
  Uint16Array.prototype,
  Int16Array.prototype,
  Uint32Array.prototype,
  Int32Array.prototype,
  Float32Array.prototype,
  Float64Array.prototype,
  BigInt64Array.prototype,
  BigUint64Array.prototype,
].forEach(Object.freeze);

// -- Expose ops for SDK shims loaded lazily after Deno is deleted -------------
// SDK shims (kv.js, crypto.js, http.js) are loaded via AllowlistModuleLoader
// after core.js has already run and deleted Deno. They capture ops via this
// reference rather than via Deno.core (which is gone by that point).

globalThis.__sandbox_ops = ops;

// PRE_FREEZE_INJECTION
// (SDK packs may inject globals here before globalThis is frozen.
//  SharedRuntime::new() replaces this marker with pack-provided JS snippets.)

// -- Delete Deno (prevent access to Deno internals from scripts) --------------

delete globalThis.Deno;

// -- Freeze globalThis (block new globals) ------------------------------------

Object.freeze(globalThis);
