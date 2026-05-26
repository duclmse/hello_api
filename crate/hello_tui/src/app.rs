use std::{collections::HashMap, path::PathBuf};

use hello_client::TestResult;

use crate::debug::DebugLog;

const SPINNER: [char; 8] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧'];

// ── FolderSpec type alias ─────────────────────────────────────────────────────

/// `(display_name, test_names)` — shape produced by the loader in main.rs.
pub type FolderSpec = (String, Vec<String>);

// ── Test row ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum TestStatus {
    Pending,
    Running,
    Passed,
    Failed,
}

pub struct TestRow {
    pub name: String,
    pub status: TestStatus,
    pub result: Option<TestResult>,
}

impl TestRow {
    pub fn response_time_ms(&self) -> Option<u64> {
        self.result.as_ref()?.response.as_ref().map(|r| r.response_time_ms)
    }
}

// ── Phase ─────────────────────────────────────────────────────────────────────

pub enum Phase {
    Running { current: usize },
    Done { elapsed_ms: u128 },
    Error(String),
}

// ── Folder tree ───────────────────────────────────────────────────────────────

/// A group of tests from one `.http` file.
pub struct Folder {
    pub name: String,
    pub collapsed: bool,
    pub test_start: usize,
    pub test_count: usize,
}

/// A visible row in the test tree.
#[derive(Clone, Copy)]
pub enum TreeItem {
    Folder(usize), // index into App::folders
    Test(usize),   // index into App::tests
}

// ── Param editor ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum ParamEditorMode {
    Navigate,
    EditKey,
    EditValue,
}

pub struct ParamEditor {
    pub rows: Vec<(String, String)>,
    pub cursor: usize,
    pub mode: ParamEditorMode,
    pub input: String,
}

impl ParamEditor {
    pub fn from_params(params: &HashMap<String, String>) -> Self {
        let mut rows: Vec<(String, String)> =
            params.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            rows,
            cursor: 0,
            mode: ParamEditorMode::Navigate,
            input: String::new(),
        }
    }

    pub fn to_params(&self) -> HashMap<String, String> {
        self.rows
            .iter()
            .filter(|(k, _)| !k.is_empty())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    pub fn is_editing(&self) -> bool {
        self.mode != ParamEditorMode::Navigate
    }

    pub fn nav_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn nav_down(&mut self) {
        if self.cursor + 1 < self.rows.len() {
            self.cursor += 1;
        }
    }

    /// Add a blank row and start editing its key.
    pub fn add_row(&mut self) {
        self.rows.push((String::new(), String::new()));
        self.cursor = self.rows.len() - 1;
        self.input.clear();
        self.mode = ParamEditorMode::EditKey;
    }

    pub fn delete_row(&mut self) {
        if !self.rows.is_empty() {
            self.rows.remove(self.cursor);
            if self.cursor >= self.rows.len() && self.cursor > 0 {
                self.cursor -= 1;
            }
        }
    }

    pub fn start_edit_value(&mut self) {
        if let Some((_, v)) = self.rows.get(self.cursor) {
            self.input = v.clone();
            self.mode = ParamEditorMode::EditValue;
        }
    }

    pub fn start_edit_key(&mut self) {
        if let Some((k, _)) = self.rows.get(self.cursor) {
            self.input = k.clone();
            self.mode = ParamEditorMode::EditKey;
        }
    }

    /// Confirm current edit:
    /// - `EditKey`   → save key, transition to `EditValue`.
    /// - `EditValue` → save value, return to `Navigate`.
    pub fn confirm_edit(&mut self) {
        match self.mode {
            ParamEditorMode::EditKey => {
                if let Some(row) = self.rows.get_mut(self.cursor) {
                    row.0 = self.input.clone();
                }
                if let Some(row) = self.rows.get(self.cursor) {
                    self.input = row.1.clone();
                }
                self.mode = ParamEditorMode::EditValue;
            },
            ParamEditorMode::EditValue => {
                if let Some(row) = self.rows.get_mut(self.cursor) {
                    row.1 = self.input.clone();
                }
                self.mode = ParamEditorMode::Navigate;
                self.input.clear();
            },
            ParamEditorMode::Navigate => {},
        }
    }

    pub fn cancel_edit(&mut self) {
        self.mode = ParamEditorMode::Navigate;
        self.input.clear();
    }

    pub fn input_char(&mut self, c: char) {
        if self.mode != ParamEditorMode::Navigate {
            self.input.push(c);
        }
    }

    pub fn input_backspace(&mut self) {
        if self.mode != ParamEditorMode::Navigate {
            self.input.pop();
        }
    }
}

// ── File browser ─────────────────────────────────────────────────────────────

#[derive(Clone)]
pub enum BrowserEntry {
    ParentDir,
    Dir(String, PathBuf),
    HttpFile(String, PathBuf),
}

pub struct FileBrowser {
    pub cwd: PathBuf,
    pub entries: Vec<BrowserEntry>,
    pub cursor: usize,
}

impl FileBrowser {
    pub fn new(start: PathBuf) -> Self {
        let mut b = Self {
            cwd: start,
            entries: Vec::new(),
            cursor: 0,
        };
        b.refresh();
        b
    }

    /// Reload directory listing for `cwd`, sorted dirs first then `.http` files.
    pub fn refresh(&mut self) {
        self.entries.clear();
        if self.cwd.parent().is_some() {
            self.entries.push(BrowserEntry::ParentDir);
        }
        if let Ok(rd) = std::fs::read_dir(&self.cwd) {
            let mut dirs: Vec<(String, PathBuf)> = Vec::new();
            let mut files: Vec<(String, PathBuf)> = Vec::new();
            for entry in rd.filter_map(|e| e.ok()) {
                let path = entry.path();
                let name =
                    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if name.starts_with('.') {
                    continue; // skip hidden entries
                }
                if path.is_dir() {
                    dirs.push((name, path));
                } else if path.extension().is_some_and(|e| e == "http") {
                    files.push((name, path));
                }
            }
            dirs.sort_by(|a, b| a.0.cmp(&b.0));
            files.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, path) in dirs {
                self.entries.push(BrowserEntry::Dir(name, path));
            }
            for (name, path) in files {
                self.entries.push(BrowserEntry::HttpFile(name, path));
            }
        }
        self.cursor = 0;
    }

    pub fn selected(&self) -> Option<&BrowserEntry> {
        self.entries.get(self.cursor)
    }

    pub fn nav_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn nav_down(&mut self) {
        if self.cursor + 1 < self.entries.len() {
            self.cursor += 1;
        }
    }

    pub fn enter_dir(&mut self, path: PathBuf) {
        self.cwd = path;
        self.refresh();
    }

    pub fn go_parent(&mut self) {
        if let Some(parent) = self.cwd.parent().map(|p| p.to_path_buf()) {
            self.cwd = parent;
            self.refresh();
        }
    }
}

// ── App ──────────────────────────────────────────────────────────────────────

pub struct App {
    pub title: String,
    pub tests: Vec<TestRow>,
    pub folders: Vec<Folder>,
    pub tree: Vec<TreeItem>,
    pub cursor: usize,
    pub phase: Phase,
    pub detail_scroll: u16,
    pub show_logs: bool,
    pub params: HashMap<String, String>,
    pub show_params: bool,
    pub param_editor: ParamEditor,
    // ── file browser ──
    pub show_browser: bool,
    pub browser: FileBrowser,
    // ── debug ──
    pub debug: DebugLog,
    pub show_debug: bool,
    pub debug_scroll: u16,
    // ── internal ──
    spinner_tick: usize,
}

impl App {
    pub fn new(
        title: String,
        folder_specs: Vec<FolderSpec>,
        params: HashMap<String, String>,
        mut debug: DebugLog,
    ) -> Self {
        let total: usize = folder_specs.iter().map(|(_, ns)| ns.len()).sum();
        debug.log(format!(
            "init: {} test(s) across {} folder(s) — {}",
            total,
            folder_specs.len(),
            title
        ));

        let mut tests: Vec<TestRow> = Vec::with_capacity(total);
        let mut folders: Vec<Folder> = Vec::with_capacity(folder_specs.len());

        for (name, names) in &folder_specs {
            let test_start = tests.len();
            for n in names {
                tests.push(TestRow {
                    name: n.clone(),
                    status: TestStatus::Pending,
                    result: None,
                });
            }
            folders.push(Folder {
                name: name.clone(),
                collapsed: false,
                test_start,
                test_count: names.len(),
            });
        }

        let param_editor = ParamEditor::from_params(&params);
        let browser =
            FileBrowser::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let mut app = Self {
            title,
            tests,
            folders,
            tree: Vec::new(),
            cursor: 0,
            phase: Phase::Running { current: 0 },
            detail_scroll: 0,
            show_logs: false,
            params,
            show_params: false,
            param_editor,
            show_browser: false,
            browser,
            debug,
            show_debug: false,
            debug_scroll: 0,
            spinner_tick: 0,
        };
        app.rebuild_tree();
        // If the tree starts with a folder header, land cursor on the first test.
        if matches!(app.tree.first(), Some(TreeItem::Folder(_))) && app.tree.len() > 1 {
            app.cursor = 1;
        }
        app
    }

    // ── tree ─────────────────────────────────────────────────────────────────

    pub fn rebuild_tree(&mut self) {
        self.tree.clear();
        let show_folders = self.folders.len() > 1;
        for (fi, folder) in self.folders.iter().enumerate() {
            if show_folders {
                self.tree.push(TreeItem::Folder(fi));
                if folder.collapsed {
                    continue; // skip tests for this folder
                }
            }
            for ti in folder.test_start..(folder.test_start + folder.test_count) {
                self.tree.push(TreeItem::Test(ti));
            }
        }
    }

    pub fn toggle_folder(&mut self) {
        if let Some(TreeItem::Folder(fi)) = self.tree.get(self.cursor).copied() {
            self.folders[fi].collapsed = !self.folders[fi].collapsed;
            self.rebuild_tree();
            self.cursor = self.cursor.min(self.tree.len().saturating_sub(1));
        }
    }

    /// Open the file browser starting at `start` directory.
    pub fn open_browser(&mut self, start: PathBuf) {
        self.browser = FileBrowser::new(start);
        self.show_browser = true;
    }

    // ── runner event handlers ─────────────────────────────────────────────────

    pub fn on_test_started(&mut self, idx: usize) {
        let name = self.tests.get(idx).map(|t| t.name.as_str()).unwrap_or("?");
        self.debug.log(format!("TestStarted({})  {}", idx, name));
        if let Some(t) = self.tests.get_mut(idx) {
            t.status = TestStatus::Running;
        }
        self.phase = Phase::Running { current: idx };
    }

    pub fn on_test_finished(&mut self, idx: usize, result: TestResult) {
        let status_code = result.response.as_ref().map(|r| r.status).unwrap_or(0);
        let ms = result.response.as_ref().map(|r| r.response_time_ms).unwrap_or(0);
        let verdict = if result.passed { "PASS" } else { "FAIL" };
        self.debug
            .log(format!("TestFinished({})  {}  HTTP {}  {}ms", idx, verdict, status_code, ms));
        for f in &result.failures {
            self.debug.log(format!("  · {}", f));
        }
        if let Some(t) = self.tests.get_mut(idx) {
            t.status = if result.passed {
                TestStatus::Passed
            } else {
                TestStatus::Failed
            };
            t.result = Some(result);
        }
    }

    pub fn on_done(&mut self, elapsed_ms: u128) {
        let (p, f) = self.summary();
        self.debug.log(format!("Done  {}ms  passed={} failed={}", elapsed_ms, p, f));
        self.phase = Phase::Done { elapsed_ms };
    }

    pub fn on_error(&mut self, msg: String) {
        self.debug.log(format!("Error: {}", msg));
        self.phase = Phase::Error(msg);
    }

    /// Rebuild tests and folders from new specs (used on rerun with changed params).
    pub fn apply_rerun(&mut self, folder_specs: Vec<FolderSpec>) {
        self.tests.clear();
        self.folders.clear();
        for (name, names) in folder_specs {
            let test_start = self.tests.len();
            let test_count = names.len();
            for n in names {
                self.tests.push(TestRow {
                    name: n,
                    status: TestStatus::Pending,
                    result: None,
                });
            }
            self.folders.push(Folder {
                name,
                collapsed: false,
                test_start,
                test_count,
            });
        }
        self.rebuild_tree();
        self.cursor = 0;
        if matches!(self.tree.first(), Some(TreeItem::Folder(_))) && self.tree.len() > 1 {
            self.cursor = 1;
        }
        self.phase = Phase::Running { current: 0 };
        self.detail_scroll = 0;
    }

    // ── navigation ────────────────────────────────────────────────────────────

    pub fn select_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.detail_scroll = 0;
        }
    }

    pub fn select_down(&mut self) {
        if self.cursor + 1 < self.tree.len() {
            self.cursor += 1;
            self.detail_scroll = 0;
        }
    }

    pub fn scroll_detail(&mut self, delta: i16) {
        if delta < 0 {
            self.detail_scroll = self.detail_scroll.saturating_sub((-delta) as u16);
        } else {
            self.detail_scroll = self.detail_scroll.saturating_add(delta as u16);
        }
    }

    pub fn scroll_debug(&mut self, delta: i16) {
        if delta < 0 {
            self.debug_scroll = self.debug_scroll.saturating_sub((-delta) as u16);
        } else {
            self.debug_scroll = self.debug_scroll.saturating_add(delta as u16);
        }
    }

    // ── misc ──────────────────────────────────────────────────────────────────

    pub fn tick(&mut self) {
        self.spinner_tick = self.spinner_tick.wrapping_add(1);
    }

    pub fn spinner(&self) -> char {
        SPINNER[self.spinner_tick % SPINNER.len()]
    }

    /// `(passed, failed)` counts across all finished tests.
    pub fn summary(&self) -> (usize, usize) {
        let passed = self.tests.iter().filter(|t| t.status == TestStatus::Passed).count();
        let failed = self.tests.iter().filter(|t| t.status == TestStatus::Failed).count();
        (passed, failed)
    }

    /// `(passed, failed, pending/running)` counts for one folder.
    pub fn folder_stats(&self, fi: usize) -> (usize, usize, usize) {
        let folder = &self.folders[fi];
        let mut passed = 0usize;
        let mut failed = 0usize;
        let mut other = 0usize;
        for ti in folder.test_start..(folder.test_start + folder.test_count) {
            match self.tests[ti].status {
                TestStatus::Passed => passed += 1,
                TestStatus::Failed => failed += 1,
                _ => other += 1,
            }
        }
        (passed, failed, other)
    }

    /// One-line snapshot of current state for the debug overlay footer.
    pub fn state_line(&self) -> String {
        let phase = match &self.phase {
            Phase::Running { current } => format!("Running({})", current),
            Phase::Done { .. } => "Done".to_string(),
            Phase::Error(_) => "Error".to_string(),
        };
        format!(
            "cursor={}  tree={}  tests={}  folders={}  phase={}  scroll={}",
            self.cursor,
            self.tree.len(),
            self.tests.len(),
            self.folders.len(),
            phase,
            self.detail_scroll,
        )
    }
}
