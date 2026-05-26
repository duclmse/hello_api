use thiserror::Error;

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("script timed out after {0:?}")]
    Timeout(std::time::Duration),

    #[error("script exceeded memory limit")]
    OutOfMemory,

    #[error("log/event quota exceeded ({0} lines)")]
    QuotaExceeded(usize),

    #[error("module not found or not allowed: {0}")]
    ModuleNotFound(String),

    #[error("TypeScript transpile error: {0}")]
    TranspileError(String),

    #[error("runtime error: {0}")]
    Runtime(#[from] anyhow::Error),

    #[error("rate limit exceeded: {resource} (limit: {limit})")]
    RateLimitExceeded { resource: String, limit: usize },

    #[error("child process error: {0}")]
    ChildProcess(String),

    #[error("capability denied: {0}")]
    CapabilityDenied(String),
}
