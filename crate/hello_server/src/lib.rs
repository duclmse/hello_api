//! `hello_server` library — exposes modules for integration tests and the CLI.

pub mod detect;
pub mod ingest;
pub mod loader;
pub mod model;
pub mod registry;
pub mod render;
pub mod server;
pub mod watcher;

pub use model::MockCollection;
pub use registry::RouteRegistry;
pub use server::{ServerConfig, ServerState};
