use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// A single HTTP request specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequest {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            method: "GET".into(),
            headers: vec![],
            body: None,
        }
    }

    pub fn post(url: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            method: "POST".into(),
            headers: vec![],
            body: Some(body.into()),
        }
    }
}

impl Default for HttpRequest {
    fn default() -> Self {
        Self::get("")
    }
}

/// One test case: an HTTP request with optional pre/post scripts and per-run
/// capability overrides.
#[derive(Default)]
pub struct TestCase {
    pub name: String,
    pub request: HttpRequest,
    pub pre_script: Option<String>,
    pub post_script: Option<String>,
    pub modules: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
    pub timeout_override: Option<Duration>,
    pub kv_key_prefix: Option<String>,
    pub http_allowed_prefixes: Option<Vec<String>>,
    pub output_file: Option<String>,
    pub response_file: Option<String>,
}
