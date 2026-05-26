// sdk-ts/src/timer_globals.js
// Timer globals injected by TimerPack before Object.freeze(globalThis).
// Provides setTimeout, clearTimeout, setInterval, clearInterval.
//
// Timers are backed by op_timer_set (Rust/tokio::time::sleep).
// Cancellation is via op_timer_clear which drops the oneshot::Sender,
// causing the pending op future to return false immediately.
//
// setInterval is bounded by max_interval_calls (from op_timer_max_interval_calls)
// to prevent infinite event-loop spinning.
//
// NOTE: extra callback arguments (setTimeout(fn, delay, arg1, ...)) are NOT
// forwarded. Use closures: setTimeout(() => fn(arg), delay).
(function() {
  let ops = globalThis.__sandbox_ops;
  let _nextId = 1;
  // Maps timer_id -> callback function (cleared on cancel or after last fire).
  let _cbs = new Map();

  // Arm one tick of a timer.
  // fired=true  -> timer fired naturally; run callback + re-arm if repeat
  // fired=false -> timer was cancelled; clean up only
  function _arm(id, delayMs, repeat, maxCalls, callCount) {
    ops.op_timer_set(id, delayMs).then(function(fired) {
      if (!fired) {
        _cbs.delete(id);
        return;
      }
      let cb = _cbs.get(id);
      if (!cb) return;
      try { cb(); } catch (_e) {}
      let next = callCount + 1;
      if (repeat && next < maxCalls && _cbs.has(id)) {
        _arm(id, delayMs, true, maxCalls, next);
      } else {
        _cbs.delete(id);
      }
    });
  }

  function _clampDelay(delay) {
    let d = +delay;
    if (!(d >= 1)) return 0; // NaN, negative, zero -> 0
    if (d > 0x7FFFFFFF) return 0x7FFFFFFF; // cap at ~24.8 days
    return d | 0; // truncate to integer
  }

  globalThis.setTimeout = function setTimeout(cb, delay) {
    let id = _nextId++;
    _cbs.set(id, cb);
    _arm(id, _clampDelay(delay), false, 1, 0);
    return id;
  };

  globalThis.clearTimeout = function clearTimeout(id) {
    id = id | 0;
    _cbs.delete(id);
    if (id > 0) ops.op_timer_clear(id);
  };

  globalThis.setInterval = function setInterval(cb, delay) {
    let id = _nextId++;
    let max = ops.op_timer_max_interval_calls();
    _cbs.set(id, cb);
    _arm(id, _clampDelay(delay), true, max, 0);
    return id;
  };

  globalThis.clearInterval = function clearInterval(id) {
    id = id | 0;
    _cbs.delete(id);
    if (id > 0) ops.op_timer_clear(id);
  };
})();
