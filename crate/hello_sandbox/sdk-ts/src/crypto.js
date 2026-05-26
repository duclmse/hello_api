// sdk-ts/src/crypto.js
const ops = globalThis.__sandbox_ops;

const crypto = Object.freeze({
  /**
   * Hash a UTF-8 string.
   * @param {"sha256" | "sha512"} algorithm
   * @param {string} data
   * @returns {Promise<string>} Lowercase hex digest.
   */
  hash: (algorithm, data) => Promise.resolve(ops.op_crypto_hash(algorithm, data)),

  /**
   * Generate n cryptographically random bytes.
   * @param {number} n
   * @returns {Uint8Array}
   */
  randomBytes: (n) => new Uint8Array(ops.op_crypto_random_bytes(n)),

  /** Generate a UUID v4 string. */
  randomUUID: () => ops.op_crypto_uuid(),
});

export { crypto };
