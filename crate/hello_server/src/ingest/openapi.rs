//! OpenAPI 3.x / Swagger 2.0 → [`MockCollection`].
//!
//! For each `paths[path][method]` operation, one [`MockRoute`] is produced.
//! Each response status code with an extractable body becomes a [`MockResponse`].
//! Multiple status codes → [`SelectionStrategy::MatchStatus`].


use anyhow::Context;
use serde_json::Value;

use crate::model::{
    MockCollection, MockResponse, MockRoute, ResponseBody, RouteMethod, SelectionStrategy,
    body_from_str,
};

use super::IngestAdapter;

pub struct OpenApiAdapter;

impl IngestAdapter for OpenApiAdapter {
    fn ingest(&self, source: &str) -> anyhow::Result<MockCollection> {
        parse_openapi(source)
    }
}

fn parse_openapi(source: &str) -> anyhow::Result<MockCollection> {
    let root: Value = serde_yaml::from_str(source).context("parse OpenAPI YAML/JSON")?;

    let name = root
        .pointer("/info/title")
        .and_then(Value::as_str)
        .unwrap_or("API Mock")
        .to_string();

    let paths = match root.get("paths").and_then(Value::as_object) {
        Some(p) => p,
        None => return Ok(MockCollection { name, routes: vec![] }),
    };

    let mut routes = Vec::new();
    const METHODS: &[&str] = &["get", "post", "put", "patch", "delete", "head", "options"];

    for (path, path_item) in paths {
        let matchit_path = openapi_path_to_matchit(path);
        for &method in METHODS {
            let Some(op) = path_item.get(method).and_then(Value::as_object) else {
                continue;
            };

            let id = op
                .get("operationId")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{} {}", method.to_uppercase(), path));

            let responses = extract_responses(op);
            if responses.is_empty() {
                continue;
            }

            let selection = if responses.len() == 1 {
                SelectionStrategy::First
            } else {
                SelectionStrategy::MatchStatus
            };

            routes.push(MockRoute {
                id,
                method: RouteMethod::Specific(method.to_uppercase()),
                path: matchit_path.clone(),
                matchers: vec![],
                responses,
                selection,
            });
        }
    }

    Ok(MockCollection { name, routes })
}

fn openapi_path_to_matchit(path: &str) -> String {
    // OpenAPI uses {param}, matchit 0.8 also uses {param} — pass through.
    path.to_string()
}

fn extract_responses(op: &serde_json::Map<String, Value>) -> Vec<MockResponse> {
    let Some(responses) = op.get("responses").and_then(Value::as_object) else {
        return vec![];
    };

    let mut out = Vec::new();
    for (status_str, resp_val) in responses {
        let status: u16 = status_str
            .trim_matches('\'')
            .trim_matches('"')
            .parse()
            .unwrap_or(200);

        let body = extract_response_body(resp_val);
        let headers = extract_response_headers(resp_val);

        out.push(MockResponse {
            status,
            headers,
            body,
            delay_ms: 0,
        });
    }

    // Sort by status code for determinism
    out.sort_by_key(|r| r.status);
    out
}

fn extract_response_body(resp: &Value) -> ResponseBody {
    // Priority order per spec §3.3
    let content = resp.get("content").and_then(Value::as_object);

    if let Some(content) = content {
        // Try application/json first, then any other media type
        let media = content
            .get("application/json")
            .or_else(|| content.values().next());

        if let Some(media) = media {
            // 1. examples.*.value (first entry)
            if let Some(examples) = media.get("examples").and_then(Value::as_object) {
                if let Some(ex) = examples.values().next() {
                    if let Some(val) = ex.get("value") {
                        return ResponseBody::Json(val.clone());
                    }
                }
            }
            // 2. example
            if let Some(ex) = media.get("example") {
                return json_or_text(ex);
            }
            // 3. schema.example
            if let Some(ex) =
                media.get("schema").and_then(|s| s.get("example"))
            {
                return json_or_text(ex);
            }
        }
    }

    // 4. description as text
    if let Some(desc) = resp.get("description").and_then(Value::as_str) {
        if !desc.is_empty() {
            return ResponseBody::Text(desc.to_string());
        }
    }

    ResponseBody::Empty
}

fn extract_response_headers(resp: &Value) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    if let Some(hmap) = resp.get("headers").and_then(Value::as_object) {
        for (name, hval) in hmap {
            // OpenAPI response header objects have schema.example or example
            let val = hval
                .get("example")
                .or_else(|| hval.get("schema").and_then(|s| s.get("example")))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if !val.is_empty() {
                headers.push((name.clone(), val));
            }
        }
    }
    // Inject Content-Type for JSON content
    if let Some(content) = resp.get("content").and_then(Value::as_object) {
        if content.contains_key("application/json") {
            headers.push(("Content-Type".to_string(), "application/json".to_string()));
        }
    }
    headers
}

fn json_or_text(v: &Value) -> ResponseBody {
    match v {
        Value::String(s) => body_from_str(s),
        other => ResponseBody::Json(other.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PETSTORE_YAML: &str = r#"
openapi: 3.0.0
info:
  title: Petstore
  version: 1.0.0
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        '200':
          description: A list of pets
          content:
            application/json:
              example:
                - id: 1
                  name: Fluffy
        '500':
          description: Unexpected error
          content:
            application/json:
              example:
                error: internal server error
  /pets/{petId}:
    get:
      operationId: getPet
      responses:
        '200':
          description: A pet
          content:
            application/json:
              schema:
                example:
                  id: 1
                  name: Fluffy
"#;

    #[test]
    fn petstore_routes() {
        let col = OpenApiAdapter.ingest(PETSTORE_YAML).unwrap();
        assert_eq!(col.name, "Petstore");
        // /pets GET has 2 status codes; /pets/{petId} GET has 1
        assert_eq!(col.routes.len(), 2);

        let list = col.routes.iter().find(|r| r.id == "listPets").unwrap();
        assert_eq!(list.responses.len(), 2);
        assert!(matches!(list.selection, SelectionStrategy::MatchStatus));

        let get = col.routes.iter().find(|r| r.id == "getPet").unwrap();
        assert_eq!(get.responses.len(), 1);
        assert!(matches!(get.selection, SelectionStrategy::First));
    }

    #[test]
    fn path_param_normalisation() {
        let yaml = r#"
openapi: 3.0.0
info: { title: T, version: 1 }
paths:
  /pets/{petId}/photos/{photoId}:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: {}
"#;
        let col = OpenApiAdapter.ingest(yaml).unwrap();
        assert_eq!(col.routes[0].path, "/pets/{petId}/photos/{photoId}");
    }

    #[test]
    fn empty_paths() {
        let yaml = "openapi: 3.0.0\ninfo: {title: T, version: 1}\npaths: {}";
        let col = OpenApiAdapter.ingest(yaml).unwrap();
        assert_eq!(col.routes.len(), 0);
    }
}
