//! `sandbox-worker` — child-process entry point for `IsolationLevel::Untrusted`.
//!
//! This binary is spawned by the parent sandbox process when `IsolationLevel::Untrusted`
//! is requested. It reads a `WorkerRequest` from stdin, executes the script inside a
//! fresh `SharedRuntime`, optionally installs a seccomp filter (Linux only), and
//! writes a `WorkerResponse` to stdout before exiting.
//!
//! # Usage
//!
//! The binary is invoked by the library — there is no user-facing CLI.
//! The worker binary path is resolved via [`hello_sandbox::child::find_worker_binary`].

fn main() {
    hello_sandbox::child::run_worker();
}
