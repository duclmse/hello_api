use std::{collections::HashMap, sync::mpsc, thread, time::Instant};

use hello_client::{HttpTestRunner, TestCase};
use tokio::task::LocalSet;

use crate::event::RunnerEvent;

/// Spawn a background thread that runs `test_cases` one-by-one through
/// [`HttpTestRunner`] and sends [`RunnerEvent`]s back via `tx`.
///
/// The thread owns a single-threaded tokio runtime + `LocalSet` so the V8
/// runtime constraint is satisfied.
pub fn spawn_runner(
    test_cases: Vec<TestCase>,
    params: HashMap<String, String>,
    tx: mpsc::SyncSender<RunnerEvent>,
) {
    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        let started = Instant::now();

        rt.block_on(async move {
            let local = LocalSet::new();
            local
                .run_until(async move {
                    // Derive HTTP allowlist from test URLs.
                    let allowed_prefixes: Vec<String> =
                        test_cases.iter().filter_map(|tc| scheme_host(&tc.request.url)).collect();

                    let mut builder = HttpTestRunner::builder().allowed_prefixes(allowed_prefixes);
                    for (k, v) in &params {
                        builder = builder.env(k.clone(), v.clone());
                    }

                    let mut runner = match builder.build() {
                        Ok(r) => r,
                        Err(e) => {
                            let _ = tx.send(RunnerEvent::Error(e.to_string()));
                            return;
                        },
                    };

                    for (i, tc) in test_cases.into_iter().enumerate() {
                        let _ = tx.send(RunnerEvent::TestStarted(i));
                        match runner.run_test(tc).await {
                            Ok(result) => {
                                let _ = tx.send(RunnerEvent::TestFinished(i, Box::new(result)));
                            },
                            Err(e) => {
                                let _ = tx.send(RunnerEvent::Error(e.to_string()));
                                return;
                            },
                        }
                    }

                    let _ = tx.send(RunnerEvent::Done {
                        elapsed_ms: started.elapsed().as_millis(),
                    });
                })
                .await;
        });
    });
}

/// Extract `"scheme://host[:port]"` from a URL string.
fn scheme_host(url: &str) -> Option<String> {
    let after = url.find("://").map(|i| i + 3)?;
    let rest = &url[after..];
    let end = rest.find('/').map(|i| after + i).unwrap_or(url.len());
    Some(url[..end].to_string())
}
