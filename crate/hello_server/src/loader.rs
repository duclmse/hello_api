//! Loads a spec file from disk, auto-detects the format, and returns a
//! [`MockCollection`] ready for routing.

use std::path::Path;

use crate::detect::{Format, detect_format};
use crate::ingest::{IngestAdapter, bruno::BrunoAdapter, http_file::HttpFileAdapter,
    openapi::OpenApiAdapter, postman::PostmanAdapter, sidecar};
use crate::model::MockCollection;

/// Load a spec file (or directory) into a [`MockCollection`].
///
/// If `format` is `None`, the format is detected automatically.
pub fn load(path: &Path, format: Option<Format>) -> anyhow::Result<MockCollection> {
    let fmt = match format {
        Some(f) => f,
        None => detect_format(path, None)?,
    };

    let mut col = if fmt == Format::Bruno && path.is_dir() {
        BrunoAdapter::ingest_dir(path)?
    } else {
        let src = std::fs::read_to_string(path)?;
        match fmt {
            Format::HttpFile => HttpFileAdapter.ingest(&src)?,
            Format::OpenApi  => OpenApiAdapter.ingest(&src)?,
            Format::Postman  => PostmanAdapter.ingest(&src)?,
            Format::Bruno    => BrunoAdapter.ingest(&src)?,
        }
    };

    // Apply sidecar overrides if present
    if let Ok(Some(sf)) = sidecar::load_sidecar(path) {
        sidecar::apply_sidecar(&mut col, sf);
    }

    if col.routes.is_empty() {
        anyhow::bail!("no routes loaded from {:?}", path);
    }

    Ok(col)
}
