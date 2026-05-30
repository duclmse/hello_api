//! `.http` file adapter → [`MockCollection`].
//!
//! Each request entry in the file defines one mock route.  The request line
//! provides `method + path`; the optional entry body becomes the response body.
//! Response metadata is given via `### @` tags:
//!
//! ```text
//! ### @response 200
//! ### @response-body {"users":[]}
//! ### @response-delay 50
//! ### @response-strategy round-robin
//!
//! GET /api/users
//!
//! ###
//! ```
//!
//! If no `@response` tag is present the entry is still included as a route with
//! a default `204 No Content` response (and a `WARN` is logged).

use hello_core::client_parser::request_collection;
use hello_core::http_request::{Body, Url, UrlSegment};

use crate::model::{
    MockCollection, MockResponse, MockRoute, ResponseBody, RouteMethod, SelectionStrategy,
    body_from_str,
};

use super::IngestAdapter;

pub struct HttpFileAdapter;

impl IngestAdapter for HttpFileAdapter {
    fn ingest(&self, source: &str) -> anyhow::Result<MockCollection> {
        let (_, entries) =
            request_collection(source).map_err(|e| anyhow::anyhow!("parse error: {:?}", e))?;

        let mut routes = Vec::new();
        for entry in entries {
            let req = &entry.request;
            let method = req.request_line.method;
            let path = url_to_matchit_path(&req.request_line.url);

            let params = &entry.metadata.params;

            let status: u16 = params
                .get("response")
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| {
                    log::warn!("entry {method} {path}: no @response tag, defaulting to 204");
                    204
                });

            // Body: @response-body param beats entry body
            let body: ResponseBody = if let Some(b) = params.get("response-body") {
                body_from_str(b)
            } else {
                match &req.body {
                    Some(Body::Raw(s)) => body_from_str(s),
                    Some(Body::File(path)) => ResponseBody::Text(format!("<file: {}>", path)),
                    _ => ResponseBody::Empty,
                }
            };

            let delay_ms: u64 = params
                .get("response-delay")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);

            let selection = params
                .get("response-strategy")
                .map(|s| SelectionStrategy::from_str(s))
                .unwrap_or_default();

            let id = entry.metadata.description.join(" ");
            let id = if id.trim().is_empty() {
                format!("{} {}", method, path)
            } else {
                id.trim().to_string()
            };

            routes.push(MockRoute {
                id,
                method: RouteMethod::from_str(method),
                path,
                matchers: vec![],
                responses: vec![MockResponse {
                    status,
                    headers: vec![],
                    body,
                    delay_ms,
                }],
                selection,
            });
        }

        Ok(MockCollection { name: String::new(), routes })
    }
}

/// Convert a parsed `Url` to a matchit-compatible path string.
///
/// Full URLs have the scheme+host stripped.
/// `{{param}}` placeholders are converted to `{param}`.
fn url_to_matchit_path(url: &Url<'_>) -> String {
    match url {
        Url::Raw(raw) => {
            let path = strip_host(raw);
            normalise_path(path)
        },
        Url::Segments { host, path, query_params } => {
            let mut out = String::new();
            // Accumulate all segments to get the full URL string first
            for seg in host.iter().chain(path.iter()).chain(query_params.iter()) {
                match seg {
                    UrlSegment::Text(t) => out.push_str(t),
                    UrlSegment::Variable(v) => {
                        out.push_str("{{");
                        out.push_str(v);
                        out.push_str("}}");
                    },
                }
            }
            let path = strip_host(&out);
            // Remove query string
            let path = path.split('?').next().unwrap_or(path);
            normalise_path(path)
        },
    }
}

fn strip_host(url: &str) -> &str {
    if url.contains("://") {
        let after = url.splitn(2, "://").nth(1).unwrap_or(url);
        after.find('/').map(|i| &after[i..]).unwrap_or("/")
    } else if url.starts_with('/') {
        url
    } else {
        url
    }
}

fn normalise_path(path: &str) -> String {
    // {{param}} → {param}
    path.replace("{{", "{").replace("}}", "}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_entry() {
        let src = "### @response 200\n### @response-body {\"users\":[]}\n\nGET /api/users\n";
        let col = HttpFileAdapter.ingest(src).unwrap();
        assert_eq!(col.routes.len(), 1);
        let r = &col.routes[0];
        assert_eq!(r.path, "/api/users");
        assert!(matches!(r.method, RouteMethod::Specific(ref m) if m == "GET"));
        assert_eq!(r.responses[0].status, 200);
        assert!(matches!(&r.responses[0].body, ResponseBody::Json(_)));
    }

    #[test]
    fn no_response_tag_defaults_to_204() {
        let src = "GET /health\n";
        let col = HttpFileAdapter.ingest(src).unwrap();
        // entry parsed but status defaults to 204
        assert_eq!(col.routes[0].responses[0].status, 204);
    }

    #[test]
    fn multiple_entries() {
        // Entries are separated by ###; the separator must be immediately followed
        // by the next entry's metadata block (no blank line between ### and ###).
        let src = concat!(
            "### @response 200\n\nGET /users\n\n###\n",
            "### @response 201\n\nPOST /users\n",
        );
        let col = HttpFileAdapter.ingest(src).unwrap();
        assert_eq!(col.routes.len(), 2);
    }

    #[test]
    fn path_param_normalised() {
        let src = "### @response 200\n\nGET /users/{{id}}\n";
        let col = HttpFileAdapter.ingest(src).unwrap();
        assert_eq!(col.routes[0].path, "/users/{id}");
    }

    #[test]
    fn full_url_stripped_to_path() {
        let src = "### @response 200\n\nGET https://api.example.com/v1/users\n";
        let col = HttpFileAdapter.ingest(src).unwrap();
        assert_eq!(col.routes[0].path, "/v1/users");
    }
}
