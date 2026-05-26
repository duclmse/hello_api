// sandbox:http

/** Minimal Response-like object returned by `fetch`. */
export declare class SandboxResponse {
  readonly status: number;
  readonly ok: boolean;
  readonly headers: Map<string, string>;
  text(): Promise<string>;
  json<T = unknown>(): Promise<T>;
}

/**
 * Fetch a URL.
 *
 * Only URLs matching a prefix on the **host's allowlist** are permitted.
 * Any other URL causes an immediate error -- no network I/O occurs.
 *
 * @example
 * ```ts
 * import { fetch } from "sandbox:http";
 *
 * const res  = await fetch("https://api.example.com/items");
 * const data = await res.json<{ items: string[] }>();
 * ```
 */
export declare function fetch(
  url: string,
  options?: {
    method?:  string;
    headers?: [string, string][];
    body?:    string;
  }
): Promise<SandboxResponse>;
