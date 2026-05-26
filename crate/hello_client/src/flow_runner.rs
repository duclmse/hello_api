//! Async execution engine for `.flow` orchestration files.
//!
//! # Execution model
//!
//! 1. `parse_flow` parses the `.flow` file into a [`FlowDef`].
//! 2. `run_flow` topologically sorts the nodes into *layers*: nodes whose
//!    `depends_on` set is satisfied by all earlier layers.
//! 3. Every layer is executed with [`futures::future::join_all`], so all nodes
//!    in a layer run concurrently on the current [`tokio::task::LocalSet`].
//! 4. Inside a `parallel` block the inner steps also run concurrently via a
//!    nested `join_all`.
//! 5. After each layer completes, captured variables are merged into the shared
//!    environment for subsequent layers.
//!
//! **Must be called from a `tokio::task::LocalSet`** — the underlying V8
//! runtime is `!Send`.

use std::collections::HashMap;
use std::path::Path;

use futures::future::join_all;
use serde_json::Value;

use crate::flow::{CaptureExpr, FlowDef, FlowNode, ParallelGroup, StepDef};
use crate::http_runner::{HttpTestRunner, TestResult};
use crate::runner::parse_collection;

pub use crate::flow_parser::parse_flow;

// ─── Public result types ──────────────────────────────────────────────────────

/// The outcome of a single executed step.
#[derive(Debug)]
pub struct StepOutcome {
    /// The step's ID as declared in the `.flow` file.
    pub id: String,
    /// The test case name resolved from the `.http` file.
    pub name: String,
    /// `true` if the post-script asserted all expectations successfully (or no
    /// post-script was present, in which case it is always `true`).
    pub passed: bool,
    /// Full test result returned by [`HttpTestRunner::run_test`].
    pub result: TestResult,
    /// Variable bindings extracted from the response and applied to the shared
    /// environment before the next layer runs.
    pub captures: Vec<(String, String)>,
}

/// Aggregate result of a complete flow execution.
pub struct FlowResult {
    /// All step outcomes in execution order (layer order, then node order
    /// within each layer, then inner-step order within parallel groups).
    pub outcomes: Vec<StepOutcome>,
    /// Number of outcomes where [`StepOutcome::passed`] is `true`.
    pub passed: usize,
    /// Number of outcomes where [`StepOutcome::passed`] is `false`.
    pub failed: usize,
}

// ─── Entry points ─────────────────────────────────────────────────────────────

/// Execute a parsed [`FlowDef`].
///
/// `base_dir` is the directory relative to which step `file` paths are
/// resolved (usually the directory containing the `.flow` file).
///
/// `extra_env` is merged over the flow's own `env` block; caller-supplied
/// values take precedence.
///
/// **Must be called from a `tokio::task::LocalSet`.**
pub async fn run_flow(
    flow: &FlowDef,
    base_dir: &Path,
    extra_env: &HashMap<String, String>,
) -> Result<FlowResult, String> {
    let layers = compute_layers(&flow.nodes)?;

    // Build the initial environment: flow env < extra_env.
    let mut env: HashMap<String, String> = flow.env.clone();
    for (k, v) in extra_env {
        env.insert(k.clone(), v.clone());
    }

    let mut all_outcomes: Vec<StepOutcome> = Vec::new();

    for layer in &layers {
        // Build one future per node in this layer.
        let futs: Vec<_> =
            layer.iter().map(|&idx| run_node(&flow.nodes[idx], &env, base_dir)).collect();

        let layer_results = join_all(futs).await;

        // Merge captures into `env` in deterministic order (node order, then
        // inner-step order within parallel groups).
        for result in layer_results {
            let outcomes = result?;
            for outcome in outcomes {
                for (var, val) in &outcome.captures {
                    env.insert(var.clone(), val.clone());
                }
                all_outcomes.push(outcome);
            }
        }
    }

    let passed = all_outcomes.iter().filter(|o| o.passed).count();
    let failed = all_outcomes.len() - passed;
    Ok(FlowResult {
        outcomes: all_outcomes,
        passed,
        failed,
    })
}

// ─── Layer computation (topological sort) ────────────────────────────────────

/// Assign each node a zero-based layer index and group them.
///
/// `layer(node) = 1 + max(layer(dep) for dep in depends_on)`, or 0 when there
/// are no dependencies.  Returns an error if an unknown node ID is referenced
/// or a cycle is detected.
fn compute_layers(nodes: &[FlowNode]) -> Result<Vec<Vec<usize>>, String> {
    let n = nodes.len();
    let mut layer_of: Vec<Option<usize>> = vec![None; n];
    let mut visiting: Vec<bool> = vec![false; n];

    for i in 0..n {
        compute_node_layer(i, nodes, &mut layer_of, &mut visiting)?;
    }

    if n == 0 {
        return Ok(vec![]);
    }
    let max_layer = layer_of.iter().filter_map(|&l| l).max().unwrap_or(0);
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); max_layer + 1];
    for (i, &layer) in layer_of.iter().enumerate() {
        layers[layer.unwrap()].push(i);
    }
    Ok(layers)
}

fn compute_node_layer(
    idx: usize,
    nodes: &[FlowNode],
    layer_of: &mut Vec<Option<usize>>,
    visiting: &mut Vec<bool>,
) -> Result<usize, String> {
    if let Some(layer) = layer_of[idx] {
        return Ok(layer);
    }
    if visiting[idx] {
        return Err(format!("cycle detected at node '{}'", nodes[idx].id()));
    }
    visiting[idx] = true;

    // Clone the dep list to avoid borrowing `nodes` twice.
    let deps: Vec<String> = nodes[idx].depends_on().to_vec();
    let mut max_dep: Option<usize> = None;

    for dep_id in &deps {
        let dep_idx = nodes.iter().position(|n| n.id() == dep_id).ok_or_else(|| {
            format!("node '{}' depends_on unknown id '{}'", nodes[idx].id(), dep_id)
        })?;
        let dep_layer = compute_node_layer(dep_idx, nodes, layer_of, visiting)?;
        max_dep = Some(max_dep.map_or(dep_layer, |m| m.max(dep_layer)));
    }

    let layer = max_dep.map(|m| m + 1).unwrap_or(0);
    layer_of[idx] = Some(layer);
    visiting[idx] = false;
    Ok(layer)
}

// ─── Node execution ───────────────────────────────────────────────────────────

/// Execute one top-level node (step or parallel group).
///
/// Returns a `Vec<StepOutcome>` — one element for a plain step, one per inner
/// step for a parallel group.  Inner steps within the group are run
/// concurrently via a nested `join_all`.
async fn run_node<'a>(
    node: &'a FlowNode,
    env: &'a HashMap<String, String>,
    base_dir: &'a Path,
) -> Result<Vec<StepOutcome>, String> {
    match node {
        FlowNode::Step(step) => {
            let outcome = run_step(step, env, base_dir).await?;
            Ok(vec![outcome])
        },
        FlowNode::Parallel(group) => run_parallel(group, env, base_dir).await,
    }
}

/// Run all inner steps of a [`ParallelGroup`] concurrently.
async fn run_parallel<'a>(
    group: &'a ParallelGroup,
    env: &'a HashMap<String, String>,
    base_dir: &'a Path,
) -> Result<Vec<StepOutcome>, String> {
    let futs: Vec<_> = group.steps.iter().map(|step| run_step(step, env, base_dir)).collect();

    let results = join_all(futs).await;

    let mut outcomes = Vec::with_capacity(results.len());
    for result in results {
        outcomes.push(result?);
    }
    Ok(outcomes)
}

// ─── Step execution ───────────────────────────────────────────────────────────

/// Execute a single [`StepDef`]: read the `.http` file, find the entry,
/// build a runner, run the request, evaluate captures.
async fn run_step(
    step: &StepDef,
    env: &HashMap<String, String>,
    base_dir: &Path,
) -> Result<StepOutcome, String> {
    // 1. Read the .http file.
    let file_path = base_dir.join(&step.file);
    let content = std::fs::read_to_string(&file_path)
        .map_err(|e| format!("step '{}': cannot read {}: {}", step.id, file_path.display(), e))?;
    let file_base = file_path.parent().unwrap_or(base_dir);

    // 2. Parse the collection and select the right entry.
    let cases = parse_collection(&content, env, file_base)?;

    let tc = match &step.entry {
        Some(entry_name) => {
            cases.into_iter().find(|tc| tc.name == entry_name.as_str()).ok_or_else(|| {
                format!("step '{}': entry '{}' not found in {}", step.id, entry_name, step.file)
            })?
        },
        None => cases
            .into_iter()
            .next()
            .ok_or_else(|| format!("step '{}': no entries in {}", step.id, step.file))?,
    };

    // 3. Build the runner with the URL prefix allowlist and current env.
    let allowed: Vec<String> = url_prefix(&tc.request.url).into_iter().collect();
    let mut builder = HttpTestRunner::builder().allowed_prefixes(allowed);
    for (k, v) in env {
        builder = builder.env(k.clone(), v.clone());
    }
    let mut runner =
        builder.build().map_err(|e| format!("step '{}': runner error: {}", step.id, e))?;

    // 4. Run the test.
    let result = runner.run_test(tc).await.map_err(|e| format!("step '{}': {}", step.id, e))?;

    // 5. Evaluate capture bindings against the response.
    let captures: Vec<(String, String)> = step
        .capture
        .iter()
        .filter_map(|b| eval_capture(&b.expr, &result).map(|v| (b.var.clone(), v)))
        .collect();

    Ok(StepOutcome {
        id: step.id.clone(),
        name: result.name.clone(),
        passed: result.passed,
        result,
        captures,
    })
}

// ─── Capture evaluation ───────────────────────────────────────────────────────

/// Extract a value from a `TestResult` according to a [`CaptureExpr`].
///
/// Returns `None` if the response is absent, the JSON path does not exist, or
/// the header is not found.
fn eval_capture(expr: &CaptureExpr, result: &TestResult) -> Option<String> {
    let resp = result.response.as_ref()?;
    match expr {
        CaptureExpr::Status => Some(resp.status.to_string()),

        CaptureExpr::Header(name) => {
            resp.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
        },

        CaptureExpr::BodyPath(path) => {
            let body: Value = serde_json::from_str(&resp.body).ok()?;
            let mut cur = &body;
            for key in path {
                cur = cur.get(key.as_str())?;
            }
            // Return strings without surrounding quotes; other types as JSON.
            Some(match cur {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
        },
    }
}

// ─── URL prefix helper ────────────────────────────────────────────────────────

/// Extract `scheme://host` from a URL string for the HTTP allowlist.
fn url_prefix(url: &str) -> Option<String> {
    let after_scheme = url.find("://").map(|i| i + 3)?;
    let rest = &url[after_scheme..];
    let host_end = rest.find('/').map(|i| after_scheme + i).unwrap_or(url.len());
    Some(url[..host_end].to_string())
}
