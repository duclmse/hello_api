//! Postman Collection v2.1 → [`MockCollection`].
//!
//! Flattens the nested folder tree; maps each item's saved `response` examples
//! to `MockResponse` objects.  Items with no saved responses are skipped.

use serde::Deserialize;

use crate::model::{
    MockCollection, MockResponse, MockRoute, ResponseBody, RouteMethod, SelectionStrategy,
    body_from_str,
};

use super::IngestAdapter;

pub struct PostmanAdapter;

impl IngestAdapter for PostmanAdapter {
    fn ingest(&self, source: &str) -> anyhow::Result<MockCollection> {
        let raw: RawCollection = serde_json::from_str(source)?;
        Ok(collection_from_raw(raw))
    }
}

// ── Minimal serde types ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RawCollection {
    info: RawInfo,
    #[serde(default)]
    item: Vec<RawItem>,
}

#[derive(Deserialize)]
struct RawInfo {
    name: String,
}

#[derive(Deserialize)]
struct RawItem {
    name: String,
    /// Sub-items (folder).
    item: Option<Vec<RawItem>>,
    request: Option<RawRequest>,
    /// Saved response examples.
    #[serde(default)]
    response: Vec<RawResponse>,
}

#[derive(Deserialize)]
struct RawRequest {
    #[serde(default = "default_get")]
    method: String,
    url: Option<RawUrl>,
}

fn default_get() -> String {
    "GET".to_string()
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawUrl {
    Simple(String),
    Object(RawUrlObject),
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct RawUrlObject {
    raw: Option<String>,
    #[serde(default)]
    path: Vec<serde_json::Value>,
    #[serde(default)]
    host: Vec<serde_json::Value>,
    #[serde(default)]
    variable: Vec<RawPathVariable>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct RawPathVariable {
    key: String,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct RawResponse {
    name: Option<String>,
    #[serde(default)]
    code: u16,
    status: Option<String>,
    #[serde(default)]
    header: Vec<RawHeader>,
    body: Option<String>,
    #[serde(rename = "originalRequest")]
    original_request: Option<RawRequest>,
}

#[derive(Deserialize)]
struct RawHeader {
    key: String,
    value: String,
    #[serde(default)]
    disabled: bool,
}

// ── Conversion ────────────────────────────────────────────────────────────────

fn collection_from_raw(raw: RawCollection) -> MockCollection {
    let mut routes = Vec::new();
    flatten_items(&raw.item, &mut routes);
    MockCollection { name: raw.info.name, routes }
}

fn flatten_items(items: &[RawItem], out: &mut Vec<MockRoute>) {
    for item in items {
        if let Some(children) = &item.item {
            flatten_items(children, out);
        } else if let Some(req) = &item.request {
            if item.response.is_empty() {
                continue;
            }
            let Some(path) = extract_path(req) else { continue };
            let responses: Vec<MockResponse> =
                item.response.iter().map(|r| response_from_raw(r, req)).collect();

            let selection = if responses.len() == 1 {
                SelectionStrategy::First
            } else {
                SelectionStrategy::RoundRobin
            };

            out.push(MockRoute {
                id: item.name.clone(),
                method: RouteMethod::from_str(&req.method),
                path,
                matchers: vec![],
                responses,
                selection,
            });
        }
    }
}

fn response_from_raw(r: &RawResponse, _fallback_req: &RawRequest) -> MockResponse {
    let status = if r.code == 0 {
        // Some collections use "status" string without a code
        r.status
            .as_deref()
            .and_then(|s| status_string_to_code(s))
            .unwrap_or(200)
    } else {
        r.code
    };

    let headers: Vec<(String, String)> = r
        .header
        .iter()
        .filter(|h| !h.disabled)
        .map(|h| (h.key.clone(), h.value.clone()))
        .collect();

    let body = r
        .body
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(body_from_str)
        .unwrap_or(ResponseBody::Empty);

    MockResponse { status, headers, body, delay_ms: 0 }
}

fn extract_path(req: &RawRequest) -> Option<String> {
    let url = req.url.as_ref()?;
    let raw_url = match url {
        RawUrl::Simple(s) => s.clone(),
        RawUrl::Object(o) => o.raw.clone().unwrap_or_else(|| {
            // Reconstruct from path segments
            let segs: Vec<String> = o
                .path
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            format!("/{}", segs.join("/"))
        }),
    };

    // Extract just the path component and normalise :param → {param}
    let path = if raw_url.contains("://") {
        let after = raw_url.splitn(2, "://").nth(1)?;
        let after_host = after.find('/').map(|i| &after[i..]).unwrap_or("/");
        after_host.split('?').next().unwrap_or("/").to_string()
    } else if raw_url.starts_with('/') {
        raw_url.split('?').next().unwrap_or(&raw_url).to_string()
    } else {
        format!("/{}", raw_url.split('?').next().unwrap_or(&raw_url))
    };

    // :param → {param}
    let path = colon_to_brace(&path);
    Some(path)
}

fn colon_to_brace(path: &str) -> String {
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

fn status_string_to_code(s: &str) -> Option<u16> {
    match s.to_lowercase().as_str() {
        "ok" => Some(200),
        "created" => Some(201),
        "no content" => Some(204),
        "bad request" => Some(400),
        "unauthorized" => Some(401),
        "forbidden" => Some(403),
        "not found" => Some(404),
        "internal server error" => Some(500),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
  "info": { "name": "My API", "schema": "https://schema.getpostman.com/v2.1" },
  "item": [
    {
      "name": "List users",
      "request": {
        "method": "GET",
        "url": { "raw": "https://api.example.com/users", "path": ["users"] }
      },
      "response": [
        {
          "name": "Success",
          "code": 200,
          "header": [{ "key": "Content-Type", "value": "application/json" }],
          "body": "{\"users\": []}"
        }
      ]
    },
    {
      "name": "Folder",
      "item": [
        {
          "name": "Get user",
          "request": {
            "method": "GET",
            "url": { "raw": "https://api.example.com/users/:id", "path": ["users",":id"] }
          },
          "response": [
            { "name": "ok", "code": 200, "body": "{\"id\":1}" },
            { "name": "not found", "code": 404, "body": "{\"error\":\"not found\"}" }
          ]
        }
      ]
    }
  ]
}"#;

    #[test]
    fn flat_collection() {
        let col = PostmanAdapter.ingest(SAMPLE).unwrap();
        assert_eq!(col.name, "My API");
        assert_eq!(col.routes.len(), 2);
    }

    #[test]
    fn nested_folders() {
        let col = PostmanAdapter.ingest(SAMPLE).unwrap();
        let get_user = col.routes.iter().find(|r| r.id == "Get user").unwrap();
        assert_eq!(get_user.path, "/users/{id}");
    }

    #[test]
    fn round_robin_for_multiple_responses() {
        let col = PostmanAdapter.ingest(SAMPLE).unwrap();
        let get_user = col.routes.iter().find(|r| r.id == "Get user").unwrap();
        assert_eq!(get_user.responses.len(), 2);
        assert_eq!(get_user.selection, SelectionStrategy::RoundRobin);
    }
}
