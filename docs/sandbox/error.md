# SandboxError

`src/error.rs` defines all error variants returned by the sandbox.

```rust
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("script timed out after {0:?}")]
    Timeout(Duration),

    #[error("script exceeded memory limit")]
    OutOfMemory,

    #[error("quota exceeded: {0} calls")]
    QuotaExceeded(usize),

    #[error("module not found: {0}")]
    ModuleNotFound(String),

    #[error("transpile error: {0}")]
    TranspileError(String),

    #[error("runtime error: {0}")]
    Runtime(#[from] anyhow::Error),

    #[error("rate limit exceeded: {resource} (limit {limit})")]
    RateLimitExceeded { resource: String, limit: usize },

    #[error("child process error: {0}")]
    ChildProcess(String),

    #[error("capability denied: {0}")]
    CapabilityDenied(String),
}
```

## Variants

| Variant                                 | When Raised                                                           |
| --------------------------------------- | --------------------------------------------------------------------- |
| `Timeout(Duration)`                     | Watchdog thread fires after `effective_timeout` elapses               |
| `OutOfMemory`                           | V8 heap limit callback triggers, OOM flag is set                      |
| `QuotaExceeded(n)`                      | Generic quota exceeded (not currently used for rate limits)           |
| `ModuleNotFound(specifier)`             | An `import` references a module not registered in the loader          |
| `TranspileError(msg)`                   | TypeScript source fails to transpile                                  |
| `Runtime(err)`                          | A JavaScript runtime error (syntax error, uncaught exception, etc.)   |
| `RateLimitExceeded { resource, limit }` | HTTP/KV/emit call count exceeds per-run quota                         |
| `ChildProcess(msg)`                     | Child-process worker fails to spawn, communicate, or returns an error |
| `CapabilityDenied(msg)`                 | A script operation is blocked by `RunCapabilities` restrictions       |

## Handling Errors

```rust
match sandbox.run(script).await {
    Ok(result) => { /* use result */ }
    Err(SandboxError::Timeout(d)) => eprintln!("timed out after {:?}", d),
    Err(SandboxError::OutOfMemory) => eprintln!("OOM"),
    Err(SandboxError::CapabilityDenied(msg)) => eprintln!("denied: {}", msg),
    Err(SandboxError::Runtime(e)) => eprintln!("JS error: {}", e),
    Err(e) => eprintln!("other error: {}", e),
}
```

After any `SandboxError`, the pool slot that ran the script is marked Stale and
will be recycled before being reused. This prevents error state from persisting
across runs.

## Source

`src/error.rs`
