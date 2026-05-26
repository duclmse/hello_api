# TimerPack — setTimeout / setInterval

`src/sdk/timer_sdk.rs` + `sdk-ts/src/timer_globals.js`

TimerPack installs `setTimeout`, `clearTimeout`, `setInterval`, and `clearInterval` as sandboxed globals (not a module import — they are available directly).

---

## Registration

```rust
use hello_sandbox::sdk::timer_sdk::TimerPack;

let sandbox = SandboxBuilder::new()
    .sdk(TimerPack)
    .build()?;
```

---

## JavaScript API

These are available as globals (no import needed):

```js
// setTimeout — fires once after delay_ms
const timerId = setTimeout(() => {
    console.log("fired after 100ms");
}, 100);

// clearTimeout — cancel before it fires
clearTimeout(timerId);

// setInterval — fires repeatedly, bounded by max_interval_calls
const intervalId = setInterval(() => {
    console.log("tick");
}, 50);

// clearInterval — stop the interval
clearInterval(intervalId);

// Awaiting a timer using a Promise
await new Promise(resolve => setTimeout(resolve, 200));
```

---

## Architecture

Timers are backed by `tokio::time::sleep`. Cancellation uses a per-timer `oneshot` channel:

```
JS: setTimeout(cb, 100)
  → op_timer_set(id=1, delay_ms=100)
      stores cancel_tx in RunState.timers[1]
      tokio::select! {
          _ = sleep(100ms) → return true  (fired)
          _ = cancel_rx    → return false (cancelled)
      }

JS: clearTimeout(1)
  → op_timer_clear(id=1)
      removes + drops cancel_tx from RunState.timers[1]
      cancel_rx resolves → op_timer_set returns false
```

When a run completes, `RunState` is dropped via `try_take::<RunState>()`, which drops all timers in `RunState.timers`. Every pending `op_timer_set` future immediately returns `false`. This prevents timer ops from outliving the run.

`setInterval` is implemented entirely in JS (`timer_globals.js`) by re-arming `op_timer_set` after each successful tick.

---

## Limits

`setInterval` is bounded by `SandboxConfig::max_interval_calls` (default: 1000). After this many re-arms, the interval silently stops. This prevents infinite event-loop spinning.

The limit is read from `RunState` via `op_timer_max_interval_calls` so it can be configured per-`SandboxConfig`.

---

## Notes

- Extra callback arguments (`setTimeout(fn, delay, arg1, ...)`) are **not** forwarded. Use a closure: `setTimeout(() => fn(arg), delay)`.
- Timer globals are installed via the `pre_freeze_globals()` mechanism in `core.js`, so they are available before `globalThis` is frozen.
- `TimerPack` has no `sandbox:timer` module specifier — timers are globals.

---

## Ops

| Op | Description |
|----|-------------|
| `op_timer_set(timer_id, delay_ms)` | Async: arm timer, returns `true` (fired) or `false` (cancelled) |
| `op_timer_clear(timer_id)` | Sync: cancel timer by dropping its sender |
| `op_timer_max_interval_calls()` | Sync: return max interval calls from `RunState` |

---

## TypeScript Declarations

```typescript
// sdk-ts/types/timer.d.ts
declare function setTimeout(callback: () => void, delay?: number): number;
declare function clearTimeout(id?: number): void;
declare function setInterval(callback: () => void, delay?: number): number;
declare function clearInterval(id?: number): void;
```

---

## Source

`src/sdk/timer_sdk.rs`
`sdk-ts/src/timer_globals.js`
`sdk-ts/types/timer.d.ts`
