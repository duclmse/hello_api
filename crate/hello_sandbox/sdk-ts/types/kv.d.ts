// sandbox:kv

/**
 * Per-slot key-value store.
 * State persists across runs within the same warm slot
 * and is cleared when the slot is recycled.
 *
 * @example
 * ```ts
 * import { kv } from "sandbox:kv";
 *
 * await kv.set("visits", (await kv.get<number>("visits") ?? 0) + 1);
 * const count = await kv.get<number>("visits");
 * ```
 */
export declare const kv: {
  /** Get a value by key. Returns `null` if not found. */
  get<T = unknown>(key: string): Promise<T | null>;

  /** Set a key to any JSON-serialisable value. */
  set(key: string, value: unknown): Promise<void>;

  /** Delete a key. No-op if absent. */
  delete(key: string): Promise<void>;

  /**
   * List all keys that start with `prefix`.
   * @param prefix  Default: `""` (all keys).
   */
  list(prefix?: string): Promise<string[]>;
};
