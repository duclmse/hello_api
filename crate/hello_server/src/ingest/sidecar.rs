//! Sidecar `.mock.json` loader.
//!
//! A file `<spec-path>.mock.json` placed alongside the spec can override or
//! add routes.  Present entries replace an existing `(method, path)` pair;
//! absent entries are appended.

use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use crate::model::{
    MockCollection, MockResponse, MockRoute, ResponseBody, RouteMethod, SelectionStrategy,
};

#[derive(Deserialize)]
pub struct SidecarFile {
    #[serde(default)]
    pub overrides: Vec<SidecarRoute>,
}

#[derive(Deserialize)]
pub struct SidecarRoute {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub responses: Vec<SidecarResponse>,
    pub selection: Option<String>,
}

#[derive(Deserialize)]
pub struct SidecarResponse {
    #[serde(default = "default_200")]
    pub status: u16,
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
    pub body: Option<Value>,
    #[serde(default)]
    pub delay_ms: u64,
}

fn default_200() -> u16 {
    200
}

/// Try to load a sidecar file adjacent to `spec_path`.
pub fn load_sidecar(spec_path: &Path) -> anyhow::Result<Option<SidecarFile>> {
    let candidate = spec_path.with_extension("mock.json");
    if !candidate.exists() {
        return Ok(None);
    }
    let src = std::fs::read_to_string(&candidate)?;
    let sf: SidecarFile = serde_json::from_str(&src)?;
    Ok(Some(sf))
}

/// Merge sidecar overrides into a `MockCollection`.
pub fn apply_sidecar(collection: &mut MockCollection, sidecar: SidecarFile) {
    for sr in sidecar.overrides {
        let method = RouteMethod::from_str(&sr.method);
        let path = sr.path.clone();

        let responses: Vec<MockResponse> = sr
            .responses
            .into_iter()
            .map(|r| {
                let headers: Vec<(String, String)> = r.headers.into_iter().collect();
                let body = r
                    .body
                    .map(|v| match v {
                        Value::String(s) => ResponseBody::Text(s),
                        other => ResponseBody::Json(other),
                    })
                    .unwrap_or(ResponseBody::Empty);
                MockResponse {
                    status: r.status,
                    headers,
                    body,
                    delay_ms: r.delay_ms,
                }
            })
            .collect();

        let selection =
            sr.selection.as_deref().map(SelectionStrategy::from_str).unwrap_or_default();

        let route = MockRoute {
            id: format!("{} {} [sidecar]", sr.method.to_uppercase(), path),
            method,
            path: path.clone(),
            matchers: vec![],
            responses,
            selection,
        };

        // Replace existing or append
        if let Some(existing) =
            collection.routes.iter_mut().find(|r| r.path == path && r.method == route.method)
        {
            *existing = route;
        } else {
            collection.routes.push(route);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_col() -> MockCollection {
        MockCollection {
            name: "test".to_string(),
            routes: vec![MockRoute {
                id: "r1".to_string(),
                method: RouteMethod::Specific("GET".to_string()),
                path: "/users".to_string(),
                matchers: vec![],
                responses: vec![MockResponse {
                    status: 200,
                    ..Default::default()
                }],
                selection: SelectionStrategy::First,
            }],
        }
    }

    #[test]
    fn override_existing() {
        let mut col = make_col();
        let sf = serde_json::from_str::<SidecarFile>(
            r#"{
                "overrides": [
                    { "method": "GET", "path": "/users", "responses": [{ "status": 503 }] }
                ]
            }"#,
        )
        .unwrap();
        apply_sidecar(&mut col, sf);
        assert_eq!(col.routes.len(), 1);
        assert_eq!(col.routes[0].responses[0].status, 503);
    }

    #[test]
    fn add_new_route() {
        let mut col = make_col();
        let sf = serde_json::from_str::<SidecarFile>(
            r#"{
                "overrides": [
                    { "method": "POST", "path": "/users", "responses": [{ "status": 201 }] }
                ]
            }"#,
        )
        .unwrap();
        apply_sidecar(&mut col, sf);
        assert_eq!(col.routes.len(), 2);
    }
}
