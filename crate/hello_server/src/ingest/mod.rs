//! Ingestion adapters — each converts a spec format into [`MockCollection`].

use crate::model::MockCollection;

pub mod bruno;
pub mod http_file;
pub mod openapi;
pub mod postman;
pub mod sidecar;

/// Common interface for all spec format adapters.
pub trait IngestAdapter {
    fn ingest(&self, source: &str) -> anyhow::Result<MockCollection>;
}
