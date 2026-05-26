// sdk-ts/src/http.js
const ops = globalThis.__sandbox_ops;

// Minimal Response-like wrapper around the op result.
class SandboxResponse {
  #raw;
  constructor(raw) {
    this.#raw = raw;
  }

  get status() {
    return this.#raw.status;
  }
  get ok() {
    return this.#raw.ok;
  }
  get headers() {
    return new Map(this.#raw.headers);
  }

  /** Decode the body as a UTF-8 string. */
  text() {
    return Promise.resolve(
      new TextDecoder().decode(
        Uint8Array.from(
          atob(this.#raw.body)
            .split("")
            .map((c) => c.charCodeAt(0))
        )
      )
    );
  }

  /** Parse the body as JSON. */
  async json() {
    return JSON.parse(await this.text());
  }
}

/**
 * Fetch a URL. Only URLs on the host allowlist are permitted.
 * @param {string} url
 * @param {{ method?: string, headers?: [string,string][], body?: string }} [options]
 * @returns {Promise<SandboxResponse>}
 */
async function fetch(url, options = {}) {
  const raw = await ops.op_http_fetch(url, {
    method: options.method ?? "GET",
    headers: options.headers ?? [],
    body: options.body ?? null,
  });
  return new SandboxResponse(raw);
}

export { fetch, SandboxResponse };
