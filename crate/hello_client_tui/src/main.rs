use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

use clap::Parser;
use crossterm::{
    event::{Event, KeyCode, KeyEvent, KeyModifiers, poll, read},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use hello_client::{
    BrunoAdapter, FlowDef, FlowNode, OpenApiAdapter, OpenCollectionAdapter, PostmanAdapter,
    StepDef, TestCase, parse_flow, runner::parse_collection,
};
use ratatui::{Terminal, backend::CrosstermBackend};

mod app;
mod debug;
mod event;
mod runner;
mod ui;

use app::{App, BrowserEntry, FolderSpec, ParamEditor};
use event::RunnerEvent;

/// `(display_name, PathBuf)` pairs for each collection source to load.
type Sources = Vec<(String, PathBuf)>;

// ─── CLI ──────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "hello_tui", about = "Interactive HTTP test runner TUI")]
struct Cli {
    /// .http file, collection file, or directory to load (default: requests.http)
    path: Option<PathBuf>,

    /// Variable substitution, e.g. key=value (repeatable)
    #[arg(short = 'p', long = "param", value_name = "KEY=VALUE")]
    params: Vec<String>,

    /// Config file path (TOML/JSON/INI)
    #[arg(short = 'c', long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Active environment name for Bruno collections (selects environments/<name>.bru)
    #[arg(long, value_name = "NAME")]
    env: Option<String>,

    /// Mirror debug log to file
    #[arg(long, value_name = "FILE")]
    debug_log: Option<PathBuf>,
}

fn main() {
    let cli = Cli::parse();

    let mut params: HashMap<String, String> = HashMap::new();
    for p in &cli.params {
        if let Some((k, v)) = p.split_once('=') {
            params.insert(k.to_string(), v.to_string());
        } else {
            eprintln!("error: invalid param '{}' — use KEY=VALUE", p);
            std::process::exit(1);
        }
    }

    let path = cli.path.unwrap_or_else(|| PathBuf::from("requests.http"));

    let mut sources = load_sources(&path);

    let title = if path.is_dir() {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string())
    } else {
        path.file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string())
    };

    // Auto-load *.env / *.env.json from the collection root (lower priority than --param).
    for (k, v) in load_env_files(&collection_root(&path)) {
        params.entry(k).or_insert(v);
    }

    // Try to load the initial collection; on failure start with an empty TUI.
    let (folder_specs, initial_cases) = if !sources.is_empty() {
        match load_collection(&sources, &params, cli.env.as_deref()) {
            Ok((specs, cases, col_env)) => {
                // Merge collection-embedded vars at lowest priority.
                for (k, v) in col_env {
                    params.entry(k).or_insert(v);
                }
                (specs, cases)
            },
            Err(e) => {
                eprintln!("warning: {}", e);
                sources.clear();
                (vec![], vec![])
            },
        }
    } else {
        (vec![], vec![])
    };

    let debug = match debug::DebugLog::new(cli.debug_log.as_deref()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot open debug log: {}", e);
            std::process::exit(1);
        },
    };

    let mut app = App::new(title, folder_specs, initial_cases, params, debug);
    app.env_name = cli.env.clone();

    if let Err(e) = run_tui(&mut app, None, &mut sources) {
        eprintln!("tui error: {}", e);
        std::process::exit(1);
    }

    let (passed, failed) = app.summary();
    println!("{} passed  {} failed", passed, failed);
    if failed > 0 {
        std::process::exit(1);
    }
}

// ─── Format detection ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum CollectionFormat {
    Http = 0,
    Flow = 1,
    Postman = 2,
    Bruno = 3,
    OpenCollection = 4,
    OpenApi = 5,
}

fn detect_format(path: &Path) -> Option<CollectionFormat> {
    if path.is_dir() {
        return Some(CollectionFormat::Bruno);
    }
    match path.extension().and_then(|e| e.to_str()) {
        Some("http") | Some("rest") => Some(CollectionFormat::Http),
        Some("flow") => Some(CollectionFormat::Flow),
        Some("bru") => Some(CollectionFormat::Bruno),
        Some("json") => {
            let content = std::fs::read_to_string(path).ok()?;
            if content.chars().take(512).collect::<String>().contains("\"opencollection\"") {
                Some(CollectionFormat::OpenCollection)
            } else {
                Some(CollectionFormat::Postman)
            }
        },
        Some("yaml") | Some("yml") => {
            let content = std::fs::read_to_string(path).ok()?;
            let is_openapi = content
                .lines()
                .take(20)
                .any(|l| l.trim().starts_with("openapi:") || l.trim().starts_with("swagger:"));
            if is_openapi {
                Some(CollectionFormat::OpenApi)
            } else {
                Some(CollectionFormat::OpenCollection)
            }
        },
        _ => None,
    }
}

fn is_supported_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("http")
            | Some("rest")
            | Some("flow")
            | Some("bru")
            | Some("json")
            | Some("yaml")
            | Some("yml")
    )
}

// ─── Collection loading ───────────────────────────────────────────────────────

/// Build the source list from a path, scanning directories recursively.
///
/// - A directory containing `.bru` files is treated as one Bruno collection.
/// - Other directories are walked recursively; each supported file becomes a source.
/// - Display names are paths relative to the root (without extension) for disambiguation.
fn load_sources(path: &Path) -> Sources {
    if path.is_dir() {
        let mut paths: Vec<PathBuf> = Vec::new();
        collect_sources_recursive(path, &mut paths);
        paths
            .into_iter()
            .map(|p| (source_display_name(path, &p), p))
            .collect()
    } else {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        vec![(name, path.to_path_buf())]
    }
}

/// Recursively collect supported source paths under `dir`.
/// Directories containing `.bru` files are added as-is (Bruno collection) and not
/// descended into. Hidden entries (names starting with `.`) are skipped.
fn collect_sources_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    entries.sort();

    let has_bru =
        entries.iter().any(|p| p.is_file() && p.extension().is_some_and(|x| x == "bru"));
    if has_bru {
        // The whole directory is a single Bruno collection source.
        out.push(dir.to_path_buf());
        return;
    }

    for path in entries {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_sources_recursive(&path, out);
        } else if is_supported_file(&path) {
            out.push(path);
        }
    }
}

/// Human-readable display name for a source path relative to the scan root.
/// Files: strip the root prefix and extension (e.g. `"auth/login"`).
/// Directories (Bruno): strip the root prefix only (e.g. `"bruno-suite"`).
fn source_display_name(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    if path.is_dir() {
        rel.display().to_string()
    } else {
        rel.with_extension("").display().to_string()
    }
}

#[allow(clippy::type_complexity)]
/// Load a source and return `(folder_display_name, test_cases, collection_env_vars)`.
/// Collection-level env vars (lower priority than `--param`) are returned in the third element.
fn load_source(
    name: &str,
    path: &Path,
    params: &HashMap<String, String>,
    env_name: Option<&str>,
) -> Result<(String, Vec<TestCase>, HashMap<String, String>), String> {
    let format =
        detect_format(path).ok_or_else(|| format!("unsupported file type: {}", path.display()))?;

    match format {
        CollectionFormat::Flow => {
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
            let flow =
                parse_flow(&content).map_err(|e| format!("{}: {}", path.display(), e))?;
            let base_dir = path.parent().unwrap_or(Path::new("."));
            let cases = flow_steps_to_cases(&flow, base_dir, params)?;
            Ok((flow.name, cases, flow.env.clone()))
        },
        CollectionFormat::Http => {
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
            let base_dir = path.parent().unwrap_or(Path::new("."));
            let cases = parse_collection(&content, params, base_dir)?;
            Ok((name.to_string(), cases, HashMap::new()))
        },
        CollectionFormat::Postman => {
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
            let col = PostmanAdapter::import(&content)
                .map_err(|e| format!("Postman import error: {:?}", e))?;
            Ok((name.to_string(), col.tests, col.variables))
        },
        CollectionFormat::Bruno => {
            let cases = if path.is_dir() {
                match env_name {
                    Some(env) => BrunoAdapter::import_dir_with_env(path, env)
                        .map_err(|e| format!("Bruno import error: {:?}", e))?,
                    None => BrunoAdapter::import_dir(path)
                        .map_err(|e| format!("Bruno import error: {:?}", e))?,
                }
            } else {
                let content = std::fs::read_to_string(path)
                    .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
                let tc = BrunoAdapter::import(&content)
                    .map_err(|e| format!("Bruno import error: {:?}", e))?;
                vec![tc]
            };
            Ok((name.to_string(), cases, HashMap::new()))
        },
        CollectionFormat::OpenCollection => {
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
            let col = OpenCollectionAdapter::import(&content)
                .map_err(|e| format!("OpenCollection import error: {}", e))?;
            Ok((name.to_string(), col.tests, col.env))
        },
        CollectionFormat::OpenApi => {
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
            let col = OpenApiAdapter::import(&content)
                .map_err(|e| format!("OpenAPI import error: {}", e))?;
            Ok((name.to_string(), col.tests, HashMap::new()))
        },
    }
}

/// Flatten all steps in a flow (including inner parallel steps) into `TestCase`s in
/// document order. Parallel group steps are expanded in inner-step order.
fn flow_steps_to_cases(
    flow: &FlowDef,
    base_dir: &Path,
    params: &HashMap<String, String>,
) -> Result<Vec<TestCase>, String> {
    let mut cases = Vec::new();
    for node in &flow.nodes {
        match node {
            FlowNode::Step(step) => cases.push(step_to_case(step, base_dir, params)?),
            FlowNode::Parallel(group) => {
                for step in &group.steps {
                    cases.push(step_to_case(step, base_dir, params)?);
                }
            },
        }
    }
    Ok(cases)
}

/// Load one flow step's file and return the matching `TestCase`.
/// Name is prefixed with the step ID: `"step_id: original_name"`.
fn step_to_case(
    step: &StepDef,
    base_dir: &Path,
    params: &HashMap<String, String>,
) -> Result<TestCase, String> {
    let file_path = base_dir.join(&step.file);
    let (_, cases, _) = load_source(&step.id, &file_path, params, None)?;
    let mut tc = match &step.entry {
        Some(entry_name) => cases
            .into_iter()
            .find(|c| c.name == *entry_name)
            .ok_or_else(|| {
                format!(
                    "step '{}': entry '{}' not found in {}",
                    step.id, entry_name, step.file
                )
            })?,
        None => cases
            .into_iter()
            .next()
            .ok_or_else(|| format!("step '{}': no entries found in {}", step.id, step.file))?,
    };
    tc.name = format!("{}: {}", step.id, tc.name);
    Ok(tc)
}

// ─── Env file loading ─────────────────────────────────────────────────────────

/// Scan `dir` for `*.env` and `*.env.json` files and return a merged map of
/// template variables. Files are processed in alphabetical order; for duplicate
/// keys the first file (alphabetically) wins.
///
/// Format:
/// - `*.env`      — `KEY = VALUE` lines (or `param.KEY = VALUE`), `#`/`//` comments
/// - `*.env.json` — `{ "param": { "KEY": "VALUE" }, "base_url": "…" }`
fn load_env_files(dir: &Path) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = HashMap::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return env;
    };
    let mut paths: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".env") || n.ends_with(".env.json"))
        })
        .collect();
    paths.sort();
    for path in &paths {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        if content.trim_start().starts_with('{') {
            parse_env_json_into(&content, &mut env);
        } else {
            parse_env_dotenv_into(&content, &mut env);
        }
    }
    env
}

/// Return the directory to scan for env files given the collection `path`
/// (a file → its parent; a directory → itself).
fn collection_root(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
    }
}

fn parse_env_json_into(content: &str, env: &mut HashMap<String, String>) {
    let Ok(root) = serde_json::from_str::<serde_json::Value>(content) else {
        return;
    };
    let Some(obj) = root.as_object() else {
        return;
    };
    // `base_url` top-level key → template variable
    if let Some(v) = obj.get("base_url").and_then(|v| v.as_str()) {
        env.entry("base_url".to_string()).or_insert_with(|| v.to_string());
    }
    // `param` sub-object → template variables
    if let Some(params) = obj.get("param").and_then(|v| v.as_object()) {
        for (k, v) in params {
            let val = v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string());
            env.entry(k.clone()).or_insert(val);
        }
    }
}

fn parse_env_dotenv_into(content: &str, env: &mut HashMap<String, String>) {
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if key == "base_url" {
                env.entry("base_url".to_string()).or_insert_with(|| value.to_string());
            } else if let Some(var_key) = key.strip_prefix("param.") {
                env.entry(var_key.to_string()).or_insert_with(|| value.to_string());
            }
        }
    }
}

#[allow(clippy::type_complexity)]
/// Parse every source and return `(folder_specs, flat_test_cases, collection_env_vars)`.
/// Collection-level env vars are merged from all sources (first occurrence wins).
fn load_collection(
    sources: &Sources,
    params: &HashMap<String, String>,
    env_name: Option<&str>,
) -> Result<(Vec<FolderSpec>, Vec<TestCase>, HashMap<String, String>), String> {
    let mut folder_specs: Vec<FolderSpec> = Vec::new();
    let mut all_cases: Vec<TestCase> = Vec::new();
    let mut col_env: HashMap<String, String> = HashMap::new();

    for (name, path) in sources {
        let (folder_name, cases, src_env) = load_source(name, path, params, env_name)?;
        // Merge env vars; first occurrence wins across sources.
        for (k, v) in src_env {
            col_env.entry(k).or_insert(v);
        }
        if !cases.is_empty() {
            let names: Vec<String> = cases.iter().map(|tc| tc.name.clone()).collect();
            folder_specs.push((folder_name, names));
            all_cases.extend(cases);
        }
    }

    Ok((folder_specs, all_cases, col_env))
}

/// Re-parse with current `app.params`, reset all test statuses, spawn a full runner.
fn rerun(app: &mut App, sources: &Sources) -> Result<mpsc::Receiver<RunnerEvent>, String> {
    let (folder_specs, cases, _) =
        load_collection(sources, &app.params, app.env_name.as_deref())?;
    if cases.is_empty() {
        return Err("no test cases found".to_string());
    }
    let run_cases = cases.clone();
    app.apply_rerun(folder_specs, cases);
    let (tx, rx) = mpsc::sync_channel::<RunnerEvent>(64);
    runner::spawn_runner(run_cases, app.params.clone(), tx);
    Ok(rx)
}

/// Run the single test at `test_idx` from `app.cases`.
fn run_selected(app: &mut App, test_idx: usize) -> Result<mpsc::Receiver<RunnerEvent>, String> {
    let case = app
        .cases
        .get(test_idx)
        .ok_or_else(|| format!("test index {} out of range", test_idx))?
        .clone();
    // Reset only this test's status.
    if let Some(row) = app.tests.get_mut(test_idx) {
        row.status = crate::app::TestStatus::Pending;
        row.result = None;
    }
    app.phase = crate::app::Phase::Running { current: test_idx };
    let (tx, rx) = mpsc::sync_channel::<RunnerEvent>(64);
    runner::spawn_single_runner(case, test_idx, app.params.clone(), tx);
    Ok(rx)
}

/// Load a new collection from `path`, update `sources`, reset the app, spawn a fresh runner.
fn open_collection(
    app: &mut App,
    sources: &mut Sources,
    path: &Path,
    rx: &mut Option<mpsc::Receiver<RunnerEvent>>,
) -> Result<(), String> {
    let new_sources = load_sources(path);
    if new_sources.is_empty() {
        return Err(format!("no collection files found in {}", path.display()));
    }
    // Auto-load env files from the new collection root (below --param, above embedded vars).
    for (k, v) in load_env_files(&collection_root(path)) {
        app.params.entry(k).or_insert(v);
    }
    let (folder_specs, test_cases, col_env) =
        load_collection(&new_sources, &app.params, app.env_name.as_deref())?;
    for (k, v) in col_env {
        app.params.entry(k).or_insert(v);
    }
    app.title = if path.is_dir() {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string())
    } else {
        path.file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string())
    };
    *sources = new_sources;
    app.apply_collection(folder_specs, test_cases);
    *rx = None; // no auto-run; user triggers runs manually
    Ok(())
}

// ─── Conversion ───────────────────────────────────────────────────────────────

fn serialize_to_http(cases: &[TestCase]) -> String {
    let mut out = String::new();
    for (i, tc) in cases.iter().enumerate() {
        if i > 0 {
            out.push_str("\n\n");
        }
        out.push_str(&format!("### {}\n\n", tc.name));
        out.push_str(&format!("{} {}\n", tc.request.method, tc.request.url));
        for (k, v) in &tc.request.headers {
            out.push_str(&format!("{}: {}\n", k, v));
        }
        if let Some(body) = &tc.request.body {
            out.push('\n');
            out.push_str(body);
            out.push('\n');
        }
        if let Some(pre) = &tc.pre_script {
            out.push_str("\n< {%\n");
            out.push_str(pre);
            out.push_str("\n%}\n");
        }
        if let Some(post) = &tc.post_script {
            out.push_str("\n> {%\n");
            out.push_str(post);
            out.push_str("\n%}\n");
        }
    }
    out
}

fn export_to_string(
    format_id: &str,
    name: &str,
    cases: &[TestCase],
    env: &HashMap<String, String>,
) -> Result<String, String> {
    match format_id {
        "http" => Ok(serialize_to_http(cases)),
        "postman" => Ok(PostmanAdapter::export(name, cases, env)),
        "opencollection" => Ok(OpenCollectionAdapter::export(name, cases, env)),
        "openapi" => Ok(OpenApiAdapter::export(name, cases)),
        "curl" => {
            use hello_client::CurlAdapter;
            let parts: Vec<String> = cases.iter().map(CurlAdapter::export).collect();
            Ok(parts.join("\n\n"))
        },
        other => Err(format!("unknown export format: {}", other)),
    }
}

fn run_convert(app: &mut App, sources: &Sources) {
    let format_id =
        app::CONVERT_FORMATS.get(app.convert.format_cursor).map(|(id, _, _)| *id).unwrap_or("http");

    let out_path = app.convert.output_path.trim().to_string();
    if out_path.is_empty() {
        app.convert.message = Some("output path cannot be empty".to_string());
        return;
    }

    let cases = match load_collection(sources, &app.params, app.env_name.as_deref()) {
        Ok((_, cases, _)) => cases,
        Err(e) => {
            app.convert.message = Some(format!("load error: {}", e));
            return;
        },
    };

    let content = match export_to_string(format_id, &app.title, &cases, &app.params) {
        Ok(s) => s,
        Err(e) => {
            app.convert.message = Some(e);
            return;
        },
    };

    match std::fs::write(&out_path, &content) {
        Ok(()) => {
            app.convert.message = Some(format!("saved to {}", out_path));
            app.convert.editing = false;
        },
        Err(e) => {
            app.convert.message = Some(format!("write error: {}", e));
        },
    }
}

// ─── TUI event loop ───────────────────────────────────────────────────────────

fn run_tui(
    app: &mut App,
    initial_rx: Option<mpsc::Receiver<RunnerEvent>>,
    sources: &mut Sources,
) -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let tick = Duration::from_millis(80);
    let mut last_tick = Instant::now();
    let mut rx: Option<mpsc::Receiver<RunnerEvent>> = initial_rx;

    loop {
        terminal.draw(|f| ui::render(f, app))?;

        if let Some(r) = &rx {
            while let Ok(ev) = r.try_recv() {
                match ev {
                    RunnerEvent::TestStarted(i) => app.on_test_started(i),
                    RunnerEvent::TestFinished(i, r) => app.on_test_finished(i, *r),
                    RunnerEvent::Done { elapsed_ms } => app.on_done(elapsed_ms),
                    RunnerEvent::DoneSingle => app.on_done_single(),
                    RunnerEvent::Error(msg) => app.on_error(msg),
                }
            }
        }

        let timeout = tick.saturating_sub(last_tick.elapsed());
        if poll(timeout)?
            && let Event::Key(key) = read()?
        {
            if app.show_browser {
                handle_browser_key(app, key, sources, &mut rx);
            } else if app.show_convert {
                handle_convert_key(app, key, sources);
            } else if app.show_request_editor {
                handle_request_editor_key(app, key);
            } else if app.show_params {
                handle_param_editor_key(app, key, sources, &mut rx);
            } else {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Up | KeyCode::Char('k') => app.select_up(),
                    KeyCode::Down | KeyCode::Char('j') => app.select_down(),
                    KeyCode::Tab => app.toggle_detail_tab(),
                    KeyCode::Enter => {
                        use app::TreeItem;
                        match app.tree.get(app.cursor).copied() {
                            Some(TreeItem::Folder(_)) => app.toggle_folder(),
                            Some(TreeItem::Test(ti)) => match run_selected(app, ti) {
                                Ok(new_rx) => rx = Some(new_rx),
                                Err(e) => app.on_error(e),
                            },
                            None => {},
                        }
                    },
                    KeyCode::Char('v') => {
                        use app::TreeItem;
                        if let Some(TreeItem::Test(ti)) = app.tree.get(app.cursor).copied() {
                            app.open_request_editor(ti);
                        }
                    },
                    KeyCode::Char('u') => app.scroll_detail(-3),
                    KeyCode::Char('d') => app.scroll_detail(3),
                    KeyCode::Char('l') => app.show_logs = !app.show_logs,
                    KeyCode::Char('e') => {
                        app.param_editor = ParamEditor::from_params(&app.params);
                        app.show_params = true;
                    },
                    KeyCode::Char('o') => {
                        let start = sources
                            .first()
                            .and_then(|(_, p)| p.parent())
                            .map(|p| p.to_path_buf())
                            .or_else(|| std::env::current_dir().ok())
                            .unwrap_or_else(|| PathBuf::from("."));
                        app.open_browser(start);
                        update_browser_preview(app);
                    },
                    KeyCode::Char('C') if !sources.is_empty() => {
                        let suggested = suggest_output_path(sources, app.convert.format_cursor);
                        app.open_convert(suggested);
                    },
                    KeyCode::Char('r') if !sources.is_empty() => match rerun(app, sources) {
                        Ok(new_rx) => rx = Some(new_rx),
                        Err(e) => app.on_error(e),
                    },
                    KeyCode::Char('?') => {
                        app.show_debug = !app.show_debug;
                        app.debug_scroll = 0;
                    },
                    KeyCode::Char('[') if app.show_debug => app.scroll_debug(-3),
                    KeyCode::Char(']') if app.show_debug => app.scroll_debug(3),
                    _ => {},
                }
            }
        }

        if last_tick.elapsed() >= tick {
            app.tick();
            last_tick = Instant::now();
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn suggest_output_path(sources: &Sources, format_cursor: usize) -> String {
    let ext = app::CONVERT_FORMATS.get(format_cursor).map(|(_, ext, _)| *ext).unwrap_or(".http");
    let stem = sources
        .first()
        .and_then(|(_, p)| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string());
    format!("{}{}", stem, ext)
}

fn handle_param_editor_key(
    app: &mut App,
    key: KeyEvent,
    sources: &Sources,
    rx: &mut Option<mpsc::Receiver<RunnerEvent>>,
) {
    let editing = app.param_editor.is_editing();
    match key.code {
        KeyCode::Esc => {
            if editing {
                app.param_editor.cancel_edit();
            } else {
                app.show_params = false;
            }
        },
        KeyCode::Enter => {
            if editing {
                app.param_editor.confirm_edit();
            } else {
                app.param_editor.start_edit_value();
            }
        },
        KeyCode::Down | KeyCode::Char('j') if !editing => app.param_editor.nav_down(),
        KeyCode::Up | KeyCode::Char('k') if !editing => app.param_editor.nav_up(),
        KeyCode::Tab if !editing => app.param_editor.start_edit_key(),
        KeyCode::Char('e') if !editing => app.param_editor.start_edit_value(),
        KeyCode::Char('a') if !editing => app.param_editor.add_row(),
        KeyCode::Char('d') if !editing => app.param_editor.delete_row(),
        KeyCode::Char('r') if !editing => {
            app.params = app.param_editor.to_params();
            app.show_params = false;
            match rerun(app, sources) {
                Ok(new_rx) => *rx = Some(new_rx),
                Err(e) => app.on_error(e),
            }
        },
        KeyCode::Backspace if editing => app.param_editor.input_backspace(),
        KeyCode::Char(c) if editing => app.param_editor.input_char(c),
        _ => {},
    }
}

/// Read up to 200 lines of the currently highlighted collection file into
/// `app.browser_preview_lines` so the render side can display them without I/O.
fn update_browser_preview(app: &mut App) {
    app.browser_preview_lines.clear();
    if let Some(BrowserEntry::CollectionFile(_, path, _)) = app.browser.selected() {
        let path = path.clone();
        if let Ok(content) = std::fs::read_to_string(&path) {
            app.browser_preview_lines =
                content.lines().take(200).map(|l| l.to_string()).collect();
        }
    }
}

fn handle_browser_key(
    app: &mut App,
    key: KeyEvent,
    sources: &mut Sources,
    rx: &mut Option<mpsc::Receiver<RunnerEvent>>,
) {
    match key.code {
        KeyCode::Esc => app.show_browser = false,
        KeyCode::Up | KeyCode::Char('k') => {
            app.browser.nav_up();
            update_browser_preview(app);
        },
        KeyCode::Down | KeyCode::Char('j') => {
            app.browser.nav_down();
            update_browser_preview(app);
        },
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => {
            app.browser.go_parent();
            update_browser_preview(app);
        },
        KeyCode::Char(' ') => {
            let path = app.browser.cwd.clone();
            match open_collection(app, sources, &path, rx) {
                Ok(()) => app.show_browser = false,
                Err(e) => app.on_error(e),
            }
        },
        KeyCode::Enter => {
            let entry = app.browser.selected().cloned();
            match entry {
                Some(BrowserEntry::ParentDir) => {
                    app.browser.go_parent();
                    update_browser_preview(app);
                },
                Some(BrowserEntry::Dir(_, path)) => {
                    app.browser.enter_dir(path);
                    update_browser_preview(app);
                },
                Some(BrowserEntry::CollectionFile(_, path, _)) => {
                    match open_collection(app, sources, &path, rx) {
                        Ok(()) => app.show_browser = false,
                        Err(e) => app.on_error(e),
                    }
                },
                None => {},
            }
        },
        _ => {},
    }
}

fn handle_request_editor_key(app: &mut App, key: KeyEvent) {
    let editing = app.request_editor.editing;
    match key.code {
        KeyCode::Esc => {
            if editing {
                app.request_editor.cancel_edit();
            } else {
                app.show_request_editor = false;
            }
        },
        KeyCode::Enter => {
            if editing {
                app.request_editor.confirm_edit();
            } else {
                app.request_editor.start_edit();
            }
        },
        KeyCode::Char('e') if !editing => {
            app.request_editor.start_edit();
        },
        KeyCode::Tab if !editing => {
            // Tab on a header or form-field row toggles the key/value column.
            let re = &mut app.request_editor;
            let on_header = re.cursor >= 2 && re.cursor < re.body_row();
            let on_form = re.form_fields.is_some() && re.cursor >= re.body_row();
            if on_header || on_form {
                re.edit_key = !re.edit_key;
            } else {
                re.nav_down();
            }
        },
        KeyCode::Down | KeyCode::Char('j') if !editing => app.request_editor.nav_down(),
        KeyCode::Up | KeyCode::Char('k') if !editing => app.request_editor.nav_up(),
        KeyCode::Char('a') if !editing => {
            let re = &mut app.request_editor;
            if re.form_fields.is_some() && re.cursor >= re.body_row() {
                re.add_form_field();
            } else {
                re.add_header();
            }
        },
        KeyCode::Char('d') if !editing => {
            let re = &mut app.request_editor;
            if re.form_fields.is_some() && re.cursor >= re.body_row() {
                re.delete_form_field();
            } else {
                re.delete_header();
            }
        },
        KeyCode::Char('s') if !editing => app.save_request_editor(),
        KeyCode::Backspace if editing => app.request_editor.input_backspace(),
        KeyCode::Char(c) if editing => app.request_editor.input_char(c),
        _ => {},
    }
}

fn handle_convert_key(app: &mut App, key: KeyEvent, sources: &Sources) {
    let editing = app.convert.editing;
    match key.code {
        KeyCode::Esc => {
            if editing {
                app.convert.editing = false;
            } else {
                app.show_convert = false;
            }
        },
        KeyCode::Enter => {
            if editing {
                app.convert.editing = false;
            } else {
                run_convert(app, sources);
            }
        },
        KeyCode::Down | KeyCode::Char('j') if !editing => {
            let max = app::CONVERT_FORMATS.len().saturating_sub(1);
            if app.convert.format_cursor < max {
                app.convert.format_cursor += 1;
                app.convert.output_path = suggest_output_path(sources, app.convert.format_cursor);
                app.convert.message = None;
            }
        },
        KeyCode::Up | KeyCode::Char('k') if !editing => {
            if app.convert.format_cursor > 0 {
                app.convert.format_cursor -= 1;
                app.convert.output_path = suggest_output_path(sources, app.convert.format_cursor);
                app.convert.message = None;
            }
        },
        KeyCode::Tab | KeyCode::Char('e') if !editing => {
            app.convert.editing = true;
            app.convert.message = None;
        },
        KeyCode::Backspace if editing => {
            app.convert.output_path.pop();
        },
        KeyCode::Char(c) if editing => {
            app.convert.output_path.push(c);
        },
        _ => {},
    }
}
