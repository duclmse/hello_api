//! Core data model.  All ingestion adapters normalise their format into these
//! types before the server starts.  Nothing outside this module knows about
//! `.http`, OpenAPI, Bruno, or Postman.

use serde::{Deserialize, Serialize};

// ── MockCollection ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MockCollection {
    pub name: String,
    pub routes: Vec<MockRoute>,
}

// ── MockRoute ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockRoute {
    /// Stable human-readable identifier (operationId, entry description, …).
    pub id: String,
    pub method: RouteMethod,
    /// matchit-compatible path pattern, e.g. `/users/{id}`.
    pub path: String,
    /// Optional guards evaluated after path matching.
    pub matchers: Vec<Matcher>,
    pub responses: Vec<MockResponse>,
    pub selection: SelectionStrategy,
}

// ── RouteMethod ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "UPPERCASE")]
pub enum RouteMethod {
    /// Matches the given HTTP method only.
    Specific(String),
    /// Matches any HTTP method.
    Any,
}

impl RouteMethod {
    pub fn from_str(s: &str) -> Self {
        let up = s.trim().to_uppercase();
        if up == "*" || up == "ANY" {
            Self::Any
        } else {
            Self::Specific(up)
        }
    }

    pub fn matches(&self, method: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Specific(m) => m.eq_ignore_ascii_case(method),
        }
    }
}

// ── Matcher ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Matcher {
    Header { name: String, value_pattern: Pattern },
    QueryParam { name: String, value_pattern: Pattern },
}

/// Pattern for request guard matching.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Pattern {
    Exact(String),
    Contains(String),
    /// Regex pattern stored as a string; compiled lazily at match time.
    Regex(String),
    /// Header / param must be present, any value accepted.
    Any,
}

impl Pattern {
    pub fn matches(&self, value: &str) -> bool {
        match self {
            Self::Exact(s) => value == s,
            Self::Contains(s) => value.contains(s.as_str()),
            // TODO: compile regex properly when `regex` crate is added
            Self::Regex(pat) => value.contains(pat.as_str()),
            Self::Any => true,
        }
    }
}

// ── MockResponse ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: ResponseBody,
    pub delay_ms: u64,
}

impl Default for MockResponse {
    fn default() -> Self {
        Self {
            status: 200,
            headers: vec![],
            body: ResponseBody::Empty,
            delay_ms: 0,
        }
    }
}

/// The response body payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResponseBody {
    Empty,
    Text(String),
    Json(serde_json::Value),
    /// Body with `{{param}}` placeholders filled from path/query params.
    Template(String),
}

impl ResponseBody {
    /// Serialise to bytes for HTTP transmission.
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Self::Empty => vec![],
            Self::Text(s) | Self::Template(s) => s.as_bytes().to_vec(),
            Self::Json(v) => v.to_string().into_bytes(),
        }
    }

    /// Infer a `Content-Type` value when no explicit header is set.
    pub fn inferred_content_type(&self) -> Option<&'static str> {
        match self {
            Self::Empty => None,
            Self::Text(_) | Self::Template(_) => Some("text/plain; charset=utf-8"),
            Self::Json(_) => Some("application/json"),
        }
    }

    /// Render a `Template` body by substituting `{{key}}` placeholders.
    pub fn render(&self, params: &std::collections::HashMap<String, String>) -> ResponseBody {
        match self {
            Self::Template(t) => {
                let mut out = t.clone();
                for (k, v) in params {
                    out = out.replace(&format!("{{{{{}}}}}", k), v);
                }
                Self::Template(out)
            },
            other => other.clone(),
        }
    }
}

/// Guess the best `ResponseBody` from a plain string.
pub fn body_from_str(s: &str) -> ResponseBody {
    let t = s.trim();
    if t.starts_with('{') || t.starts_with('[') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
            return ResponseBody::Json(v);
        }
    }
    if t.contains("{{") && t.contains("}}") {
        return ResponseBody::Template(s.to_string());
    }
    ResponseBody::Text(s.to_string())
}

// ── SelectionStrategy ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SelectionStrategy {
    /// Always return `responses[0]`.
    #[default]
    First,
    /// Cycle through responses in order.
    RoundRobin,
    /// Pseudo-random selection.
    Random,
    /// Select the response whose status code matches the `X-Mock-Status` header.
    MatchStatus,
}

impl SelectionStrategy {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().replace('_', "-").as_str() {
            "round-robin" | "roundrobin" => Self::RoundRobin,
            "random" => Self::Random,
            "match-status" | "matchstatus" => Self::MatchStatus,
            _ => Self::First,
        }
    }
}
