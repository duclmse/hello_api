use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

use crossterm::{
    event::{Event, KeyCode, KeyEvent, KeyModifiers, poll, read},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

mod app;
mod debug;
mod event;
mod runner;
mod ui;

use app::{App, BrowserEntry, FolderSpec, ParamEditor};
use event::RunnerEvent;

/// `(display_name, PathBuf)` pairs for each `.http` file to load.
type Sources = Vec<(String, PathBuf)>;

fn main() {
    let (path_str, params, debug_log_path) = parse_args();
    let path = PathBuf::from(&path_str);

    let mut sources = load_sources(&path);
    if sources.is_empty() {
        eprintln!("error: no .http files found in {}", path_str);
        std::process::exit(1);
    }

    let title = if path.is_dir() {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path_str.clone())
    } else {
        path_str.clone()
    };

    let (folder_specs, test_cases) = match load_collection(&sources, &params) {
        Ok(r) if !r.1.is_empty() => r,
        Ok(_) => {
            eprintln!("error: no test cases found");
            std::process::exit(0);
        },
        Err(e) => {
            eprintln!("parse error: {}", e);
            std::process::exit(1);
        },
    };

    let (tx, rx) = mpsc::sync_channel::<RunnerEvent>(64);
    runner::spawn_runner(test_cases, params.clone(), tx);

    let debug = match debug::DebugLog::new(debug_log_path.as_deref().map(Path::new)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot open debug log: {}", e);
            std::process::exit(1);
        },
    };

    let mut app = App::new(title, folder_specs, params, debug);

    if let Err(e) = run_tui(&mut app, rx, &mut sources) {
        eprintln!("tui error: {}", e);
        std::process::exit(1);
    }

    // After the TUI exits print a plain-text summary.
    let (passed, failed) = app.summary();
    println!("{} passed  {} failed", passed, failed);
    if failed > 0 {
        std::process::exit(1);
    }
}

// ─── Collection loading ───────────────────────────────────────────────────────

/// Build the source list from a file or directory path.
/// For directories, returns all `*.http` files sorted alphabetically.
fn load_sources(path: &Path) -> Sources {
    if path.is_dir() {
        let mut paths: Vec<PathBuf> = match std::fs::read_dir(path) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|ext| ext == "http"))
                .collect(),
            Err(_) => return Vec::new(),
        };
        paths.sort();
        paths
            .into_iter()
            .map(|p| {
                let name = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.display().to_string());
                (name, p)
            })
            .collect()
    } else {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        vec![(name, path.to_path_buf())]
    }
}

/// Parse each source file and return `(folder_specs, flat_test_cases)`.
fn load_collection(
    sources: &Sources,
    params: &HashMap<String, String>,
) -> Result<(Vec<FolderSpec>, Vec<hello_client::TestCase>), String> {
    let mut folder_specs: Vec<FolderSpec> = Vec::new();
    let mut all_cases: Vec<hello_client::TestCase> = Vec::new();

    for (name, path) in sources {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
        let base_dir = path.parent().unwrap_or(Path::new("."));
        let cases = hello_client::runner::parse_collection(&content, params, base_dir)?;
        if !cases.is_empty() {
            let names: Vec<String> = cases.iter().map(|tc| tc.name.clone()).collect();
            folder_specs.push((name.clone(), names));
            all_cases.extend(cases);
        }
    }

    Ok((folder_specs, all_cases))
}

/// Re-parse with current `app.params`, reset app state, spawn a new runner thread.
fn rerun(app: &mut App, sources: &Sources) -> Result<mpsc::Receiver<RunnerEvent>, String> {
    let (folder_specs, test_cases) = load_collection(sources, &app.params)?;
    if test_cases.is_empty() {
        return Err("no test cases found".to_string());
    }
    app.apply_rerun(folder_specs);
    let (tx, rx) = mpsc::sync_channel::<RunnerEvent>(64);
    runner::spawn_runner(test_cases, app.params.clone(), tx);
    Ok(rx)
}

/// Load a new collection from `path` (file or directory), update `sources`,
/// reset the app, and spawn a fresh runner thread.
fn open_collection(
    app: &mut App,
    sources: &mut Sources,
    path: &Path,
    rx: &mut mpsc::Receiver<RunnerEvent>,
) -> Result<(), String> {
    let new_sources = load_sources(path);
    if new_sources.is_empty() {
        return Err(format!("no .http files found in {}", path.display()));
    }
    let (folder_specs, test_cases) = load_collection(&new_sources, &app.params)?;
    if test_cases.is_empty() {
        return Err("no test cases found".to_string());
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
    app.apply_rerun(folder_specs);
    let (tx, new_rx) = mpsc::sync_channel::<RunnerEvent>(64);
    runner::spawn_runner(test_cases, app.params.clone(), tx);
    *rx = new_rx;
    Ok(())
}

// ─── TUI event loop ───────────────────────────────────────────────────────────

fn run_tui(
    app: &mut App,
    rx: mpsc::Receiver<RunnerEvent>,
    sources: &mut Sources,
) -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let tick = Duration::from_millis(80);
    let mut last_tick = Instant::now();
    let mut rx = rx;

    loop {
        terminal.draw(|f| ui::render(f, app))?;

        // Drain all pending runner events non-blockingly.
        while let Ok(ev) = rx.try_recv() {
            match ev {
                RunnerEvent::TestStarted(i) => app.on_test_started(i),
                RunnerEvent::TestFinished(i, r) => app.on_test_finished(i, *r),
                RunnerEvent::Done { elapsed_ms } => app.on_done(elapsed_ms),
                RunnerEvent::Error(msg) => app.on_error(msg),
            }
        }

        // Block up to the remaining tick budget for a key event.
        let timeout = tick.saturating_sub(last_tick.elapsed());
        if poll(timeout)?
            && let Event::Key(key) = read()?
        {
            if app.show_browser {
                handle_browser_key(app, key, sources, &mut rx);
            } else if app.show_params {
                handle_param_editor_key(app, key, sources, &mut rx);
            } else {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Up | KeyCode::Char('k') => app.select_up(),
                    KeyCode::Down | KeyCode::Char('j') => app.select_down(),
                    KeyCode::Enter => app.toggle_folder(),
                    KeyCode::Char('u') => app.scroll_detail(-3),
                    KeyCode::Char('d') => app.scroll_detail(3),
                    KeyCode::Char('l') => app.show_logs = !app.show_logs,
                    KeyCode::Char('e') => {
                        app.param_editor = ParamEditor::from_params(&app.params);
                        app.show_params = true;
                    },
                    KeyCode::Char('o') => {
                        // Start browser at the directory of the current collection.
                        let start = sources
                            .first()
                            .and_then(|(_, p)| p.parent())
                            .map(|p| p.to_path_buf())
                            .or_else(|| std::env::current_dir().ok())
                            .unwrap_or_else(|| PathBuf::from("."));
                        app.open_browser(start);
                    },
                    KeyCode::Char('r') => match rerun(app, sources) {
                        Ok(new_rx) => rx = new_rx,
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

fn handle_param_editor_key(
    app: &mut App,
    key: KeyEvent,
    sources: &Sources,
    rx: &mut mpsc::Receiver<RunnerEvent>,
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
                Ok(new_rx) => *rx = new_rx,
                Err(e) => app.on_error(e),
            }
        },
        KeyCode::Backspace if editing => app.param_editor.input_backspace(),
        KeyCode::Char(c) if editing => app.param_editor.input_char(c),
        _ => {},
    }
}

fn handle_browser_key(
    app: &mut App,
    key: KeyEvent,
    sources: &mut Sources,
    rx: &mut mpsc::Receiver<RunnerEvent>,
) {
    match key.code {
        KeyCode::Esc => app.show_browser = false,
        KeyCode::Up | KeyCode::Char('k') => app.browser.nav_up(),
        KeyCode::Down | KeyCode::Char('j') => app.browser.nav_down(),
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => app.browser.go_parent(),
        KeyCode::Char(' ') => {
            // Open the currently browsed directory as a collection.
            let path = app.browser.cwd.clone();
            match open_collection(app, sources, &path, rx) {
                Ok(()) => app.show_browser = false,
                Err(e) => app.on_error(e),
            }
        },
        KeyCode::Enter => {
            let entry = app.browser.selected().cloned();
            match entry {
                Some(BrowserEntry::ParentDir) => app.browser.go_parent(),
                Some(BrowserEntry::Dir(_, path)) => app.browser.enter_dir(path),
                Some(BrowserEntry::HttpFile(_, path)) => {
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

// ─── Argument parsing ─────────────────────────────────────────────────────────

fn parse_args() -> (String, HashMap<String, String>, Option<String>) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut path = String::from("requests.http");
    let mut params: HashMap<String, String> = HashMap::new();
    let mut debug_log: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-p" | "--param" if i + 1 < args.len() => {
                i += 1;
                if let Some((k, v)) = args[i].split_once('=') {
                    params.insert(k.to_string(), v.to_string());
                }
            },
            "-r" | "--request" if i + 1 < args.len() => {
                i += 1;
                path = args[i].clone();
            },
            "--debug-log" if i + 1 < args.len() => {
                i += 1;
                debug_log = Some(args[i].clone());
            },
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            },
            arg if !arg.starts_with('-') => {
                path = arg.to_string();
            },
            _ => {},
        }
        i += 1;
    }

    (path, params, debug_log)
}

fn print_help() {
    println!("hello_tui — interactive HTTP test runner\n");
    println!("USAGE:");
    println!("  hello_tui [OPTIONS] [FILE_OR_DIR]\n");
    println!("OPTIONS:");
    println!("  -r, --request <FILE|DIR>  .http file or directory of .http files");
    println!("  -p, --param <KEY=VALUE>   variable substitution (repeatable)");
    println!("  --debug-log <FILE>        mirror debug log to file");
    println!("  -h, --help                print this help\n");
    println!("KEYS (main view):");
    println!("  j / ↓       next item");
    println!("  k / ↑       previous item");
    println!("  Enter       expand / collapse folder");
    println!("  d           scroll detail down");
    println!("  u           scroll detail up");
    println!("  l           toggle logs");
    println!("  o           open file browser to load another collection");
    println!("  e           open environment params editor");
    println!("  r           rerun all tests");
    println!("  ?           toggle debug overlay");
    println!("  [ / ]       scroll debug overlay");
    println!("  q / Esc     quit\n");
    println!("KEYS (param editor):");
    println!("  j / k       navigate rows");
    println!("  Enter / e   edit value");
    println!("  Tab         edit key");
    println!("  a           add new param");
    println!("  d           delete param");
    println!("  r           save and rerun");
    println!("  Esc         close (discard changes)");
}
