// sdk-ts/types/assert.d.ts -- TypeScript declarations for sandbox:assert

export interface Assert {
  /** Assert that `value` is truthy. */
  ok(value: unknown, msg?: string): void;
  /** Assert strict equality (`===`). */
  equal(actual: unknown, expected: unknown, msg?: string): void;
  /** Assert strict inequality (`!==`). */
  notEqual(actual: unknown, expected: unknown, msg?: string): void;
  /** Assert string or array contains `needle`. */
  contains(haystack: string | unknown[], needle: unknown, msg?: string): void;
  /** Assert `a > b`. */
  greaterThan(a: number, b: number, msg?: string): void;
  /** Assert `a < b`. */
  lessThan(a: number, b: number, msg?: string): void;
  /** Assert strictly `null`. */
  isNull(value: unknown, msg?: string): void;
  /** Assert strictly `undefined`. */
  isUndefined(value: unknown, msg?: string): void;
}

export declare const assert: Assert;
