//! Bruno `.bru` file adapter → [`MockCollection`].
//!
//! Parses the request URL/method and any `response` blocks from `.bru` files.
//! For directory input, use [`BrunoAdapter::ingest_dir`].

use std::path::Path;

use crate::model::{
    MockCollection, MockResponse, MockRoute, ResponseBody, RouteMethod, SelectionStrategy,
    body_from_str,
};

use super::IngestAdapter;

pub struct BrunoAdapter;

impl IngestAdapter for BrunoAdapter {
    fn ingest(&self, source: &str) -> anyhow::Result<MockCollection> {
        let route = parse_bru_file(source, "unknown");
        let routes = route.into_iter().collect();
        Ok(MockCollection { name: String::new(), routes })
    }
}

impl BrunoAdapter {
    /// Ingest all `.bru` files in a directory recursively.
    pub fn ingest_dir(path: &Path) -> anyhow::Result<MockCollection> {
        let mut routes = Vec::new();
        let mut name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Bruno Collection")
            .to_string();

        collect_bru_routes(path, &mut routes, &mut name)?;
        Ok(MockCollection { name, routes })
    }
}

fn collect_bru_routes(
    dir: &Path,
    routes: &mut Vec<MockRoute>,
    name: &mut String,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_bru_routes(&path, routes, name)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("bru") {
            let source = std::fs::read_to_string(&path)?;
            let file_name = path.file_stem().and_then(|n| n.to_str()).unwrap_or("?");
            if let Some(route) = parse_bru_file(&source, file_name) {
                routes.push(route);
            }
        }
    }
    Ok(())
}

/// Parse a single `.bru` file into a `MockRoute`.
///
/// Recognises these top-level blocks:
/// - `meta { name: ... }` → route id
/// - `get/post/put/… { url: ... }` → method + path
/// - `response { status: N; body: json { ... } }` → MockResponse
fn parse_bru_file(source: &str, fallback_name: &str) -> Option<MockRoute> {
    let mut id = fallback_name.to_string();
    let mut method: Option<String> = None;
    let mut url: Option<String> = None;
    let mut responses: Vec<MockResponse> = Vec::new();

    let mut lines = source.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();

        // meta block — handles both inline `meta { name: X }` and multi-line
        if trimmed.starts_with("meta") && trimmed.contains('{') {
            let after = trimmed.splitn(2, '{').nth(1).unwrap_or("").trim();
            if after.contains('}') {
                // Inline block
                let inner = after.split('}').next().unwrap_or("").trim();
                for part in inner.split(';') {
                    if let Some(val) = part.trim().strip_prefix("name:") {
                        id = val.trim().to_string();
                    }
                }
            } else {
                // Multi-line block
                while let Some(inner) = lines.next() {
                    let t = inner.trim();
                    if t == "}" { break; }
                    if let Some(val) = t.strip_prefix("name:") {
                        id = val.trim().to_string();
                    }
                }
            }
        }

        // HTTP method block — handles both inline and multi-line
        const METHODS: &[&str] = &["get", "post", "put", "patch", "delete", "head", "options"];
        if let Some(&m) = METHODS.iter().find(|&&m| {
            trimmed.starts_with(m)
                && trimmed[m.len()..].trim_start().starts_with('{')
        }) {
            method = Some(m.to_uppercase());
            let after = trimmed.splitn(2, '{').nth(1).unwrap_or("").trim();
            if after.contains('}') {
                // Inline
                let inner = after.split('}').next().unwrap_or("").trim();
                for part in inner.split(';') {
                    if let Some(val) = part.trim().strip_prefix("url:") {
                        url = Some(val.trim().to_string());
                    }
                }
            } else {
                for inner in lines.by_ref() {
                    let t = inner.trim();
                    if t == "}" { break; }
                    if let Some(val) = t.strip_prefix("url:") {
                        url = Some(val.trim().to_string());
                    }
                }
            }
        }

        // response block
        if trimmed == "response {" || trimmed.starts_with("response {") {
            responses.push(parse_response_block(&mut lines));
        }
    }

    let method = method?;
    let url = url?;
    let path = url_to_path(&url);

    let selection = if responses.len() > 1 {
        SelectionStrategy::RoundRobin
    } else {
        SelectionStrategy::First
    };

    // If no response blocks found, add a default 200 empty response
    if responses.is_empty() {
        responses.push(MockResponse::default());
    }

    Some(MockRoute {
        id,
        method: RouteMethod::Specific(method),
        path,
        matchers: vec![],
        responses,
        selection,
    })
}

fn parse_response_block(
    lines: &mut std::iter::Peekable<std::str::Lines<'_>>,
) -> MockResponse {
    let mut status: u16 = 200;
    let mut body_str: Option<String> = None;
    let mut headers: Vec<(String, String)> = Vec::new();

    while let Some(line) = lines.next() {
        let t = line.trim();
        if t == "}" { break; }

        if let Some(val) = t.strip_prefix("status:") {
            status = val.trim().parse().unwrap_or(200);
        } else if t == "body:json {" || t.starts_with("body {") || t.starts_with("body:json {") {
            let mut json_lines = Vec::new();
            while let Some(inner) = lines.next() {
                let it = inner.trim();
                if it == "}" { break; }
                json_lines.push(it.to_string());
            }
            body_str = Some(json_lines.join("\n"));
            headers.push(("Content-Type".to_string(), "application/json".to_string()));
        } else if let Some(kv) = t.strip_prefix("headers {") {
            // inline headers block
            let _block = if kv.trim() == "{" { String::new() } else { kv.to_string() };
            while let Some(inner) = lines.next() {
                let it = inner.trim();
                if it == "}" { break; }
                if let Some(pos) = it.find(':') {
                    headers.push((it[..pos].trim().to_string(), it[pos+1..].trim().to_string()));
                }
            }
        }
    }

    let body = body_str
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(body_from_str)
        .unwrap_or(ResponseBody::Empty);

    MockResponse { status, headers, body, delay_ms: 0 }
}

fn url_to_path(url: &str) -> String {
    let path = if url.contains("://") {
        let after = url.splitn(2, "://").nth(1).unwrap_or(url);
        after.find('/').map(|i| &after[i..]).unwrap_or("/")
    } else if url.starts_with('/') {
        url
    } else {
        return format!("/{}", url.split('?').next().unwrap_or(url));
    };

    // Strip query string and normalise :param → {param}
    let path = path.split('?').next().unwrap_or(path);
    path.split('/')
        .map(|seg| {
            if let Some(name) = seg.strip_prefix(':') {
                format!("{{{}}}", name)
            } else {
                seg.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
meta {
  name: List users
}

get {
  url: http://localhost:3000/api/users
}

response {
  status: 200
  body:json {
    [{"id":1,"name":"Alice"}]
  }
}
"#;

    #[test]
    fn single_file() {
        let route = parse_bru_file(SAMPLE, "list_users").unwrap();
        assert_eq!(route.id, "List users");
        assert_eq!(route.path, "/api/users");
        assert!(matches!(route.method, RouteMethod::Specific(ref m) if m == "GET"));
        assert_eq!(route.responses.len(), 1);
        assert_eq!(route.responses[0].status, 200);
    }

    #[test]
    fn colon_param_normalised() {
        let bru = "meta { name: X }\nget { url: http://host/users/:id }\n";
        let route = parse_bru_file(bru, "x").unwrap();
        assert_eq!(route.path, "/users/{id}");
    }
}
