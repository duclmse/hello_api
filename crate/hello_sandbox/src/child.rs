//! Child-process worker for `IsolationLevel::Untrusted`.
//!
//! # Architecture
//!
//! When `IsolationLevel::Untrusted` is requested on Linux, the parent process
//! spawns a fresh copy of the `sandbox-worker` binary (child), communicates
//! via stdin/stdout using a simple newline-delimited JSON protocol, and
//! enforces a hard wall-clock timeout on the entire child lifecycle.
//!
//! ```text
//! Parent                              Child (sandbox-worker)
//! ──────                              ──────────────────────
//! serialize WorkerRequest → stdin ──► read stdin
//!                                     create SharedRuntime (V8 init)
//!                                     install seccomp filter  (Linux)
//!                                     run script
//!                                     serialize WorkerResponse → stdout
//! read stdout ◄─────────────────────── write stdout
//! parse WorkerResponse                exit 0
//! ```
//!
//! On non-Linux platforms, the child process path is unavailable. The caller
//! (`Sandbox::run`) logs a warning and falls back to `PowerUser` isolation.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{IsolationLevel, SandboxConfig};
use crate::event::SandboxEvent;
use crate::SandboxError;

#[cfg(target_os = "linux")]
use crate::sandbox::SandboxResult;
#[cfg(target_os = "linux")]
use std::time::Instant;

// ─── Wire protocol ────────────────────────────────────────────────────────────

/// JSON payload written to the worker's stdin.
#[derive(Serialize, Deserialize)]
pub(crate) struct WorkerRequest {
    pub script: String,
    pub inputs: HashMap<String, Value>,
    /// `config.timeout` in milliseconds.
    pub timeout_ms: u64,
    pub heap_initial_bytes: usize,
    pub heap_max_bytes: usize,
    pub max_log_lines: usize,
    pub allow_typescript: bool,
    pub allow_modules: bool,
    pub allow_events: bool,
}

impl WorkerRequest {
    #[cfg(target_os = "linux")]
    pub(crate) fn from_parts(
        script: &str,
        inputs: &HashMap<String, Value>,
        config: &SandboxConfig,
    ) -> Self {
        Self {
            script: script.to_owned(),
            inputs: inputs.clone(),
            timeout_ms: config.timeout.as_millis() as u64,
            heap_initial_bytes: config.heap_initial_bytes,
            heap_max_bytes: config.heap_max_bytes,
            max_log_lines: config.max_log_lines,
            allow_typescript: config.allow_typescript,
            allow_modules: config.allow_modules,
            allow_events: config.allow_events,
        }
    }

    pub(crate) fn to_sandbox_config(&self) -> SandboxConfig {
        use crate::config::NoopMetricsSink;
        use std::sync::Arc;
        SandboxConfig {
            // Worker always uses PowerUser isolation inside the sandbox:
            // the OS-level isolation (seccomp) is the child-process boundary.
            isolation: IsolationLevel::PowerUser,
            timeout: Duration::from_millis(self.timeout_ms),
            heap_initial_bytes: self.heap_initial_bytes,
            heap_max_bytes: self.heap_max_bytes,
            max_log_lines: self.max_log_lines,
            allow_typescript: self.allow_typescript,
            allow_modules: self.allow_modules,
            allow_events: self.allow_events,
            metrics_sink: Arc::new(NoopMetricsSink),
            rate_limits: crate::config::RateLimitConfig::default(),
            max_interval_calls: 1_000,
        }
    }
}

/// JSON payload written to the worker's stdout.
#[derive(Serialize, Deserialize, Default)]
pub(crate) struct WorkerResponse {
    #[serde(default)]
    pub value: Value,
    #[serde(default)]
    pub logs: Vec<String>,
    #[serde(default)]
    pub events: Vec<SandboxEvent>,
    /// Set when the script raised a runtime error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Set when V8's near-heap-limit callback fired.
    #[serde(default)]
    pub oom: bool,
    /// Set when the watchdog timed out the script.
    #[serde(default)]
    pub timed_out: bool,
}

// ─── seccomp (Linux only) ─────────────────────────────────────────────────────

/// Install a seccomp-BPF allowlist so that the worker process cannot perform
/// dangerous OS operations (file I/O, network, process creation).
///
/// Called in the worker **after** V8 / tokio / deno_core have been initialised
/// but **before** user script execution begins.  This means V8's startup
/// (JIT compilation, shared-library init) is unrestricted; only the user
/// script's execution environment is locked down.
///
/// The allowlist covers the syscalls required by the V8 JIT, tokio's async
/// I/O, and the sandbox's op channel (pipes), while blocking the families
/// that pose the most risk from adversarial scripts:
///
/// - File access: `open`, `openat`, `creat`, `unlink`, `rename`, …
/// - Network: `socket`, `connect`, `bind`, `sendto`, `recvfrom`, …
/// - Process creation: `fork`, `execve`, `execveat`
/// - Debugging: `ptrace`, `process_vm_readv`, …
///
/// **Linux x86-64 / aarch64 only.**
#[cfg(target_os = "linux")]
fn install_seccomp_filter() -> Result<(), Box<dyn std::error::Error>> {
    use seccompiler::{apply_filter, BpfProgram, SeccompAction, SeccompFilter, SeccompRule};
    use std::collections::HashMap;

    // Syscalls required by V8 JIT + tokio + deno_core op channels.
    // After V8 initialises (including loading its snapshot), the script
    // execution phase only needs memory management, signalling, scheduling,
    // and I/O on already-open file descriptors.
    let allowed: &[i64] = &[
        // ── Memory ──────────────────────────────────────────────────────────
        libc::SYS_brk,
        libc::SYS_mmap,
        libc::SYS_mprotect,
        libc::SYS_munmap,
        libc::SYS_madvise,
        libc::SYS_memfd_create,
        libc::SYS_mremap,
        // ── File descriptors (existing FDs only — no open/socket) ───────────
        libc::SYS_read,
        libc::SYS_write,
        libc::SYS_readv,
        libc::SYS_writev,
        libc::SYS_pread64,
        libc::SYS_pwrite64,
        libc::SYS_lseek,
        libc::SYS_fstat,
        libc::SYS_fcntl,
        libc::SYS_ioctl,
        libc::SYS_close,
        libc::SYS_pipe2,
        // ── Async I/O (tokio epoll reactor) ─────────────────────────────────
        libc::SYS_epoll_create1,
        libc::SYS_epoll_ctl,
        libc::SYS_epoll_wait,
        libc::SYS_epoll_pwait,
        libc::SYS_eventfd2,
        libc::SYS_timerfd_create,
        libc::SYS_timerfd_settime,
        libc::SYS_timerfd_gettime,
        // ── Threading / scheduling ───────────────────────────────────────────
        libc::SYS_clone,
        libc::SYS_clone3,
        libc::SYS_futex,
        libc::SYS_sched_yield,
        libc::SYS_sched_getaffinity,
        libc::SYS_set_robust_list,
        libc::SYS_get_robust_list,
        // ── Signals ──────────────────────────────────────────────────────────
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigreturn,
        libc::SYS_tgkill,
        // ── Clocks / timers ──────────────────────────────────────────────────
        libc::SYS_clock_gettime,
        libc::SYS_clock_nanosleep,
        libc::SYS_nanosleep,
        // ── Process identity (read-only, safe) ───────────────────────────────
        libc::SYS_getpid,
        libc::SYS_gettid,
        libc::SYS_getpgrp,
        libc::SYS_getuid,
        libc::SYS_getgid,
        // ── Randomness ───────────────────────────────────────────────────────
        libc::SYS_getrandom,
        // ── Process exit ─────────────────────────────────────────────────────
        libc::SYS_exit,
        libc::SYS_exit_group,
        // ── Misc required by V8 / glibc internals ───────────────────────────
        libc::SYS_prctl,
        libc::SYS_rseq,
        libc::SYS_statx,
    ];

    let mut rules: HashMap<i64, Vec<SeccompRule>> = HashMap::new();
    for &nr in allowed {
        // SeccompRule with empty conditions = unconditionally match (always allow).
        rules.insert(nr, vec![SeccompRule::new(vec![])?]);
    }

    let filter = SeccompFilter::new(
        rules,
        // Default action: return EPERM for any syscall not in the allowlist.
        // Using Errno rather than KillProcess so V8 can propagate errors
        // gracefully through the op channel instead of crashing silently.
        SeccompAction::Errno(libc::EPERM as u32),
        SeccompAction::Allow,
        std::env::consts::ARCH.try_into()?,
    )?;

    let bpf: BpfProgram = filter.try_into()?;
    apply_filter(&bpf)?;
    Ok(())
}

// ─── Worker binary entry point ────────────────────────────────────────────────

/// Run as the sandbox worker process.
///
/// Reads a [`WorkerRequest`] as JSON from stdin, executes the script in a
/// fresh `SharedRuntime`, and writes a [`WorkerResponse`] as JSON to stdout.
///
/// Exits with code 0 on success (including script errors, which are reported
/// inside the response) or code 1 if the request is malformed.
///
/// This function is called from `src/bin/worker.rs`. It is also the hook for
/// the re-invoke pattern: any binary that links `hello_sandbox` can call this
/// at startup when `SANDBOX_WORKER=1` is set.
pub fn run_worker() {
    use crate::loader::AllowlistModuleLoaderBuilder;
    use crate::runtime::SharedRuntime;
    use crate::sdk::{core_sdk::CorePack, SdkRegistry};
    use tokio::task::LocalSet;

    // ── Read request from stdin ───────────────────────────────────────────────
    let mut raw = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut raw) {
        eprintln!("worker: failed to read stdin: {e}");
        std::process::exit(1);
    }

    let request: WorkerRequest = match serde_json::from_str(&raw) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("worker: failed to parse request: {e}");
            std::process::exit(1);
        },
    };

    let config = request.to_sandbox_config();

    // ── Build SDK registry (CorePack only in Untrusted tier) ─────────────────
    let sdk = SdkRegistry {
        packs: vec![Box::new(CorePack)],
    };

    // ── Create the tokio + LocalSet runtime ──────────────────────────────────
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("worker: tokio runtime");

    rt.block_on(async move {
        let local = LocalSet::new();
        local
            .run_until(async move {
                // ── Create SharedRuntime (V8 init, unrestricted) ─────────────────
                let loader = AllowlistModuleLoaderBuilder::default();
                let mut runtime = SharedRuntime::new(config.clone(), loader, &sdk);

                // ── Install seccomp filter (Linux only, AFTER V8 init) ────────────
                #[cfg(target_os = "linux")]
                {
                    if let Err(e) = install_seccomp_filter() {
                        let response = WorkerResponse {
                            error: Some(format!("seccomp install failed: {e}")),
                            ..Default::default()
                        };
                        write_response(&response);
                        std::process::exit(1);
                    }
                }

                // ── Execute user script ───────────────────────────────────────────
                let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
                let result = runtime
                    .run(
                        &request.script,
                        request.inputs,
                        event_tx,
                        crate::config::RunCapabilities::default(),
                    )
                    .await;
                let events: Vec<SandboxEvent> =
                    std::iter::from_fn(|| event_rx.try_recv().ok()).collect();

                let response = match result {
                    Ok((value, logs, _metrics)) => WorkerResponse {
                        value,
                        logs,
                        events,
                        ..Default::default()
                    },
                    Err(SandboxError::OutOfMemory) => WorkerResponse {
                        oom: true,
                        ..Default::default()
                    },
                    Err(SandboxError::Timeout(_)) => WorkerResponse {
                        timed_out: true,
                        ..Default::default()
                    },
                    Err(e) => WorkerResponse {
                        error: Some(e.to_string()),
                        ..Default::default()
                    },
                };

                write_response(&response);
            })
            .await;
    });
}

/// Serialize `response` to stdout and flush. Exits with code 1 on failure.
fn write_response(response: &WorkerResponse) {
    let bytes = match serde_json::to_vec(response) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("worker: failed to serialize response: {e}");
            std::process::exit(1);
        },
    };
    if let Err(e) = std::io::stdout().write_all(&bytes) {
        eprintln!("worker: failed to write response: {e}");
        std::process::exit(1);
    }
}

// ─── Worker binary resolution ─────────────────────────────────────────────────

/// Find the `sandbox-worker` binary path.
///
/// Resolution order:
/// 1. `SANDBOX_WORKER_BIN` environment variable (useful in tests).
/// 2. `sandbox-worker` adjacent to the current executable.
/// 3. `sandbox-worker` (rely on `PATH`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn find_worker_binary() -> PathBuf {
    if let Ok(p) = std::env::var("SANDBOX_WORKER_BIN") {
        return PathBuf::from(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        let adjacent = exe.parent().unwrap_or(Path::new(".")).join("sandbox-worker");
        if adjacent.exists() {
            return adjacent;
        }
    }
    PathBuf::from("sandbox-worker")
}

// ─── Parent-side: spawn worker ────────────────────────────────────────────────

/// Execute `script` in a fresh child process running the `sandbox-worker` binary.
///
/// The child receives a [`WorkerRequest`] on stdin and is expected to write a
/// [`WorkerResponse`] on stdout before exiting with code 0.
///
/// `config.timeout` governs the wall-clock deadline for the entire child
/// lifecycle (spawn → exit).  A 500 ms buffer is added to account for
/// startup overhead; the watchdog inside the worker handles the in-script
/// timeout.
///
/// **Linux only.**  The non-Linux fallback lives in `sandbox.rs`.
#[cfg(target_os = "linux")]
pub(crate) async fn run_in_child_process(
    script: &str,
    inputs: &HashMap<String, Value>,
    config: &SandboxConfig,
    worker_binary: &Path,
) -> Result<SandboxResult, SandboxError> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;

    let start = Instant::now();
    let request = WorkerRequest::from_parts(script, inputs, config);
    let request_json = serde_json::to_vec(&request)
        .map_err(|e| SandboxError::ChildProcess(format!("serialize request: {e}")))?;

    // Add 500 ms overhead for process startup on top of the script timeout.
    let wall_timeout = config.timeout + Duration::from_millis(500);

    // Spawn the worker.
    let mut child = Command::new(worker_binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            SandboxError::ChildProcess(format!(
                "failed to spawn '{}': {e}",
                worker_binary.display()
            ))
        })?;

    // Write request to worker stdin; drop the handle to signal EOF.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&request_json)
            .await
            .map_err(|e| SandboxError::ChildProcess(format!("stdin write: {e}")))?;
        // `stdin` is dropped here → child sees EOF
    }

    // Wait for the child to finish, bounded by `wall_timeout`.
    let output = tokio::time::timeout(wall_timeout, child.wait_with_output())
        .await
        .map_err(|_| {
            SandboxError::ChildProcess(format!("worker timed out after {:?}", wall_timeout))
        })?
        .map_err(|e| SandboxError::ChildProcess(format!("wait_with_output: {e}")))?;

    let elapsed = start.elapsed();

    if !output.status.success() {
        return Err(SandboxError::ChildProcess(format!("worker exited with {}", output.status)));
    }

    // Parse the response.
    let resp: WorkerResponse = serde_json::from_slice(&output.stdout)
        .map_err(|e| SandboxError::ChildProcess(format!("parse response: {e}")))?;

    // Map worker-side error signals back to `SandboxError` variants.
    if resp.oom {
        return Err(SandboxError::OutOfMemory);
    }
    if resp.timed_out {
        return Err(SandboxError::Timeout(config.timeout));
    }
    if let Some(err_msg) = resp.error {
        return Err(SandboxError::Runtime(anyhow::anyhow!("{}", err_msg)));
    }

    Ok(SandboxResult {
        value: resp.value,
        logs: resp.logs,
        events: resp.events,
        elapsed,
        runtime_kind: crate::pool::RuntimeKind::Isolated,
        metrics: crate::config::RunMetrics::default(),
    })
}
