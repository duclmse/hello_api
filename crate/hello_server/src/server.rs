//! Axum HTTP server, catch-all handler, and admin API.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::{Json, Router, routing};
use serde::Serialize;
use tower_http::cors::CorsLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::registry::RouteRegistry;
use crate::render;

// ── Config ────────────────────────────────────────────────────────────────────

pub struct ServerConfig {
    pub bind: SocketAddr,
    pub cors: bool,
    pub timeout_secs: u64,
    pub history_size: usize,
    pub admin: bool,
    pub verbose: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:3000".parse().unwrap(),
            cors: true,
            timeout_secs: 30,
            history_size: 100,
            admin: true,
            verbose: false,
        }
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

pub struct ServerState {
    pub registry: ArcSwap<RouteRegistry>,
    pub history: Mutex<VecDeque<HistoryEntry>>,
    pub config: ServerConfig,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryEntry {
    pub method: String,
    pub path: String,
    pub status: u16,
    pub matched_route: Option<String>,
    pub latency_ms: u64,
}

impl ServerState {
    pub fn new(registry: RouteRegistry, config: ServerConfig) -> Self {
        let cap = config.history_size;
        Self {
            registry: ArcSwap::new(Arc::new(registry)),
            history: Mutex::new(VecDeque::with_capacity(cap)),
            config,
        }
    }

    pub fn push_history(&self, entry: HistoryEntry) {
        let max = self.config.history_size;
        let mut h = self.history.lock().unwrap();
        if h.len() >= max { h.pop_front(); }
        h.push_back(entry);
    }
}

// ── Server entry ──────────────────────────────────────────────────────────────

pub async fn serve(state: Arc<ServerState>) -> anyhow::Result<()> {
    let app = build_app(state.clone());
    let listener = tokio::net::TcpListener::bind(state.config.bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

pub fn build_app(state: Arc<ServerState>) -> Router {
    let cors = state.config.cors;
    let timeout = Duration::from_secs(state.config.timeout_secs);
    let admin = state.config.admin;

    let mut app = Router::new()
        .fallback(handle_mock)
        .with_state(state.clone());

    if admin {
        app = app.merge(admin_router(state));
    }

    app = app.layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::new(timeout));

    if cors {
        app = app.layer(CorsLayer::permissive());
    }

    app
}

fn admin_router(state: Arc<ServerState>) -> Router {
    Router::new()
        .route("/_mock/health",  routing::get(admin_health))
        .route("/_mock/routes",  routing::get(admin_routes))
        .route("/_mock/history", routing::get(admin_history))
        .route("/_mock/history", routing::delete(admin_clear_history))
        .route("/_mock/reload",  routing::post(admin_reload))
        .with_state(state)
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn handle_mock(State(state): State<Arc<ServerState>>, req: Request) -> Response {
    let method = req.method().as_str().to_string();
    let uri = req.uri().clone();
    let path = uri.path();
    let query = uri.query();
    let req_headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .filter_map(|(k, v)| {
            Some((k.as_str().to_string(), v.to_str().ok()?.to_string()))
        })
        .collect();

    let start = Instant::now();
    let registry = state.registry.load();

    match registry.lookup(&method, path, &req_headers, query) {
        Some(m) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            let matched = registry
                .summaries
                .iter()
                .find(|s| s.path == path || matchit_path_matches(&s.path, path))
                .map(|s| s.path.clone());

            let rendered = render::render(&m).await;

            state.push_history(HistoryEntry {
                method: method.clone(),
                path: path.to_string(),
                status: rendered.status,
                matched_route: matched,
                latency_ms,
            });

            if let Some(d) = rendered.delay {
                tokio::time::sleep(d).await;
            }

            let mut builder = axum::http::Response::builder()
                .status(rendered.status);
            for (k, v) in &rendered.headers {
                builder = builder.header(k.as_str(), v.as_str());
            }
            builder
                .body(axum::body::Body::from(rendered.body))
                .unwrap_or_else(|_| {
                    axum::http::Response::builder()
                        .status(500)
                        .body(axum::body::Body::empty())
                        .unwrap()
                })
        },
        None => {
            state.push_history(HistoryEntry {
                method: method.clone(),
                path: path.to_string(),
                status: 404,
                matched_route: None,
                latency_ms: start.elapsed().as_millis() as u64,
            });
            axum::http::Response::builder()
                .status(404)
                .header("Content-Type", "application/json")
                .body(axum::body::Body::from(format!(
                    r#"{{"error":"no mock route matched","method":"{}","path":"{}"}}"#,
                    method, path
                )))
                .unwrap()
        },
    }
}

async fn admin_health(State(state): State<Arc<ServerState>>) -> Json<serde_json::Value> {
    let reg = state.registry.load();
    Json(serde_json::json!({
        "status": "ok",
        "routes": reg.route_count(),
    }))
}

async fn admin_routes(State(state): State<Arc<ServerState>>) -> Json<serde_json::Value> {
    let reg = state.registry.load();
    Json(serde_json::json!({
        "name": reg.collection_name,
        "routes": reg.summaries,
    }))
}

async fn admin_history(State(state): State<Arc<ServerState>>) -> Json<serde_json::Value> {
    let h = state.history.lock().unwrap();
    let entries: Vec<_> = h.iter().cloned().collect();
    Json(serde_json::json!(entries))
}

async fn admin_clear_history(State(state): State<Arc<ServerState>>) -> StatusCode {
    state.history.lock().unwrap().clear();
    StatusCode::NO_CONTENT
}

async fn admin_reload(State(state): State<Arc<ServerState>>) -> Json<serde_json::Value> {
    // The actual reload is driven externally (watcher or CLI re-ingest).
    // This endpoint just reports the current count.
    let reg = state.registry.load();
    Json(serde_json::json!({
        "ok": true,
        "routes": reg.route_count(),
    }))
}

/// Quick heuristic to check if a matchit pattern matches a path.
/// Used only for history annotation — not authoritative.
fn matchit_path_matches(pattern: &str, path: &str) -> bool {
    let pp: Vec<&str> = pattern.split('/').collect();
    let sp: Vec<&str> = path.split('/').collect();
    if pp.len() != sp.len() { return false; }
    pp.iter().zip(sp.iter()).all(|(p, s)| {
        p.starts_with('{') && p.ends_with('}') || p == s
    })
}
