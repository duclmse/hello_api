use std::{collections::HashMap, path::PathBuf};

use hello_client::{TestCase, TestResult};

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
    /// No collection is loaded yet.
    Empty,
    /// Collection loaded, no run in progress.
    Idle,
    Running {
        current: usize,
    },
    Done {
        elapsed_ms: u128,
    },
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

/// Return a short format label for recognised collection file extensions.
fn collection_format_label(path: &std::path::Path) -> Option<&'static str> {
    match path.extension()?.to_str()? {
        "http" | "rest" => Some("http"),
        "flow" => Some("flow"),
        "bru" => Some("bru"),
        "json" => Some("json"),
        "yaml" | "yml" => Some("yaml"),
        _ => None,
    }
}

/// Format labels for `CollectionFile` entries in the browser.
/// Each label is the same string used in `CONVERT_FORMATS`.
#[derive(Clone)]
pub enum BrowserEntry {
    ParentDir,
    Dir(String, PathBuf),
    /// `(display_name, path, format_label)` — format_label is one of "http", "json", "bru", "yaml"
    CollectionFile(String, PathBuf, &'static str),
}

/// `(id, extension, display_label)` — output formats selectable in the convert overlay.
pub const CONVERT_FORMATS: &[(&str, &str, &str)] = &[
    ("http", ".http", "HTTP (.http)"),
    ("postman", ".json", "Postman v2.1 (.json)"),
    ("opencollection", ".json", "OpenCollection (.json)"),
    ("openapi", ".yaml", "OpenAPI 3.0 (.yaml)"),
    ("curl", ".sh", "curl commands (.sh)"),
];

pub struct ConvertOverlay {
    pub format_cursor: usize,
    pub output_path: String,
    pub editing: bool,
    pub message: Option<String>,
}

// ── Detail tab ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum DetailTab {
    Request,
    Result,
}

// ── Request editor ────────────────────────────────────────────────────────────

/// Extract the multipart boundary value from a set of request headers.
pub fn extract_boundary(headers: &[(String, String)]) -> Option<String> {
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("content-type") {
            for part in v.split(';') {
                let part = part.trim();
                if let Some(rest) = part.strip_prefix("boundary=") {
                    return Some(rest.trim_matches('"').to_string());
                }
            }
        }
    }
    None
}

fn extract_form_field_name(line: &str) -> Option<String> {
    if !line.to_ascii_lowercase().starts_with("content-disposition") {
        return None;
    }
    for part in line.split(';') {
        let part = part.trim();
        if part.to_ascii_lowercase().starts_with("name=") {
            return Some(part[5..].trim_matches('"').to_string());
        }
    }
    None
}

/// Parse a multipart/form-data body into `(name, value)` pairs.
/// Returns `None` if the Content-Type header is not multipart/form-data.
pub fn parse_multipart_fields(
    headers: &[(String, String)],
    body: &str,
) -> Option<Vec<(String, String)>> {
    let boundary = extract_boundary(headers)?;
    let is_multipart = headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("content-type")
            && v.to_ascii_lowercase().contains("multipart/form-data")
    });
    if !is_multipart {
        return None;
    }

    let delimiter = format!("--{}", boundary);
    let end_delimiter = format!("--{}--", boundary);
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut in_part = false;
    let mut headers_done = false;
    let mut current_name: Option<String> = None;
    let mut current_value_lines: Vec<String> = Vec::new();

    for line in body.lines() {
        if line == end_delimiter || line.starts_with(&end_delimiter) {
            if let Some(name) = current_name.take() {
                while current_value_lines.last().map(|l: &String| l.is_empty()) == Some(true) {
                    current_value_lines.pop();
                }
                fields.push((name, current_value_lines.join("\n")));
            }
            break;
        } else if line == delimiter || line.starts_with(&delimiter) {
            if let Some(name) = current_name.take() {
                while current_value_lines.last().map(|l: &String| l.is_empty()) == Some(true) {
                    current_value_lines.pop();
                }
                fields.push((name, current_value_lines.join("\n")));
                current_value_lines.clear();
            }
            in_part = true;
            headers_done = false;
        } else if in_part && !headers_done {
            if line.is_empty() {
                headers_done = true;
            } else if let Some(name) = extract_form_field_name(line) {
                current_name = Some(name);
            }
        } else if in_part && headers_done {
            current_value_lines.push(line.to_string());
        }
    }

    Some(fields)
}

/// Serialize form fields back to a multipart/form-data body string.
pub fn serialize_form_body(boundary: &str, fields: &[(String, String)]) -> String {
    let mut body = String::new();
    for (name, value) in fields {
        body.push_str(&format!("--{}\n", boundary));
        body.push_str(&format!("Content-Disposition: form-data; name=\"{}\"\n", name));
        body.push('\n');
        body.push_str(value);
        body.push('\n');
    }
    body.push_str(&format!("--{}--\n", boundary));
    body
}

/// An in-place overlay for viewing and editing a single test's request.
pub struct RequestEditor {
    pub test_idx: usize,
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    /// Parsed form fields when Content-Type is multipart/form-data; None otherwise.
    pub form_fields: Option<Vec<(String, String)>>,
    /// Cursor row: 0=Method, 1=URL, 2..=2+h-1=Header[i], body_row()..=form rows or Body
    pub cursor: usize,
    pub editing: bool,
    /// When on a header or form-field row, true means editing the key/name column.
    pub edit_key: bool,
    pub input: String,
    pub message: Option<String>,
}

impl RequestEditor {
    /// Total number of navigable rows.
    pub fn row_count(&self) -> usize {
        let br = self.body_row();
        match &self.form_fields {
            None => br + 1,
            Some(fields) => br + fields.len().max(1),
        }
    }

    /// Index of the body row (or first form-field row when in form mode).
    pub fn body_row(&self) -> usize {
        2 + self.headers.len()
    }

    /// When in form mode and cursor is in the form section, returns the field index.
    fn form_field_idx(&self) -> Option<usize> {
        if self.form_fields.is_some() && self.cursor >= self.body_row() {
            Some(self.cursor - self.body_row())
        } else {
            None
        }
    }

    pub fn nav_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn nav_down(&mut self) {
        if self.cursor + 1 < self.row_count() {
            self.cursor += 1;
        }
    }

    /// Start editing the currently focused row. Returns false if nothing to edit.
    pub fn start_edit(&mut self) -> bool {
        let br = self.body_row();
        self.input = if self.cursor == 0 {
            self.method.clone()
        } else if self.cursor == 1 {
            self.url.clone()
        } else if self.cursor == br && self.form_fields.is_none() {
            self.body.clone()
        } else if self.form_fields.is_some() && self.cursor >= br {
            let fi = self.cursor - br;
            match self.form_fields.as_ref().and_then(|f| f.get(fi)) {
                Some((k, v)) => if self.edit_key { k.clone() } else { v.clone() },
                None => return false,
            }
        } else {
            let hi = self.cursor - 2;
            if self.edit_key {
                self.headers.get(hi).map(|(k, _)| k.clone()).unwrap_or_default()
            } else {
                self.headers.get(hi).map(|(_, v)| v.clone()).unwrap_or_default()
            }
        };
        self.editing = true;
        true
    }

    /// Confirm the current edit; advances key→value for header and form-field rows.
    pub fn confirm_edit(&mut self) {
        let br = self.body_row();
        if self.cursor == 0 {
            self.method = self.input.clone();
        } else if self.cursor == 1 {
            self.url = self.input.clone();
        } else if self.cursor == br && self.form_fields.is_none() {
            self.body = self.input.clone();
        } else if self.form_fields.is_some() && self.cursor >= br {
            let fi = self.cursor - br;
            let fields_len = self.form_fields.as_ref().map(|f| f.len()).unwrap_or(0);
            if fi < fields_len {
                if self.edit_key {
                    self.form_fields.as_mut().unwrap()[fi].0 = self.input.clone();
                    self.edit_key = false;
                    self.input = self.form_fields.as_ref().unwrap()[fi].1.clone();
                    return;
                } else {
                    self.form_fields.as_mut().unwrap()[fi].1 = self.input.clone();
                    self.edit_key = true;
                }
            }
        } else {
            let hi = self.cursor - 2;
            if self.edit_key {
                if let Some(row) = self.headers.get_mut(hi) {
                    row.0 = self.input.clone();
                }
                self.edit_key = false;
                if let Some(row) = self.headers.get(hi) {
                    self.input = row.1.clone();
                } else {
                    self.input.clear();
                }
                return;
            } else {
                if let Some(row) = self.headers.get_mut(hi) {
                    row.1 = self.input.clone();
                }
                self.edit_key = true;
            }
        }
        self.editing = false;
        self.input.clear();
    }

    pub fn cancel_edit(&mut self) {
        self.editing = false;
        self.input.clear();
        self.edit_key = true;
    }

    pub fn add_header(&mut self) {
        let insert = (self.cursor + 1).min(2 + self.headers.len()) - 2;
        let insert = insert.min(self.headers.len());
        self.headers.insert(insert, (String::new(), String::new()));
        self.cursor = 2 + insert;
        self.edit_key = true;
        self.start_edit();
    }

    pub fn delete_header(&mut self) {
        if self.cursor >= 2 && self.cursor < self.body_row() {
            let hi = self.cursor - 2;
            self.headers.remove(hi);
            if self.cursor >= self.row_count() {
                self.cursor = self.row_count().saturating_sub(1);
            }
        }
    }

    /// Add a new empty form field (only when in form mode).
    pub fn add_form_field(&mut self) {
        if self.form_fields.is_none() {
            return;
        }
        let br = self.body_row();
        let fields_len = self.form_fields.as_ref().unwrap().len();
        let fi = if self.cursor >= br {
            (self.cursor - br + 1).min(fields_len)
        } else {
            fields_len
        };
        self.form_fields.as_mut().unwrap().insert(fi, (String::new(), String::new()));
        self.cursor = br + fi;
        self.edit_key = true;
        self.start_edit();
    }

    /// Delete the focused form field (only when in form mode and on a field row).
    pub fn delete_form_field(&mut self) {
        if self.form_fields.is_none() {
            return;
        }
        if let Some(fi) = self.form_field_idx() {
            let fields_len = self.form_fields.as_ref().unwrap().len();
            if fi < fields_len {
                self.form_fields.as_mut().unwrap().remove(fi);
                let rc = self.row_count();
                if self.cursor >= rc {
                    self.cursor = rc.saturating_sub(1);
                }
            }
        }
    }

    pub fn input_char(&mut self, c: char) {
        if self.editing {
            self.input.push(c);
        }
    }

    pub fn input_backspace(&mut self) {
        if self.editing {
            self.input.pop();
        }
    }
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

    /// Reload directory listing for `cwd`, sorted dirs first then supported collection files.
    pub fn refresh(&mut self) {
        self.entries.clear();
        if self.cwd.parent().is_some() {
            self.entries.push(BrowserEntry::ParentDir);
        }
        if let Ok(rd) = std::fs::read_dir(&self.cwd) {
            let mut dirs: Vec<(String, PathBuf)> = Vec::new();
            let mut files: Vec<(String, PathBuf, &'static str)> = Vec::new();
            for entry in rd.filter_map(|e| e.ok()) {
                let path = entry.path();
                let name =
                    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if name.starts_with('.') {
                    continue;
                }
                if path.is_dir() {
                    dirs.push((name, path));
                } else if let Some(label) = collection_format_label(&path) {
                    files.push((name, path, label));
                }
            }
            dirs.sort_by(|a, b| a.0.cmp(&b.0));
            files.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, path) in dirs {
                self.entries.push(BrowserEntry::Dir(name, path));
            }
            for (name, path, label) in files {
                self.entries.push(BrowserEntry::CollectionFile(name, path, label));
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
    pub cases: Vec<TestCase>,
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
    /// Cached raw lines of the currently highlighted collection file (empty = nothing selected).
    pub browser_preview_lines: Vec<String>,
    // ── convert overlay ──
    pub show_convert: bool,
    pub convert: ConvertOverlay,
    // ── environment ──
    /// Active Bruno environment name (--env flag).
    pub env_name: Option<String>,
    // ── detail tab ──
    pub detail_tab: DetailTab,
    // ── request editor ──
    pub show_request_editor: bool,
    pub request_editor: RequestEditor,
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
        cases: Vec<TestCase>,
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
        let initial_phase = if total == 0 {
            Phase::Empty
        } else {
            Phase::Idle
        };
        let mut app = Self {
            title,
            tests,
            cases,
            folders,
            tree: Vec::new(),
            cursor: 0,
            phase: initial_phase,
            detail_scroll: 0,
            show_logs: false,
            params,
            show_params: false,
            param_editor,
            show_browser: false,
            browser,
            browser_preview_lines: Vec::new(),
            show_convert: false,
            convert: ConvertOverlay {
                format_cursor: 0,
                output_path: String::new(),
                editing: false,
                message: None,
            },
            env_name: None,
            detail_tab: DetailTab::Request,
            show_request_editor: false,
            request_editor: RequestEditor {
                test_idx: 0,
                method: String::new(),
                url: String::new(),
                headers: Vec::new(),
                body: String::new(),
                form_fields: None,
                cursor: 0,
                editing: false,
                edit_key: true,
                input: String::new(),
                message: None,
            },
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

    /// Open the convert overlay, pre-filling the output path suggestion.
    pub fn open_convert(&mut self, suggested_path: String) {
        self.convert.format_cursor = 0;
        self.convert.output_path = suggested_path;
        self.convert.editing = false;
        self.convert.message = None;
        self.show_convert = true;
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

    /// Load a new collection without running — resets all test statuses to Pending.
    pub fn apply_collection(&mut self, folder_specs: Vec<FolderSpec>, cases: Vec<TestCase>) {
        self.cases = cases;
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
        self.phase = Phase::Idle;
        self.detail_scroll = 0;
        self.detail_tab = DetailTab::Request;
    }

    /// Rebuild from new specs and immediately mark as running (used when rerunning all).
    pub fn apply_rerun(&mut self, folder_specs: Vec<FolderSpec>, cases: Vec<TestCase>) {
        self.apply_collection(folder_specs, cases);
        if !self.tests.is_empty() {
            self.phase = Phase::Running { current: 0 };
        }
    }

    /// Called when a single-test run completes — returns to Idle.
    pub fn on_done_single(&mut self) {
        self.debug.log("DoneSingle");
        self.phase = Phase::Idle;
    }

    // ── detail tab ───────────────────────────────────────────────────────────

    pub fn toggle_detail_tab(&mut self) {
        self.detail_tab = match self.detail_tab {
            DetailTab::Request => DetailTab::Result,
            DetailTab::Result => DetailTab::Request,
        };
        self.detail_scroll = 0;
    }

    // ── request editor ───────────────────────────────────────────────────────

    /// Open the request editor populated from `app.cases[ti]`.
    pub fn open_request_editor(&mut self, ti: usize) {
        if let Some(case) = self.cases.get(ti) {
            let body = case.request.body.clone().unwrap_or_default();
            let form_fields = parse_multipart_fields(&case.request.headers, &body);
            self.request_editor = RequestEditor {
                test_idx: ti,
                method: case.request.method.clone(),
                url: case.request.url.clone(),
                headers: case.request.headers.clone(),
                body,
                form_fields,
                cursor: 0,
                editing: false,
                edit_key: true,
                input: String::new(),
                message: None,
            };
            self.show_request_editor = true;
        }
    }

    /// Write the editor state back to `app.cases[ti]` and close the editor.
    pub fn save_request_editor(&mut self) {
        let ti = self.request_editor.test_idx;
        if let Some(case) = self.cases.get_mut(ti) {
            case.request.method = self.request_editor.method.clone();
            case.request.url = self.request_editor.url.clone();
            case.request.headers = self.request_editor.headers.clone();
            case.request.body = if let Some(fields) = &self.request_editor.form_fields {
                // Re-serialize the edited form fields back to multipart body.
                if let Some(boundary) = extract_boundary(&self.request_editor.headers) {
                    let s = serialize_form_body(&boundary, fields);
                    if s.is_empty() { None } else { Some(s) }
                } else {
                    None
                }
            } else if self.request_editor.body.is_empty() {
                None
            } else {
                Some(self.request_editor.body.clone())
            };
        }
        self.show_request_editor = false;
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
            Phase::Empty => "Empty".to_string(),
            Phase::Idle => "Idle".to_string(),
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
