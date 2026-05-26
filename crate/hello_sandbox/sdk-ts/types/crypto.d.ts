// sandbox:crypto

/**
 * Safe cryptographic primitives.
 * Excludes key generation and signing -- those belong on the host.
 *
 * @example
 * ```ts
 * import { crypto } from "sandbox:crypto";
 *
 * const id     = crypto.randomUUID();
 * const digest = await crypto.hash("sha256", payload);
 * ```
 */
export declare const crypto: {
  /**
   * Hash a UTF-8 string.
   * @returns Lowercase hex digest.
   */
  hash(algorithm: "sha256" | "sha512", data: string): Promise<string>;

  /**
   * Generate cryptographically random bytes.
   * @param n  Number of bytes.
   */
  randomBytes(n: number): Uint8Array;

  /** Generate a UUID v4 string. */
  randomUUID(): string;
};
