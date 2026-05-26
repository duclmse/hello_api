//! Parser for the `.flow` orchestration file format.
//!
//! Accepts two formats:
//! - **Native**: named brace-delimited sections (the default `.flow` syntax).
//! - **JSON**: detected by a leading `{`; maps directly onto the same `FlowDef`
//!   types. JSON nodes carry a `"type"` field (`"step"` or `"parallel"`).
//!
//! JSON schema summary:
//! ```json
//! {
//!   "name": "My Flow",
//!   "description": "optional",
//!   "env": { "key": "value" },
//!   "nodes": [
//!     { "type": "step", "id": "s1", "file": "a.http", "entry": "name",
//!       "depends_on": ["prev"], "capture": { "var": "res.body.field" } },
//!     { "type": "parallel", "id": "p1", "depends_on": ["s1"],
//!       "steps": [{ "id": "s2", "file": "b.http" }] }
//!   ]
//! }
//! ```

use std::collections::HashMap;

use serde_json::Value;

use crate::flow::{CaptureBinding, CaptureExpr, FlowDef, FlowNode, ParallelGroup, StepDef};

// ─── Public entry point ───────────────────────────────────────────────────────

/// Parse a `.flow` file from a string.
///
/// Accepts both the native brace-delimited format and JSON (auto-detected by a
/// leading `{`).  Returns `Err` if a required field (`step.file`) is missing, a
/// `capture` expression is malformed, or a `depends_on` list is unclosed.
/// Cycle detection is deferred to the runner.
pub fn parse_flow(input: &str) -> Result<FlowDef, String> {
    if input.trim_start().starts_with('{') {
        return parse_flow_json(input);
    }
    parse_flow_native(input)
}

// ─── JSON parser ─────────────────────────────────────────────────────────────

fn parse_flow_json(input: &str) -> Result<FlowDef, String> {
    let root: Value =
        serde_json::from_str(input).map_err(|e| format!("flow JSON parse error: {e}"))?;
    let obj = root.as_object().ok_or("flow JSON must be an object")?;

    let name = obj.get("name").and_then(Value::as_str).unwrap_or("Unnamed Flow").to_string();

    let description = obj.get("description").and_then(Value::as_str).map(str::to_string);

    let env: HashMap<String, String> = obj
        .get("env")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect()
        })
        .unwrap_or_default();

    let nodes = obj
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or("flow JSON must have a 'nodes' array")?
        .iter()
        .map(json_node)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(FlowDef {
        name,
        description,
        env,
        nodes,
    })
}

fn json_node(v: &Value) -> Result<FlowNode, String> {
    let obj = v.as_object().ok_or("flow node must be a JSON object")?;
    match obj.get("type").and_then(Value::as_str) {
        Some("step") => Ok(FlowNode::Step(json_step(obj)?)),
        Some("parallel") => Ok(FlowNode::Parallel(json_parallel(obj)?)),
        Some(t) => Err(format!("unknown node type {t:?}; expected \"step\" or \"parallel\"")),
        None => Err("flow node must have a \"type\" field".to_string()),
    }
}

fn json_step(obj: &serde_json::Map<String, Value>) -> Result<StepDef, String> {
    let id =
        obj.get("id").and_then(Value::as_str).ok_or("step must have an \"id\" field")?.to_string();

    let file = obj
        .get("file")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("step '{id}' missing required \"file\" field"))?
        .to_string();

    let entry = obj.get("entry").and_then(Value::as_str).map(str::to_string);
    let depends_on = json_depends_on(obj.get("depends_on"), &id)?;

    let capture = obj
        .get("capture")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(var, expr_v)| {
                    let expr_str = expr_v.as_str().ok_or_else(|| {
                        format!("step '{id}' capture: value for '{var}' must be a string")
                    })?;
                    let expr = parse_capture_expr(expr_str)
                        .map_err(|e| format!("step '{id}' capture var '{var}': {e}"))?;
                    Ok(CaptureBinding {
                        var: var.clone(),
                        expr,
                    })
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .transpose()?
        .unwrap_or_default();

    Ok(StepDef {
        id,
        file,
        entry,
        depends_on,
        capture,
    })
}

fn json_parallel(obj: &serde_json::Map<String, Value>) -> Result<ParallelGroup, String> {
    let id = obj
        .get("id")
        .and_then(Value::as_str)
        .ok_or("parallel must have an \"id\" field")?
        .to_string();

    let depends_on = json_depends_on(obj.get("depends_on"), &id)?;

    let steps = obj
        .get("steps")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("parallel '{id}' must have a \"steps\" array"))?
        .iter()
        .map(|s| {
            s.as_object()
                .ok_or_else(|| format!("parallel '{id}': each step must be a JSON object"))
                .and_then(json_step)
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ParallelGroup {
        id,
        depends_on,
        steps,
    })
}

fn json_depends_on(v: Option<&Value>, ctx: &str) -> Result<Vec<String>, String> {
    match v {
        None => Ok(vec![]),
        Some(Value::String(s)) => Ok(vec![s.clone()]),
        Some(Value::Array(arr)) => arr
            .iter()
            .map(|item| {
                item.as_str()
                    .ok_or_else(|| format!("'{ctx}': depends_on items must be strings"))
                    .map(str::to_string)
            })
            .collect(),
        Some(other) => Err(format!("'{ctx}': depends_on must be a string or array, got {other}")),
    }
}

// ─── Native parser ────────────────────────────────────────────────────────────

fn parse_flow_native(input: &str) -> Result<FlowDef, String> {
    // bru_parser::parse_sections does not skip top-level comment lines;
    // strip them before handing off so that `# comment` lines don't
    // confuse the section-name scanner.
    let stripped: String = input
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.starts_with('#') && !t.starts_with("//")
        })
        .flat_map(|l| [l, "\n"])
        .collect();
    let sections = hello_core::adapters::bru_parser::parse_sections(&stripped);

    let mut flow_name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut env: HashMap<String, String> = HashMap::new();
    let mut nodes: Vec<FlowNode> = Vec::new();

    for section in &sections {
        let name = section.name.trim();

        if name == "flow" {
            for (k, v) in hello_core::adapters::bru_parser::parse_kv(&section.content) {
                match k.as_str() {
                    "name" => flow_name = Some(v),
                    "description" => description = Some(v),
                    _ => {},
                }
            }
        } else if name == "env" {
            for (k, v) in hello_core::adapters::bru_parser::parse_kv(&section.content) {
                env.insert(k, v);
            }
        } else if let Some(raw_id) = name.strip_prefix("step") {
            let id = raw_id.trim().to_string();
            if id.is_empty() {
                return Err("step block missing id".to_string());
            }
            nodes.push(FlowNode::Step(parse_step_content(&section.content, &id)?));
        } else if let Some(raw_id) = name.strip_prefix("parallel") {
            let id = raw_id.trim().to_string();
            if id.is_empty() {
                return Err("parallel block missing id".to_string());
            }
            nodes.push(FlowNode::Parallel(parse_parallel_content(&section.content, &id)?));
        }
        // Unknown section names are silently ignored.
    }

    Ok(FlowDef {
        name: flow_name.unwrap_or_else(|| "Unnamed Flow".to_string()),
        description,
        env,
        nodes,
    })
}

// ─── Block parsers ────────────────────────────────────────────────────────────

/// Parse the body of a `step id { ... }` block.
fn parse_step_content(content: &str, id: &str) -> Result<StepDef, String> {
    let mut file: Option<String> = None;
    let mut entry: Option<String> = None;
    let mut depends_on: Vec<String> = Vec::new();
    let mut capture: Vec<CaptureBinding> = Vec::new();

    let mut remaining = content;

    loop {
        remaining = skip_ws_comments(remaining);
        if remaining.is_empty() {
            break;
        }

        // Detect the optional `capture { ... }` sub-block.
        let trimmed = remaining.trim_start_matches([' ', '\t']);
        if let Some(after_kw) = trimmed.strip_prefix("capture") {
            let after_kw = after_kw.trim_start_matches([' ', '\t']);
            if after_kw.starts_with('{') {
                let offset = remaining.len() - after_kw.len();
                remaining = &remaining[offset..];
                let (rest, inner) = extract_balanced_braces(remaining)
                    .map_err(|e| format!("step '{}' capture block: {}", id, e))?;
                remaining = rest;
                capture = parse_capture_bindings(inner, id)?;
                continue;
            }
        }

        // Regular key: value line.
        let end = remaining.find('\n').unwrap_or(remaining.len());
        let line = remaining[..end].trim();
        remaining = if end < remaining.len() {
            &remaining[end + 1..]
        } else {
            ""
        };

        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }

        let colon =
            line.find(':').ok_or_else(|| format!("step '{}': unexpected line: {:?}", id, line))?;
        let key = line[..colon].trim();
        let val = line[colon + 1..].trim();

        match key {
            "file" => file = Some(val.to_string()),
            "entry" => entry = Some(val.to_string()),
            "depends_on" => depends_on = parse_depends_on(val)?,
            _ => {}, // forward-compatible: ignore unknown fields
        }
    }

    Ok(StepDef {
        id: id.to_string(),
        file: file.ok_or_else(|| format!("step '{}' missing required 'file' field", id))?,
        entry,
        depends_on,
        capture,
    })
}

/// Parse the body of a `parallel id { ... }` block.
///
/// The body may contain plain `key: value` lines (for `depends_on`) and
/// nested `step id { ... }` blocks (the concurrent inner steps).
fn parse_parallel_content(content: &str, group_id: &str) -> Result<ParallelGroup, String> {
    let mut depends_on: Vec<String> = Vec::new();
    let mut steps: Vec<StepDef> = Vec::new();
    let mut remaining = content;

    loop {
        remaining = skip_ws_comments(remaining);
        if remaining.is_empty() {
            break;
        }

        // Detect nested `step id {` block.
        let trimmed = remaining.trim_start_matches([' ', '\t']);
        if let Some(after_kw) = trimmed.strip_prefix("step") {
            let after_kw = after_kw.trim_start_matches([' ', '\t']);
            // step id must be followed by whitespace or '{', id must be non-empty
            if after_kw.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                let id_end = after_kw.find(['{', ' ', '\t', '\n']).unwrap_or(after_kw.len());
                let step_id = after_kw[..id_end].to_string();

                // Advance `remaining` to the opening brace.
                let brace_offset = remaining.find('{').ok_or_else(|| {
                    format!("parallel '{}': step '{}' missing opening '{{'", group_id, step_id)
                })?;
                remaining = &remaining[brace_offset..];

                let (rest, inner) = extract_balanced_braces(remaining)
                    .map_err(|e| format!("parallel '{}' step '{}': {}", group_id, step_id, e))?;
                remaining = rest;

                let step = parse_step_content(inner, &step_id)?;
                steps.push(step);
                continue;
            }
        }

        // Regular key: value line.
        let end = remaining.find('\n').unwrap_or(remaining.len());
        let line = remaining[..end].trim();
        remaining = if end < remaining.len() {
            &remaining[end + 1..]
        } else {
            ""
        };

        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }

        if let Some(colon) = line.find(':') {
            let key = line[..colon].trim();
            let val = line[colon + 1..].trim();
            if key == "depends_on" {
                depends_on = parse_depends_on(val)?;
            }
        }
    }

    Ok(ParallelGroup {
        id: group_id.to_string(),
        depends_on,
        steps,
    })
}

/// Parse the body of a `capture { ... }` block into a list of bindings.
fn parse_capture_bindings(content: &str, step_id: &str) -> Result<Vec<CaptureBinding>, String> {
    let mut bindings = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let colon = line
            .find(':')
            .ok_or_else(|| format!("step '{}' capture: invalid line: {:?}", step_id, line))?;
        let var = line[..colon].trim().to_string();
        let expr_str = line[colon + 1..].trim();
        let expr = parse_capture_expr(expr_str)
            .map_err(|e| format!("step '{}' capture var '{}': {}", step_id, var, e))?;
        bindings.push(CaptureBinding { var, expr });
    }

    Ok(bindings)
}

// ─── Expression and value parsers ─────────────────────────────────────────────

/// Parse a capture expression string (`res.status`, `res.body.a.b`,
/// `res.headers.ETag`) into a [`CaptureExpr`].
fn parse_capture_expr(s: &str) -> Result<CaptureExpr, String> {
    let s = s.trim();

    if s == "res.status" {
        return Ok(CaptureExpr::Status);
    }

    if let Some(rest) = s.strip_prefix("res.headers.") {
        if rest.is_empty() {
            return Err("res.headers requires a header name after the dot".to_string());
        }
        return Ok(CaptureExpr::Header(rest.to_string()));
    }

    if s == "res.body" {
        return Ok(CaptureExpr::BodyPath(vec![]));
    }

    if let Some(rest) = s.strip_prefix("res.body.") {
        let path: Vec<String> = rest.split('.').map(|p| p.to_string()).collect();
        return Ok(CaptureExpr::BodyPath(path));
    }

    Err(format!(
        "unknown capture expression {:?}; expected res.status, res.body[.path], or res.headers.name",
        s
    ))
}

/// Parse a `depends_on` value — either a bare identifier or a bracketed list.
///
/// Accepts:
/// - `single_id`
/// - `[id1, id2, id3]`
fn parse_depends_on(val: &str) -> Result<Vec<String>, String> {
    let val = val.trim();
    if val.is_empty() {
        return Err("depends_on value is empty".to_string());
    }

    if val.starts_with('[') {
        let close =
            val.rfind(']').ok_or_else(|| format!("depends_on: unclosed '[' in {:?}", val))?;
        let inner = &val[1..close];
        Ok(inner.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
    } else {
        Ok(vec![val.to_string()])
    }
}

// ─── Low-level helpers ────────────────────────────────────────────────────────

/// Skip leading whitespace (including newlines) and `#`/`//` comment lines.
fn skip_ws_comments(mut s: &str) -> &str {
    loop {
        s = s.trim_start_matches(|c: char| c.is_whitespace());
        if s.starts_with('#') || s.starts_with("//") {
            match s.find('\n') {
                Some(nl) => s = &s[nl + 1..],
                None => return "",
            }
        } else {
            return s;
        }
    }
}

/// Extract the content between the first pair of balanced braces.
///
/// Input must start with `{` (after optional leading space/tab).  Returns
/// `(remaining_input, content_between_braces)` on success.
fn extract_balanced_braces(input: &str) -> Result<(&str, &str), String> {
    let input = input.trim_start_matches([' ', '\t']);
    if !input.starts_with('{') {
        return Err(format!("expected '{{', got {:?}", &input[..input.len().min(12)]));
    }
    let inner = &input[1..];
    let mut depth = 1usize;
    for (i, ch) in inner.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((&inner[i + 1..], &inner[..i]));
                }
            },
            _ => {},
        }
    }
    Err("unmatched '{' in block".to_string())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_flow() {
        let input = r#"
flow {
  name: My Flow
}
step login {
  file: auth/login.http
}
"#;
        let flow = parse_flow(input).unwrap();
        assert_eq!(flow.name, "My Flow");
        assert_eq!(flow.nodes.len(), 1);
        let FlowNode::Step(step) = &flow.nodes[0] else {
            panic!("expected step")
        };
        assert_eq!(step.id, "login");
        assert_eq!(step.file, "auth/login.http");
    }

    #[test]
    fn env_block() {
        let input = r#"
flow { name: F }
env {
  base_url: https://api.example.com
  token: abc123
}
step s { file: req.http }
"#;
        let flow = parse_flow(input).unwrap();
        assert_eq!(flow.env.get("base_url").map(|s| s.as_str()), Some("https://api.example.com"));
        assert_eq!(flow.env.get("token").map(|s| s.as_str()), Some("abc123"));
    }

    #[test]
    fn step_with_entry_and_deps() {
        let input = r#"
flow { name: F }
step a { file: a.http }
step b {
  file: b.http
  entry: Do The Thing
  depends_on: a
}
"#;
        let flow = parse_flow(input).unwrap();
        let FlowNode::Step(b) = &flow.nodes[1] else {
            panic!()
        };
        assert_eq!(b.entry.as_deref(), Some("Do The Thing"));
        assert_eq!(b.depends_on, vec!["a"]);
    }

    #[test]
    fn depends_on_list() {
        let input = "flow { name: F }\nstep s {\n  file: f.http\n  depends_on: [a, b, c]\n}\n";
        let flow = parse_flow(input).unwrap();
        let FlowNode::Step(s) = &flow.nodes[0] else {
            panic!()
        };
        assert_eq!(s.depends_on, vec!["a", "b", "c"]);
    }

    #[test]
    fn step_with_capture() {
        let input = r#"
flow { name: F }
step login {
  file: auth.http
  capture {
    token: res.body.access_token
    status: res.status
    etag: res.headers.ETag
  }
}
"#;
        let flow = parse_flow(input).unwrap();
        let FlowNode::Step(s) = &flow.nodes[0] else {
            panic!()
        };
        assert_eq!(s.capture.len(), 3);

        assert_eq!(s.capture[0].var, "token");
        assert!(
            matches!(s.capture[0].expr, CaptureExpr::BodyPath(ref p) if p == &["access_token"])
        );

        assert_eq!(s.capture[1].var, "status");
        assert!(matches!(s.capture[1].expr, CaptureExpr::Status));

        assert_eq!(s.capture[2].var, "etag");
        assert!(matches!(s.capture[2].expr, CaptureExpr::Header(ref h) if h == "ETag"));
    }

    #[test]
    fn parallel_block() {
        let input = r#"
flow { name: F }
step register { file: register.http }
parallel notify {
  depends_on: register
  step email { file: email.http }
  step profile { file: profile.http }
}
step done {
  file: done.http
  depends_on: notify
}
"#;
        let flow = parse_flow(input).unwrap();
        assert_eq!(flow.nodes.len(), 3);

        let FlowNode::Parallel(g) = &flow.nodes[1] else {
            panic!("expected parallel")
        };
        assert_eq!(g.id, "notify");
        assert_eq!(g.depends_on, vec!["register"]);
        assert_eq!(g.steps.len(), 2);
        assert_eq!(g.steps[0].id, "email");
        assert_eq!(g.steps[1].id, "profile");

        let FlowNode::Step(done) = &flow.nodes[2] else {
            panic!()
        };
        assert_eq!(done.depends_on, vec!["notify"]);
    }

    #[test]
    fn capture_body_path() {
        let expr = parse_capture_expr("res.body.data.items").unwrap();
        assert!(matches!(expr, CaptureExpr::BodyPath(ref p) if p == &["data", "items"]));
    }

    #[test]
    fn capture_whole_body() {
        let expr = parse_capture_expr("res.body").unwrap();
        assert!(matches!(expr, CaptureExpr::BodyPath(ref p) if p.is_empty()));
    }

    #[test]
    fn unknown_capture_expr_errors() {
        assert!(parse_capture_expr("req.url").is_err());
    }

    #[test]
    fn missing_file_field_errors() {
        let input = "flow { name: F }\nstep s {\n  entry: Foo\n}\n";
        assert!(parse_flow(input).is_err());
    }

    #[test]
    fn json_minimal_flow() {
        let input = r#"{
  "name": "JSON Flow",
  "nodes": [
    { "type": "step", "id": "login", "file": "auth.http" }
  ]
}"#;
        let flow = parse_flow(input).unwrap();
        assert_eq!(flow.name, "JSON Flow");
        assert_eq!(flow.nodes.len(), 1);
        let FlowNode::Step(s) = &flow.nodes[0] else {
            panic!()
        };
        assert_eq!(s.id, "login");
        assert_eq!(s.file, "auth.http");
    }

    #[test]
    fn json_env_and_deps() {
        let input = r#"{
  "name": "F",
  "env": { "base_url": "https://httpbin.org", "token": "abc" },
  "nodes": [
    { "type": "step", "id": "a", "file": "a.http" },
    { "type": "step", "id": "b", "file": "b.http", "depends_on": "a" }
  ]
}"#;
        let flow = parse_flow(input).unwrap();
        assert_eq!(flow.env.get("base_url").map(String::as_str), Some("https://httpbin.org"));
        let FlowNode::Step(b) = &flow.nodes[1] else {
            panic!()
        };
        assert_eq!(b.depends_on, vec!["a"]);
    }

    #[test]
    fn json_parallel_node() {
        let input = r#"{
  "name": "F",
  "nodes": [
    { "type": "step", "id": "login", "file": "login.http" },
    {
      "type": "parallel", "id": "notify", "depends_on": ["login"],
      "steps": [
        { "id": "email", "file": "email.http" },
        { "id": "sms",   "file": "sms.http" }
      ]
    }
  ]
}"#;
        let flow = parse_flow(input).unwrap();
        let FlowNode::Parallel(g) = &flow.nodes[1] else {
            panic!()
        };
        assert_eq!(g.id, "notify");
        assert_eq!(g.depends_on, vec!["login"]);
        assert_eq!(g.steps.len(), 2);
    }

    #[test]
    fn json_capture_bindings() {
        let input = r#"{
  "name": "F",
  "nodes": [
    {
      "type": "step", "id": "s1", "file": "a.http",
      "capture": { "tok": "res.body.token", "status": "res.status" }
    }
  ]
}"#;
        let flow = parse_flow(input).unwrap();
        let FlowNode::Step(s) = &flow.nodes[0] else {
            panic!()
        };
        assert_eq!(s.capture.len(), 2);
        let tok = s.capture.iter().find(|b| b.var == "tok").unwrap();
        assert!(matches!(&tok.expr, CaptureExpr::BodyPath(p) if p == &["token"]));
        let st = s.capture.iter().find(|b| b.var == "status").unwrap();
        assert!(matches!(st.expr, CaptureExpr::Status));
    }

    #[test]
    fn json_missing_file_errors() {
        let input = r#"{ "name": "F", "nodes": [{ "type": "step", "id": "s" }] }"#;
        assert!(parse_flow(input).is_err());
    }

    #[test]
    fn json_missing_nodes_errors() {
        let input = r#"{ "name": "F" }"#;
        assert!(parse_flow(input).is_err());
    }

    #[test]
    fn comments_ignored() {
        let input = r#"
# top-level comment
flow {
  # inline comment
  name: Flow With Comments
}
// another style
step s {
  file: req.http
  # capture omitted
}
"#;
        let flow = parse_flow(input).unwrap();
        assert_eq!(flow.name, "Flow With Comments");
    }
}
