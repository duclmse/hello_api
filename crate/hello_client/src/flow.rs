//! Data types for the `.flow` orchestration file format.
//!
//! A `.flow` file describes how to run multiple `.http` / `.bru` requests in a
//! directed, dependency-ordered graph.  Steps that share no dependency are run
//! concurrently; the `parallel` block makes concurrent grouping explicit.
//!
//! # Example
//!
//! ```text
//! flow {
//!   name: User Onboarding
//! }
//!
//! env {
//!   base_url: https://api.example.com
//! }
//!
//! step register {
//!   file: auth/register.http
//!   capture {
//!     user_id: res.body.id
//!   }
//! }
//!
//! parallel notify {
//!   depends_on: register
//!
//!   step send_email   { file: email/welcome.http }
//!   step create_profile { file: profiles/create.http }
//! }
//!
//! step finalize {
//!   file: onboard/complete.http
//!   depends_on: notify
//! }
//! ```

use std::collections::HashMap;

// ─── Top-level definition ─────────────────────────────────────────────────────

/// Parsed representation of a `.flow` file.
#[derive(Debug, Clone)]
pub struct FlowDef {
    /// Human-readable flow name (from the `flow { name: ... }` block).
    pub name: String,
    /// Optional description string.
    pub description: Option<String>,
    /// Default environment variables merged with any caller-supplied env.
    pub env: HashMap<String, String>,
    /// Ordered list of top-level nodes (steps and parallel groups).
    pub nodes: Vec<FlowNode>,
}

// ─── Nodes ────────────────────────────────────────────────────────────────────

/// A top-level node in the flow graph — either a single step or a parallel
/// group of steps.
#[derive(Debug, Clone)]
pub enum FlowNode {
    Step(StepDef),
    Parallel(ParallelGroup),
}

impl FlowNode {
    /// The unique ID of this node within the flow.
    pub fn id(&self) -> &str {
        match self {
            Self::Step(s) => &s.id,
            Self::Parallel(p) => &p.id,
        }
    }

    /// IDs of nodes that must complete before this node runs.
    pub fn depends_on(&self) -> &[String] {
        match self {
            Self::Step(s) => &s.depends_on,
            Self::Parallel(p) => &p.depends_on,
        }
    }
}

// ─── Step ─────────────────────────────────────────────────────────────────────

/// A single request step.
///
/// `file` points to a `.http` (or `.bru`) file relative to the `.flow` file's
/// directory.  If `entry` is `Some`, the runner selects the [`TestCase`] whose
/// `name` matches exactly; otherwise the first entry is used.
///
/// [`TestCase`]: crate::TestCase
#[derive(Debug, Clone)]
pub struct StepDef {
    /// Unique identifier within the flow (used in `depends_on` references).
    pub id: String,
    /// Path to the `.http` file, relative to the `.flow` file.
    pub file: String,
    /// Optional entry name — matches against [`TestCase::name`].
    ///
    /// If `None`, the first entry in the file is used.
    ///
    /// [`TestCase::name`]: crate::TestCase::name
    pub entry: Option<String>,
    /// IDs of nodes (steps or parallel groups) that must finish first.
    pub depends_on: Vec<String>,
    /// Variable captures applied to the shared env after the step completes.
    pub capture: Vec<CaptureBinding>,
}

// ─── Parallel group ───────────────────────────────────────────────────────────

/// An explicit group of steps that all run concurrently.
///
/// The group itself acts as a single node in the dependency graph: downstream
/// steps can `depends_on` the group's `id` and will wait for **all** inner
/// steps to finish.
#[derive(Debug, Clone)]
pub struct ParallelGroup {
    /// Unique identifier for this group (used in `depends_on` references).
    pub id: String,
    /// Node IDs that all inner steps implicitly depend on.
    pub depends_on: Vec<String>,
    /// Inner steps — all run concurrently within the same layer.
    pub steps: Vec<StepDef>,
}

// ─── Capture ──────────────────────────────────────────────────────────────────

/// One variable capture: extracts a value from the response and stores it in
/// the shared environment under `var`.
#[derive(Debug, Clone)]
pub struct CaptureBinding {
    /// Destination environment variable name.
    pub var: String,
    /// Expression describing which part of the response to extract.
    pub expr: CaptureExpr,
}

/// A capture expression that addresses part of an HTTP response.
#[derive(Debug, Clone)]
pub enum CaptureExpr {
    /// HTTP status code as a decimal string, e.g. `"200"`.
    ///
    /// Syntax: `res.status`
    Status,
    /// JSON body or a nested path within it.
    ///
    /// Syntax: `res.body` (whole body) or `res.body.field.subfield`.
    /// The extracted value is stringified with `serde_json::Value::to_string`
    /// (strings are returned without surrounding quotes).
    BodyPath(Vec<String>),
    /// A response header value (case-insensitive name lookup).
    ///
    /// Syntax: `res.headers.Content-Type`
    Header(String),
}
