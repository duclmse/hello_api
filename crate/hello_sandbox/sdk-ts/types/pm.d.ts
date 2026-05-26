// sdk-ts/types/pm.d.ts -- TypeScript declarations for sandbox:pm

/** Chai-like assertion chain returned by pm.expect() and expect(). */
export interface Assertion {
  // Negation
  not: Assertion;
  // Fluent chain words (no-op passthrough)
  to: Assertion;
  be: Assertion;
  been: Assertion;
  is: Assertion;
  have: Assertion;
  that: Assertion;
  which: Assertion;
  and: Assertion;
  has: Assertion;
  with: Assertion;
  at: Assertion;
  of: Assertion;
  same: Assertion;
  but: Assertion;
  does: Assertion;
  // Property assertions
  readonly ok: Assertion;
  readonly true: Assertion;
  readonly false: Assertion;
  readonly null: Assertion;
  readonly undefined: Assertion;
  readonly empty: Assertion;
  // Method assertions
  equal(expected: unknown): Assertion;
  eql(expected: unknown): Assertion;
  include(item: unknown): Assertion;
  contain(item: unknown): Assertion;
  a(type: string): Assertion;
  an(type: string): Assertion;
  above(n: number): Assertion;
  greaterThan(n: number): Assertion;
  below(n: number): Assertion;
  lessThan(n: number): Assertion;
  least(n: number): Assertion;
  most(n: number): Assertion;
  property(key: string, value?: unknown): Assertion;
  lengthOf(n: number): Assertion;
  match(re: RegExp): Assertion;
  startsWith(s: string): Assertion;
  endsWith(s: string): Assertion;
  status(code: number): Assertion;
}

/** Header accessor interface. */
export interface HeaderAccessor {
  get(name: string): string | null;
  has(name: string): boolean;
}

/** pm.response — response object available in post-request scripts. */
export interface PmResponse {
  readonly code: number;
  readonly status: number;
  readonly responseTime: number;
  readonly responseSize: number;
  readonly redirected: boolean;
  readonly headers: HeaderAccessor;
  text(): string;
  json(): unknown;
  to: { have: { status(code: number): void }; be: { ok: void } };
}

/** pm.request — request object available in scripts. */
export interface PmRequest {
  readonly url: string;
  readonly method: string;
  readonly headers: HeaderAccessor;
  readonly body: string | null;
}

/** In-memory key-value store (pm.environment, pm.variables, pm.globals). */
export interface Store {
  get(key: string): unknown;
  set(key: string, value: unknown): void;
  has(key: string): boolean;
  unset(key: string): void;
  clear(): void;
}

/** Bruno bru utility object. */
export interface BruStore {
  getEnvVar(key: string): unknown;
  setEnvVar(key: string, value: unknown): void;
  deleteEnvVar(key: string): void;
  getVar(key: string): unknown;
  setVar(key: string, value: unknown): void;
  getEnvName(): string;
}

/** Bruno res response object. */
export interface BruResponse {
  readonly status: number | null;
  readonly body: string;
  readonly headers: [string, string][];
  getBody(): string;
  getStatus(): number | null;
  getResponseTime(): number;
  getSize(): number;
  getHeader(name: string): string | null;
}

/** Bruno req request object. */
export interface BruRequest {
  readonly url: string;
  readonly method: string;
  readonly headers: [string, string][];
  readonly body: string | null;
}

/** pm info metadata. */
export interface PmInfo {
  readonly eventName: string;
  readonly iterationCount: number;
  readonly iteration: number;
  readonly requestName: string;
  readonly requestId: string;
}

/** Main Postman pm object. */
export interface Pm {
  /** Run a named test. fn() throws on failure. */
  test(name: string, fn: () => void): void;
  /** Create a Chai-like assertion chain. */
  expect(value: unknown): Assertion;
  /** Lazily-read response for this run. */
  readonly response: PmResponse | null;
  /** Lazily-read request for this run. */
  readonly request: PmRequest | null;
  /** In-memory environment variable store. */
  environment: Store;
  /** In-memory per-request variable store. */
  variables: Store;
  /** Alias for pm.variables. */
  collectionVariables: Store;
  /** In-memory global variable store. */
  globals: Store;
  /** Script metadata. */
  info: PmInfo;
}

/** Result of a results() call. */
export interface Results {
  pass: boolean;
  failures: string[];
}

/** Postman-compatible pm object. */
export declare const pm: Pm;

/** Bruno-compatible global test() — alias for pm.test(). */
export declare function test(name: string, fn: () => void): void;

/** Bruno-compatible global expect() — alias for pm.expect(). */
export declare function expect(value: unknown): Assertion;

/** Bruno-compatible res response object. */
export declare const res: BruResponse;

/** Bruno-compatible req request object. */
export declare const req: BruRequest;

/** Bruno bru utility object. */
export declare const bru: BruStore;

/**
 * Return current pm.test() results and reset all state for the next run.
 * Always call as the final `return results();` in a post-script.
 */
export declare function results(): Results;
