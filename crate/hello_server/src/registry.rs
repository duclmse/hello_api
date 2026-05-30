//! Route registry — builds a lookup table from a [`MockCollection`] and
//! matches incoming HTTP requests to the correct [`MockRoute`].

use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;

use anyhow::Context;

use crate::model::{
    Matcher, MockCollection, MockResponse, MockRoute, RouteMethod, SelectionStrategy,
};

// ── Public types ──────────────────────────────────────────────────────────────

pub struct RouteRegistry {
    router: matchit::Router<usize>,
    buckets: Vec<RouteBucket>,
    pub collection_name: String,
    pub summaries: Vec<RouteSummary>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RouteSummary {
    pub method: String,
    pub path: String,
    pub response_count: usize,
    pub strategy: String,
}

// ── Internal ──────────────────────────────────────────────────────────────────

struct RouteBucket {
    entries: Vec<RouteEntry>,
}

struct RouteEntry {
    method: RouteMethod,
    matchers: Vec<Matcher>,
    responses: Vec<MockResponse>,
    selection: SelectionStrategy,
    counter: AtomicUsize,
}

// ── Impl ──────────────────────────────────────────────────────────────────────

impl RouteRegistry {
    pub fn build(col: MockCollection) -> anyhow::Result<Self> {
        // Group routes by path pattern
        let mut path_groups: HashMap<String, Vec<MockRoute>> = HashMap::new();
        let mut path_order: Vec<String> = Vec::new();

        for route in col.routes {
            let p = route.path.clone();
            if !path_groups.contains_key(&p) {
                path_order.push(p.clone());
            }
            path_groups.entry(p).or_default().push(route);
        }

        // Sort paths: longer first, then lexicographic (more specific before less)
        path_order.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));

        let mut router: matchit::Router<usize> = matchit::Router::new();
        let mut buckets: Vec<RouteBucket> = Vec::new();
        let mut summaries: Vec<RouteSummary> = Vec::new();

        for path in &path_order {
            let routes = path_groups.remove(path).unwrap();

            // Within a bucket, specific methods before Any
            let mut entries: Vec<RouteEntry> = routes
                .into_iter()
                .map(|r| {
                    summaries.push(RouteSummary {
                        method: method_display(&r.method),
                        path: path.clone(),
                        response_count: r.responses.len(),
                        strategy: format!("{:?}", r.selection).to_lowercase(),
                    });
                    RouteEntry {
                        method: r.method,
                        matchers: r.matchers,
                        responses: r.responses,
                        selection: r.selection,
                        counter: AtomicUsize::new(0),
                    }
                })
                .collect();

            // Specific methods first; Any last
            entries.sort_by_key(|e| matches!(e.method, RouteMethod::Any) as u8);

            let idx = buckets.len();
            buckets.push(RouteBucket { entries });
            router.insert(path, idx).with_context(|| format!("insert route {path}"))?;
        }

        Ok(Self {
            router,
            buckets,
            collection_name: col.name,
            summaries,
        })
    }

    /// Look up a matching route entry.
    ///
    /// Returns the matched [`MockResponse`] (selected by the entry's strategy)
    /// and the extracted path parameters, or `None` if no route matched.
    pub fn lookup(
        &self,
        method: &str,
        path: &str,
        request_headers: &[(String, String)],
        query: Option<&str>,
    ) -> Option<MatchResult<'_>> {
        let Ok(m) = self.router.at(path) else { return None };
        let bucket = &self.buckets[*m.value];

        let path_params: HashMap<String, String> = m
            .params
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        let query_params = parse_query(query.unwrap_or(""));

        for entry in &bucket.entries {
            if !entry.method.matches(method) {
                continue;
            }
            if !eval_matchers(&entry.matchers, request_headers, &query_params) {
                continue;
            }

            let response = select_response(
                &entry.responses,
                &entry.selection,
                &entry.counter,
                request_headers,
            )?;

            return Some(MatchResult { response, path_params, query_params });
        }
        None
    }

    pub fn route_count(&self) -> usize {
        self.summaries.len()
    }
}

pub struct MatchResult<'a> {
    pub response: &'a MockResponse,
    pub path_params: HashMap<String, String>,
    pub query_params: HashMap<String, String>,
}

// ── Selection ─────────────────────────────────────────────────────────────────

fn select_response<'a>(
    responses: &'a [MockResponse],
    strategy: &SelectionStrategy,
    counter: &AtomicUsize,
    headers: &[(String, String)],
) -> Option<&'a MockResponse> {
    if responses.is_empty() {
        return None;
    }
    let n = responses.len();

    match strategy {
        SelectionStrategy::First => Some(&responses[0]),
        SelectionStrategy::RoundRobin => {
            let i = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % n;
            Some(&responses[i])
        },
        SelectionStrategy::Random => {
            let seed = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let i = xorshift(seed) % n;
            Some(&responses[i])
        },
        SelectionStrategy::MatchStatus => {
            let requested: u16 = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("x-mock-status"))
                .and_then(|(_, v)| v.parse().ok())
                .unwrap_or(0);
            if requested != 0 {
                responses.iter().find(|r| r.status == requested).or(Some(&responses[0]))
            } else {
                // Default: first 2xx, then first available
                responses
                    .iter()
                    .find(|r| (200..300).contains(&r.status))
                    .or(Some(&responses[0]))
            }
        },
    }
}

fn xorshift(mut x: usize) -> usize {
    if x == 0 { x = 0xdeadbeef_usize; }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

// ── Matchers ──────────────────────────────────────────────────────────────────

fn eval_matchers(
    matchers: &[Matcher],
    headers: &[(String, String)],
    query_params: &HashMap<String, String>,
) -> bool {
    for matcher in matchers {
        match matcher {
            Matcher::Header { name, value_pattern } => {
                let found = headers
                    .iter()
                    .filter(|(k, _)| k.eq_ignore_ascii_case(name))
                    .any(|(_, v)| value_pattern.matches(v));
                if !found { return false; }
            },
            Matcher::QueryParam { name, value_pattern } => {
                let found = query_params
                    .get(name.as_str())
                    .map(|v| value_pattern.matches(v))
                    .unwrap_or(false);
                if !found { return false; }
            },
        }
    }
    true
}

fn parse_query(query: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for part in query.split('&').filter(|s| !s.is_empty()) {
        if let Some(eq) = part.find('=') {
            map.insert(part[..eq].to_string(), part[eq + 1..].to_string());
        } else {
            map.insert(part.to_string(), String::new());
        }
    }
    map
}

fn method_display(m: &RouteMethod) -> String {
    match m {
        RouteMethod::Specific(s) => s.clone(),
        RouteMethod::Any => "*".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MockCollection, MockResponse, MockRoute, ResponseBody, SelectionStrategy};

    fn make_col(routes: Vec<MockRoute>) -> MockCollection {
        MockCollection { name: "test".to_string(), routes }
    }

    fn route(method: &str, path: &str, status: u16) -> MockRoute {
        MockRoute {
            id: format!("{method} {path}"),
            method: RouteMethod::from_str(method),
            path: path.to_string(),
            matchers: vec![],
            responses: vec![MockResponse { status, headers: vec![], body: ResponseBody::Empty, delay_ms: 0 }],
            selection: SelectionStrategy::First,
        }
    }

    #[test]
    fn basic_match() {
        let col = make_col(vec![route("GET", "/users", 200)]);
        let reg = RouteRegistry::build(col).unwrap();
        let m = reg.lookup("GET", "/users", &[], None).unwrap();
        assert_eq!(m.response.status, 200);
    }

    #[test]
    fn path_param_extracted() {
        let col = make_col(vec![route("GET", "/users/{id}", 200)]);
        let reg = RouteRegistry::build(col).unwrap();
        let m = reg.lookup("GET", "/users/42", &[], None).unwrap();
        assert_eq!(m.path_params.get("id"), Some(&"42".to_string()));
    }

    #[test]
    fn no_match_returns_none() {
        let col = make_col(vec![route("GET", "/users", 200)]);
        let reg = RouteRegistry::build(col).unwrap();
        assert!(reg.lookup("GET", "/posts", &[], None).is_none());
    }

    #[test]
    fn method_wildcard() {
        let col = make_col(vec![route("*", "/health", 200)]);
        let reg = RouteRegistry::build(col).unwrap();
        assert!(reg.lookup("GET",    "/health", &[], None).is_some());
        assert!(reg.lookup("DELETE", "/health", &[], None).is_some());
    }

    #[test]
    fn round_robin_cycles() {
        let mut r = route("GET", "/flip", 200);
        r.responses.push(MockResponse { status: 503, headers: vec![], body: ResponseBody::Empty, delay_ms: 0 });
        r.selection = SelectionStrategy::RoundRobin;
        let reg = RouteRegistry::build(make_col(vec![r])).unwrap();
        let s1 = reg.lookup("GET", "/flip", &[], None).unwrap().response.status;
        let s2 = reg.lookup("GET", "/flip", &[], None).unwrap().response.status;
        assert_ne!(s1, s2);
    }
}
