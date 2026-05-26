// sdk-ts/types/test.d.ts -- TypeScript declarations for sandbox:test

export interface Assertion {
  toBe(expected: unknown, msg?: string): Assertion;
  toEqual(expected: unknown, msg?: string): Assertion;
  toContain(item: unknown, msg?: string): Assertion;
  toBeTruthy(msg?: string): Assertion;
  toBeFalsy(msg?: string): Assertion;
  toBeNull(msg?: string): Assertion;
  toBeUndefined(msg?: string): Assertion;
  toBeGreaterThan(n: number, msg?: string): Assertion;
  toBeLessThan(n: number, msg?: string): Assertion;
  not: Assertion;
}

export interface ResponseHeaders {
  get(name: string): string | null;
  has(name: string): boolean;
}

export interface RawResponse {
  status: number;
  ok: boolean;
  headers: [string, string][];
  body: string;
  response_time_ms: number;
}

export interface WrappedResponse {
  status: number;
  ok: boolean;
  responseTime: number;
  size: number;
  redirected: boolean;
  headers: ResponseHeaders;
  text(): string;
  json(): unknown;
}

export declare function expect(actual: unknown): Assertion;

export declare function wrapResponse(raw: RawResponse): WrappedResponse;

export declare function results(): { pass: boolean; failures: string[] };
