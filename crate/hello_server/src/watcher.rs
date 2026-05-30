//! File-watch hot reload via `notify`.
//!
//! On any `Create` / `Modify` event on the spec file or directory, the spec is
//! re-ingested and the registry is swapped atomically.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};

use crate::detect::Format;
use crate::registry::RouteRegistry;
use crate::server::ServerState;

/// Start a background thread that watches `spec_path` for changes.
///
/// On each relevant event the spec is re-read, re-ingested, and the shared
/// `ServerState::registry` is swapped.
pub fn start_watcher(spec_path: PathBuf, state: Arc<ServerState>, format: Format) {
    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();

        let mut watcher = match notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        }) {
            Ok(w) => w,
            Err(e) => {
                log::error!("failed to create file watcher: {}", e);
                return;
            },
        };

        let watch_path = if spec_path.is_dir() {
            spec_path.clone()
        } else {
            spec_path.parent().unwrap_or(&spec_path).to_path_buf()
        };

        if let Err(e) = watcher.watch(&watch_path, RecursiveMode::Recursive) {
            log::error!("failed to watch {:?}: {}", watch_path, e);
            return;
        }

        log::info!("watching {:?} for changes", watch_path);

        // Debounce: wait 200 ms after last event before reloading
        loop {
            match rx.recv() {
                Ok(Ok(event)) => {
                    let interesting = matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_)
                    );
                    if !interesting { continue; }

                    // Drain any additional events within 200 ms
                    let _ = rx.recv_timeout(Duration::from_millis(200));

                    match reload(&spec_path, format) {
                        Ok(reg) => {
                            state.registry.store(Arc::new(reg));
                            log::info!(
                                "[reload] {} routes loaded from {:?}",
                                state.registry.load().route_count(),
                                spec_path,
                            );
                        },
                        Err(e) => log::error!("[reload] failed: {}", e),
                    }
                },
                Ok(Err(e)) => log::warn!("watcher error: {}", e),
                Err(_) => break, // channel closed
            }
        }
    });
}

fn reload(spec_path: &PathBuf, format: Format) -> anyhow::Result<RouteRegistry> {
    let col = crate::loader::load(spec_path, Some(format))?;
    RouteRegistry::build(col)
}
