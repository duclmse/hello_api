// sandbox:core — always available as globals, no import needed.

/** Injected by the sandbox runtime. Available without any import. */
declare const sandbox: {
  /**
   * Read a named value provided by the host before this run.
   * Returns `null` if the key was not set.
   */
  readInput<T = unknown>(key: string): T;

  /**
   * Push a named event with an arbitrary JSON payload to the host.
   * The host receives events in real time via an async channel.
   */
  emit(name: string, payload?: unknown): void;

  /**
   * Read the per-run tags attached by the host via `RunCapabilities.tags`.
   * Returns a frozen `Record<string, string>`. Empty object if no tags were set.
   */
  tags(): Readonly<Record<string, string>>;
};

/** Standard console — output is captured and returned in `SandboxResult.logs`. */
//@ts-ignore
declare const console: {
  log(...args: unknown[]): void;
  info(...args: unknown[]): void;
  warn(...args: unknown[]): void;
  error(...args: unknown[]): void;
  debug(...args: unknown[]): void;
};
