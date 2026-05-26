use hello_client::TestResult;

/// Events sent from the background runner thread to the TUI event loop.
pub enum RunnerEvent {
    /// Test at `index` has started executing.
    TestStarted(usize),
    /// Test at `index` finished; result is attached.
    TestFinished(usize, Box<TestResult>),
    /// All tests finished; total wall-clock time in milliseconds.
    Done { elapsed_ms: u128 },
    /// A fatal runner error occurred; no further events will be sent.
    Error(String),
}
