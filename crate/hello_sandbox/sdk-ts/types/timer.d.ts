// sdk-ts/types/timer.d.ts
// Ambient TypeScript declarations for timer globals installed by TimerPack.
//
// setTimeout and setInterval behave like browser APIs, with sandbox-specific
// constraints:
//   - setInterval callbacks are limited to max_interval_calls per timer per run
//     (default: 1000). Further firings are silently stopped.
//   - All pending timers are automatically cancelled when the run completes.
//   - Extra callback arguments (arg1, arg2, ...) are accepted in the type
//     signature for compatibility but are NOT passed to the callback.
//     Use closures instead: setTimeout(() => fn(arg), delay).

declare function setTimeout(
  callback: (...args: unknown[]) => void,
  delay?: number,
  ...args: unknown[]
): number;

declare function clearTimeout(id?: number): void;

declare function setInterval(
  callback: (...args: unknown[]) => void,
  delay?: number,
  ...args: unknown[]
): number;

declare function clearInterval(id?: number): void;
