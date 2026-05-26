use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A structured event pushed from a user script to the host via
/// `sandbox.emit(name, payload)`.
///
/// The host receives these through an `mpsc` channel as the script runs,
/// enabling streaming / progress reporting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxEvent {
    /// Logical event name chosen by the script (e.g. `"progress"`, `"result"`).
    pub name: String,
    /// Arbitrary JSON payload.
    pub payload: Value,
    /// Monotonic timestamp (ms) since the run started.
    pub timestamp_ms: u64,
}

/// Convenience constructor used by the host-side wrapper code.
impl SandboxEvent {
    pub fn new(name: impl Into<String>, payload: Value, timestamp_ms: u64) -> Self {
        Self {
            name: name.into(),
            payload,
            timestamp_ms,
        }
    }
}
