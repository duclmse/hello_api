//! `hello_server` CLI — loads a spec file and starts a local mock HTTP server.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use hello_server::{ServerConfig, ServerState};
use hello_server::detect::detect_format;
use hello_server::loader::load;
use hello_server::registry::RouteRegistry;
use hello_server::server::serve;
use hello_server::watcher::start_watcher;

#[derive(Parser)]
#[command(name = "hello_server", about = "Local HTTP mock server")]
struct Args {
    /// Spec file or directory to serve
    #[arg(value_name = "SPEC_FILE")]
    spec: PathBuf,

    /// Port to listen on
    #[arg(short, long, default_value = "3000")]
    port: u16,

    /// Host address to bind
    #[arg(short = 'H', long, default_value = "127.0.0.1")]
    host: String,

    /// Force format: http | openapi | postman | bruno
    #[arg(short, long, value_name = "FORMAT")]
    format: Option<String>,

    /// Watch spec file for changes and hot-reload
    #[arg(short, long)]
    watch: bool,

    /// Disable permissive CORS headers
    #[arg(long)]
    no_cors: bool,

    /// Disable admin API at /_mock/*
    #[arg(long)]
    no_admin: bool,

    /// Global response delay in milliseconds
    #[arg(long, default_value = "0")]
    delay: u64,

    /// History ring-buffer size
    #[arg(long, default_value = "100")]
    history: usize,

    /// Verbose: log each matched route
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .init();

    let args = Args::parse();

    let bind: std::net::SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .unwrap_or_else(|e| {
            eprintln!("invalid bind address: {e}");
            std::process::exit(1);
        });

    let fmt = detect_format(&args.spec, args.format.as_deref()).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });

    let col = load(&args.spec, Some(fmt)).unwrap_or_else(|e| {
        eprintln!("error loading spec: {e}");
        std::process::exit(1);
    });

    let route_count = col.routes.len();
    let collection_name = col.name.clone();

    let registry = RouteRegistry::build(col).unwrap_or_else(|e| {
        eprintln!("error building routes: {e}");
        std::process::exit(1);
    });

    let config = ServerConfig {
        bind,
        cors: !args.no_cors,
        timeout_secs: 30,
        history_size: args.history,
        admin: !args.no_admin,
        verbose: args.verbose,
    };

    let state = Arc::new(ServerState::new(registry, config));

    if args.watch {
        start_watcher(args.spec.clone(), state.clone(), fmt);
    }

    eprintln!(
        "hello_server {}\nLoaded {route_count} routes from {} ({fmt})\nListening on http://{bind}",
        env!("CARGO_PKG_VERSION"),
        collection_name.as_str().if_empty(args.spec.to_string_lossy().as_ref()),
    );
    if !args.no_admin {
        eprintln!("Admin:    http://{bind}/_mock/routes");
    }

    if let Err(e) = serve(state).await {
        eprintln!("server error: {e}");
        std::process::exit(1);
    }
}

trait IfEmpty {
    fn if_empty<'a>(&'a self, fallback: &'a str) -> &'a str;
}
impl IfEmpty for str {
    fn if_empty<'a>(&'a self, fallback: &'a str) -> &'a str {
        if self.is_empty() { fallback } else { self }
    }
}
