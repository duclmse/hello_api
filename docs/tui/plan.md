# hello_tui — Implementation Plan

## Current state

`hello_tui` is a ratatui-based interactive runner for `.http` test collections.
It displays a four-row layout (header bar, test list, detail panel, status bar)
with three modal overlays (file browser, param editor, debug log). The runner
executes tests in a background OS thread via `mpsc::SyncSender<RunnerEvent>`,
satisfying the V8 single-thread constraint. The codebase has six source files:
`main.rs`, `app.rs`, `ui.rs`, `runner.rs`, `event.rs`, and `debug.rs`.

## Design principles

- **Single source of truth**: `App` owns all mutable state. `ui.rs` is a pure
  function of `&App`. `main.rs` drives events and mutates `App` through
  well-named methods only.
- **Additive data model**: new fields on `App`, `TestRow`, etc. are always
  `Option<_>` or default-constructible so `App::new` and `App::apply_rerun`
  require no structural changes.
- **Backward-compatible keybindings**: each phase only adds new keys or remaps
  keys that previously did nothing in the relevant context.
- **No async in the TUI thread**: the event loop in `main.rs` stays
  `std::thread`-based; new runner operations go on the existing background
  thread or a new dedicated thread communicating via `RunnerEvent`.

---

## Phase 1 — Detail panel redesign

### Goal

Replace the flat detail panel with a five-tab view (Request, Response, Failures,
Logs, Timings) so all data captured in `TestResult` is directly accessible
without toggling overlays.

---

### Feature 1.1 — Tabbed detail panel

**Description**: The detail panel shows a tab bar along its top border. The
active tab name is highlighted. `Tab` cycles forward; `Shift-Tab` (or `BackTab`
in crossterm terms) cycles backward.

**Data model (`app.rs`)**

Add to `App`:

```rust
pub detail_tab: DetailTab,
```

New enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum DetailTab {
    #[default]
    Request,
    Response,
    Failures,
    Logs,
    Timings,
}

impl DetailTab {
    pub fn next(self) -> Self { /* cycle forward */ }
    pub fn prev(self) -> Self { /* cycle backward */ }
    pub fn label(self) -> &'static str { /* "Request" | "Response" | ... */ }
}
```

Reset `detail_tab` to `DetailTab::default()` in `App::select_up`,
`App::select_down`, and `App::apply_rerun` so navigating away from a test clears
the stale tab state.

**UI (`ui.rs`)**

`render_test_detail` is split into:

```rust
fn render_detail_tab_bar(f, app, area: Rect)   // 1-line inside top border
fn render_detail_request(f, app, ti, area: Rect)
fn render_detail_response(f, app, ti, area: Rect)
fn render_detail_failures(f, app, ti, area: Rect)
fn render_detail_logs(f, app, ti, area: Rect)
fn render_detail_timings(f, app, ti, area: Rect)
```

Tab bar line: spans rendered as
`[Request]  Response   Failures   Logs   Timings`. Active tab uses
`Style::new().bold().underlined()` or a contrasting background. Inactive tabs
use `Color::DarkGray`.

The detail panel `Block` title changes to show the test name only; the tab bar
occupies the first interior line. `app.detail_scroll` is kept per-tab:

```rust
pub detail_scroll: [u16; 5],  // indexed by DetailTab as usize
```

(Replace the current `pub detail_scroll: u16` with this array.)

**Keybindings**

| Key                     | Action                     |
| ----------------------- | -------------------------- |
| `Tab`                   | Advance to next detail tab |
| `BackTab` (`Shift+Tab`) | Go to previous detail tab  |

These keys apply only when the main view is active (no overlay open).

**Notes**

- `crossterm::event::KeyCode::BackTab` is the canonical crossterm value for
  `Shift+Tab`; it is handled in the main `match key.code` arm.
- Existing `show_logs` field on `App` becomes redundant when the Logs tab is
  present. Keep the field for at least one release cycle and default-show logs
  when the Logs tab is active, so the `l` key can be repurposed as a shortcut to
  jump directly to the Logs tab.

---

### Feature 1.2 — Request tab

**Description**: Shows the effective HTTP request that was sent — method, URL,
headers as a two-column table, and the request body (scrollable). Data comes
from `TestResult::request: HttpRequest`.

**Data model (`app.rs`)**

`TestResult` (in `hello_client::http_runner`) already has:

```rust
pub request: HttpRequest,   // effective request (after pre-script + interpolation)
```

`TestRow` already holds `pub result: Option<TestResult>`, so no new fields are
needed on `TestRow`. The request is accessed as `test.result.as_ref()?.request`.

**UI (`ui.rs`)**

`render_detail_request` layout:

```
Method: GET
URL:    https://api.example.com/users/42

Headers:
  Authorization       Bearer tok-abc
  Content-Type        application/json
  Accept              */*

Body:
  (none)
```

Headers rendered as a `Table` widget with two columns: `Constraint::Length(24)`
for the name, `Constraint::Fill(1)` for the value. Body section: if
`HttpRequest::body` is `Some`, render with `Paragraph::scroll` using
`app.detail_scroll[DetailTab::Request as usize]`. Detect JSON by attempting
`serde_json::from_str` and pretty-print if successful.

**Keybindings**: `d`/`u` scroll the body section (same scroll keys, tab-scoped
via the new scroll array).

**Notes**

- For `Pending` or `Running` tests the request tab shows a placeholder:
  `"Not yet run."` / `"Running — request not yet available."`.
- If `result.request.body` is `None`, show `(none)` in dimmed style.

---

### Feature 1.3 — Response tab

**Description**: Shows status line, response headers table, and scrollable
response body. Detects JSON and pretty-prints it.

**Data model (`app.rs`)**

`TestResult::response: Option<HttpResponse>` is already present. `HttpResponse`
has `status: u16`, `ok: bool`, `headers: Vec<(String,String)>`, `body: String`,
`response_time_ms: u64`, `redirected: bool`.

No new fields needed.

**UI (`ui.rs`)**

`render_detail_response` layout:

```
HTTP 200 OK     142 ms     (redirected)

Headers:
  content-type        application/json
  content-length      482
  x-request-id        abc-123

Body (JSON):
  {
    "id": 42,
    "name": "Alice"
  }
```

Status line: colour-code the status code as the existing detail panel does
(green 2xx, yellow 3xx, red 4xx/5xx). Show `(redirected)` in dim style when
`HttpResponse::redirected` is true.

Body detection:

```rust
fn detect_body_format(body: &str) -> BodyFormat { /* Json | Plain */ }
```

If `BodyFormat::Json`, call `serde_json::from_str::<serde_json::Value>` then
`serde_json::to_string_pretty`. Display up to 4096 chars; truncate with a
`… (N bytes total)` indicator for large bodies.

**Keybindings**: `d`/`u` scroll the body, tab-scoped.

**Notes**

- `serde_json` is already in `hello_client`'s dependency tree and available to
  `hello_tui` via the re-exported types. Add `serde_json` directly to
  `hello_tui/Cargo.toml` for use in `ui.rs`.

---

### Feature 1.4 — Failures tab

**Description**: Lists each `String` in `TestResult::failures` as a bullet with
red highlight. Shows `All assertions passed.` in green when the list is empty
and the test passed.

**Data model (`app.rs`)**

No changes. Data from `test.result.as_ref()?.failures`.

**UI (`ui.rs`)**

`render_detail_failures`:

- If `result.failures.is_empty() && result.passed` → single green line.
- Otherwise, each failure on its own line: `· <message>` in `Color::Red`.
- Long failure messages word-wrap using `Wrap { trim: false }`.

**Keybindings**: `d`/`u` scroll (tab-scoped), useful when there are many
failures.

**Notes**

- This tab is the default landing tab when a test status transitions to
  `Failed`. Implement in `App::on_test_finished`: after setting
  `t.status = TestStatus::Failed`, also set
  `app.detail_tab = DetailTab::Failures` if the cursor is currently on that test
  index.

---

### Feature 1.5 — Logs tab

**Description**: Shows all entries from `TestResult::logs: Vec<String>`.
Replaces the existing `show_logs` toggle in the old panel.

**Data model (`app.rs`)**

No structural changes. The existing `show_logs: bool` field remains for backward
compatibility but the `l` key now switches to the Logs tab directly.

**UI (`ui.rs`)**

`render_detail_logs`:

- Header line: `Logs (N entries)` in cyan.
- Each entry as `  <text>` with multi-line support (split on `\n`).
- Empty log list: `No script output.` in dim style.

**Keybindings**

| Key     | Action                                                          |
| ------- | --------------------------------------------------------------- |
| `l`     | Switch active detail tab to `DetailTab::Logs` (replaces toggle) |
| `d`/`u` | Scroll log content (tab-scoped)                                 |

**Notes**

- Update `print_help()` in `main.rs` to say `[l] jump to logs tab`.

---

### Feature 1.6 — Timings tab

**Description**: Displays a horizontal stacked bar chart built from braille-
block characters showing relative time spent in each phase. Uses
`TestResult::phase_timings: PhaseTimings`.

`PhaseTimings` fields (from `hello_client::http_runner`):

```rust
pub struct PhaseTimings {
    pub collection_pre_ms: Option<u64>,
    pub pre_ms:            Option<u64>,
    pub fetch_ms:          Option<u64>,
    pub post_ms:           Option<u64>,
    pub collection_post_ms: Option<u64>,
}
```

**Data model (`app.rs`)**

No new fields. Accessed via `test.result.as_ref()?.phase_timings`.

**UI (`ui.rs`)**

`render_detail_timings` layout:

```
Phase timings

  pre_script        12 ms  ████░░░░░░░░░░░░░░░░░░░░░
  fetch            118 ms  ████████████████████░░░░░░
  post_script        8 ms  ████░░░░░░░░░░░░░░░░░░░░░░

  Total            138 ms
```

Bar implementation:

```rust
fn braille_bar(filled: u16, total_cells: u16) -> String {
    // Use Unicode braille blocks for sub-character resolution.
    // Full block: U+2588, 3/4: U+2586, 1/2: U+2584, 1/4: U+2582, empty: space.
    // Or use block elements: '█', '▓', '▒', '░'.
}
```

Each bar is scaled to the maximum observed phase duration so the longest bar
fills `bar_width` cells (computed as `inner_width - label_col - ms_col - 2`).
Phases with `None` timing are shown as `  (skipped)` in dim style.

**Keybindings**: None specific; tab switching only.

**Notes**

- `collection_pre_ms` and `collection_post_ms` are shown only if non-None.
  Standard single-test runs will not have collection scripts.
- If `phase_timings` is entirely `None` (old runner result), show fallback:
  `Phase timings not available (no PhaseTimings in result).`

---

### New dependencies (Phase 1)

| Crate        | Version   | Reason                                     |
| ------------ | --------- | ------------------------------------------ |
| `serde_json` | workspace | Pretty-print response body JSON in `ui.rs` |

No other new dependencies. The braille/block bar is implemented with plain
`std::fmt::Write` over Unicode character literals.

---

### Test strategy (Phase 1)

**Unit tests (new `src/tests/` or inline `#[cfg(test)]` in `app.rs`)**

- `DetailTab::next` cycles through all five variants and wraps around.
- `App::select_down` resets `detail_tab` to `Request` and zeroes
  `detail_scroll[..]`.
- `detect_body_format` returns `Json` for valid JSON and `Plain` otherwise.
- `braille_bar(0, 40)` returns all empty characters; `braille_bar(40, 40)`
  returns all full characters.

**Manual smoke tests**

- Run against a collection with failures; verify the Failures tab is
  auto-selected when a test finishes as failed.
- Scroll the response body with a large JSON payload; verify `d`/`u` are scoped
  to the Response tab and do not affect other tabs.

---

## Phase 2 — Collection and run controls

### Goal

Give users direct control over individual test runs, the ability to abort a
running collection, filter the visible list, navigate failures quickly, and
export results. Extend the file browser to support all collection formats.

---

### Feature 2.1 — Multi-format file browser

**Description**: The file browser currently only shows `.http` files. Extend it
to also show `.json` (Postman), `.bru` files and directories containing `.bru`
files (Bruno), `.yaml`/`.yml` (OpenAPI and OpenCollection), so users can open
any supported format without leaving the TUI.

**Data model (`app.rs`)**

Extend `BrowserEntry`:

```rust
pub enum BrowserEntry {
    ParentDir,
    Dir(String, PathBuf),
    HttpFile(String, PathBuf),
    // New variants:
    JsonFile(String, PathBuf),      // Postman
    BruFile(String, PathBuf),       // Bruno single file
    BruDir(String, PathBuf),        // Bruno collection directory (contains .bru)
    YamlFile(String, PathBuf),      // OpenAPI or OpenCollection
}
```

`FileBrowser::refresh` adds detection logic:

```rust
} else if ext == "json" {
    files.push(BrowserEntry::JsonFile(...));
} else if ext == "bru" {
    files.push(BrowserEntry::BruFile(...));
} else if ext == "yaml" || ext == "yml" {
    files.push(BrowserEntry::YamlFile(...));
}
// For directories, check if they contain any .bru files:
if path.is_dir() && contains_bru_files(&path) {
    dirs.push(BrowserEntry::BruDir(...));
} else if path.is_dir() {
    dirs.push(BrowserEntry::Dir(...));
}
```

`contains_bru_files(path)` does a single `read_dir` and returns early on the
first `.bru` entry — no recursion.

**UI (`ui.rs`)**

`render_browser`: each variant gets a distinct icon and colour:

| Variant    | Icon   | Colour       |
| ---------- | ------ | ------------ |
| `HttpFile` | `  `   | Default      |
| `JsonFile` | `  `   | Yellow       |
| `BruFile`  | `  `   | Magenta      |
| `BruDir`   | `▶ B ` | Magenta Bold |
| `YamlFile` | `  `   | Cyan         |

**Keybindings**: unchanged.

**Notes**

- `open_collection` in `main.rs` must detect format from file extension and call
  the appropriate adapter:

```rust
fn load_sources_any(path: &Path) -> Result<Sources, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "http" => /* existing load_sources */,
        "json" => /* PostmanAdapter */,
        "bru"  => /* BrunoAdapter single file */,
        "yaml" | "yml" => /* OpenApiAdapter or OpenCollectionAdapter — try both */,
        "" if path.is_dir() => /* existing dir scan + BrunoAdapter for dirs */,
        _ => Err(format!("unsupported format: {}", path.display())),
    }
}
```

The `hello_client` adapters (`PostmanAdapter`, `BrunoAdapter`, etc.) all return
`Vec<TestCase>` after calling their respective `into_test_cases()` methods.
`FolderSpec` names are derived from collection name / file stem.

---

### Feature 2.2 — Per-test rerun

**Description**: When a test is `Passed` or `Failed` (not `Running`), pressing
`Enter` on it reruns that single test case instead of toggling folder collapse.
Folder `Enter` keeps its expand/collapse behaviour.

**Data model (`app.rs`)**

Add to `App`:

```rust
pub single_test_pending: Option<usize>,  // test index waiting for a solo run
```

Add a new `RunnerEvent` variant in `event.rs`:

```rust
RunnerEvent::SingleTestFinished(usize, Box<TestResult>),
```

Or reuse `TestFinished` — the runner thread for a solo run still sends index
`0`, which `main.rs` maps back to the actual test index stored in
`single_test_pending`.

Cleaner approach: add `RunnerEvent::TestFinished` with an `absolute_index` field
that is already the final index in `App::tests`. The per-test runner thread is
given `absolute_index` and uses it as the event payload.

**UI (`ui.rs`)**

A test row in `Running` state during a solo rerun shows the `⋯` spinner as
usual. No overlay needed.

**Keybindings**

| Context                                       | Key     | Action                      |
| --------------------------------------------- | ------- | --------------------------- |
| Main view, cursor on `Passed`/`Failed` test   | `Enter` | Rerun that single test      |
| Main view, cursor on folder                   | `Enter` | Expand/collapse (unchanged) |
| Main view, cursor on `Pending`/`Running` test | `Enter` | No-op                       |

**Notes**

- In `main.rs`, `handle_main_key` checks:
  ```rust
  KeyCode::Enter => match app.tree.get(app.cursor).copied() {
      Some(TreeItem::Folder(_)) => app.toggle_folder(),
      Some(TreeItem::Test(ti)) => {
          let status = &app.tests[ti].status;
          if matches!(status, TestStatus::Passed | TestStatus::Failed) {
              // spawn single-test runner
          }
      },
      None => {},
  },
  ```
- The solo runner thread in `runner.rs` gets a single `TestCase` (re-parsed or
  stored) and sends events with the known absolute index. Add:
  ```rust
  pub fn spawn_single_runner(
      test_case: TestCase,
      absolute_index: usize,
      params: HashMap<String, String>,
      tx: mpsc::SyncSender<RunnerEvent>,
  )
  ```
- `TestCase` must be retrievable. Since `TestRow` only stores `TestResult`,
  store the original `TestCase` in `TestRow`:
  ```rust
  pub struct TestRow {
      pub name: String,
      pub status: TestStatus,
      pub result: Option<TestResult>,
      pub test_case: TestCase,   // new field; stored on construction
  }
  ```
  `App::new` and `App::apply_rerun` are updated to accept the flat
  `Vec<TestCase>` in addition to `Vec<FolderSpec>`, or the two are zipped.
- The per-test runner must be blocked from starting while `Phase::Running` is
  active (a collection run is in progress). Check
  `matches!(app.phase, Phase::Done | Phase::Error(_))` before spawning.

---

### Feature 2.3 — Stop runner

**Description**: Press `s` during a run to abort. The background thread receives
a cancellation signal and stops after the current in-flight test completes.

**Data model (`app.rs`)**

No new `App` fields.

Add to `runner.rs` a `CancelToken`:

```rust
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> (Self, CancelGuard) { ... }
    pub fn cancel(&self) { self.0.store(true, Ordering::Relaxed); }
}

pub struct CancelGuard(Arc<AtomicBool>);
impl CancelGuard {
    pub fn is_cancelled(&self) -> bool { self.0.load(Ordering::Relaxed); }
}
```

`spawn_runner` returns `(mpsc::Receiver<RunnerEvent>, CancelToken)` instead of
just the receiver.

Store the current token in `main.rs` as a local variable
`cancel_token: Option<CancelToken>`.

Add `RunnerEvent::Cancelled` so the TUI knows the run was aborted rather than
errored.

**UI (`ui.rs`)**

Header bar: when `Phase::Running`, show `[s]stop` in the keymap hint.
`Phase::Cancelled` (new variant) shows as `⊘ cancelled` with a dim style.
`App::phase`:

```rust
pub enum Phase {
    Running { current: usize },
    Done { elapsed_ms: u128 },
    Cancelled { elapsed_ms: u128 },
    Error(String),
}
```

**Keybindings**

| Key | Condition        | Action                                           |
| --- | ---------------- | ------------------------------------------------ |
| `s` | `Phase::Running` | Call `cancel_token.cancel()`; update header hint |

**Notes**

- The cancellation is cooperative: the runner thread checks
  `guard.is_cancelled()` between tests. The current in-flight test completes
  normally (it may already have network I/O in flight).
- After cancellation, tests that were `Pending` stay `Pending` — they are not
  marked failed.

---

### Feature 2.4 — Filter by status

**Description**: Press `f` to cycle through filter modes. Only matching tests
(and their parent folders) appear in the test tree.

**Data model (`app.rs`)**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum FilterMode {
    #[default]
    All,
    Failed,
    Passed,
    Pending,
}

impl FilterMode {
    pub fn next(self) -> Self { /* All → Failed → Passed → Pending → All */ }
    pub fn label(self) -> &'static str { /* "All" | "Failed" | ... */ }
}
```

Add to `App`:

```rust
pub filter: FilterMode,
```

`App::rebuild_tree` applies the filter: a test is included if
`filter == FilterMode::All` or if the test's `TestStatus` matches the filter. A
folder is included if it has at least one visible test.

**UI (`ui.rs`)**

Status bar (left side): append `  [f:Failed]` when a filter is active, using
`Color::Yellow`.

Header keymap hint: add `[f]filter`.

**Keybindings**

| Key | Action                                                               |
| --- | -------------------------------------------------------------------- |
| `f` | `app.filter = app.filter.next(); app.rebuild_tree(); app.cursor = 0` |

**Notes**

- `rebuild_tree` is already called after rerun and folder toggle; the filter
  logic is cleanly added there.
- When the filter removes the currently-selected test from the tree, reset
  `app.cursor = 0`.

---

### Feature 2.5 — Jump to next/previous failure

**Description**: `n` jumps the cursor to the next `Failed` test in tree order;
`N` (`Shift+n`) jumps to the previous.

**Data model (`app.rs`)**

Add helper methods:

```rust
impl App {
    pub fn next_failure(&mut self) { /* search tree forward from cursor+1 */ }
    pub fn prev_failure(&mut self) { /* search tree backward from cursor-1 */ }
}
```

Both methods wrap around and do nothing if there are no failed tests.

**UI**: No changes. The cursor moves, which highlights the test as usual.

**Keybindings**

| Key             | Action               |
| --------------- | -------------------- |
| `n`             | `app.next_failure()` |
| `N` (`Shift+n`) | `app.prev_failure()` |

**Notes**

- `KeyCode::Char('N')` with `KeyModifiers::SHIFT` is the crossterm form.
  However, on most terminals the terminal emulator sends lowercase `n` with
  `SHIFT` modifier, so match `KeyCode::Char('N')` only (capital letter from
  keyboard with Shift held).

---

### Feature 2.6 — Config file load (`-c` flag)

**Description**: Support `-c config.toml` (or `-c config.json`) in
`parse_args()` to set `base_url`, `timeout`, `verbose`, and initial `params`.

**Data model (`app.rs`)**

No changes.

**`main.rs` changes**

`parse_args` returns `(path, params, debug_log, config_path)`. A new
`load_config(path)` function reads TOML or JSON and merges:

```rust
pub struct TuiConfig {
    pub base_url: Option<String>,
    pub timeout_secs: Option<u64>,
    pub verbose: bool,
    pub params: HashMap<String, String>,
}
```

CLI `--param` values override config file values (config is loaded first, then
params are merged on top).

**UI**: None.

**Keybindings**: None.

**Notes**

- Config format detection: if the file ends in `.json`, use `serde_json`;
  otherwise use `toml`. Add `toml` crate to `hello_tui/Cargo.toml`.
- Config loading errors print a warning to `app.debug` and continue; they do not
  abort startup.

---

### Feature 2.7 — Export results (`x` key)

**Description**: Press `x` after a run completes to write a JSON summary of all
test results to `./results.json`.

**Data model (`app.rs`)**

No new fields. Serialise from `App::tests`.

Serialised shape:

```json
{
  "title": "my-collection",
  "elapsed_ms": 312,
  "passed": 4,
  "failed": 1,
  "tests": [
    {
      "name": "GET /users",
      "status": "passed",
      "response_status": 200,
      "response_time_ms": 141,
      "failures": []
    }
  ]
}
```

**UI (`ui.rs`)**

Status bar: after a successful export show a transient `Saved → results.json`
message for one render cycle. Implement with a new field:

```rust
pub status_message: Option<String>,   // cleared after next tick
```

**Keybindings**

| Key | Condition                           | Action                           |
| --- | ----------------------------------- | -------------------------------- |
| `x` | `Phase::Done` or `Phase::Cancelled` | Write JSON, set `status_message` |

**Notes**

- `serde_json::to_string_pretty` is used; no new dependencies beyond what Phase
  1 adds.
- If `./results.json` already exists it is overwritten without prompting.
- Write errors are shown in `status_message` as `Error: <msg>`.

---

### New dependencies (Phase 2)

| Crate  | Version | Reason                             |
| ------ | ------- | ---------------------------------- |
| `toml` | `0.8`   | Parse `-c config.toml` config file |

---

### Test strategy (Phase 2)

**Unit tests**

- `FilterMode::next` cycles all four variants and wraps to `All`.
- `App::rebuild_tree` with `FilterMode::Failed` hides `Pending` and `Passed`
  tests and includes only folders with at least one failed child.
- `App::next_failure` / `App::prev_failure` on a list with two failed tests at
  positions 2 and 5 advances correctly and wraps.
- `CancelToken` / `CancelGuard`: set cancel, check `is_cancelled()` returns
  true.
- `load_config` parses a TOML fixture with `base_url` and one param entry.

**Integration / smoke tests**

- Start a mock collection; press `s` mid-run; verify `Phase::Cancelled` and that
  pending tests remain `Pending`.
- Filter to `Failed` with one failing test; verify only that test and its folder
  appear in the tree.
- `x` with a completed run; verify `results.json` exists and parses as valid
  JSON with correct counts.

---

## Phase 3 — Run history and diff

### Goal

Persist a rolling history of run results in memory (and optionally on disk),
display a history overlay, show per-test trend indicators in the list, and open
a diff view comparing the current response body with the previous run.

---

### Feature 3.1 — History store

**Description**: After each full collection run, `App` snapshots the results
into a `VecDeque<RunSnapshot>` capped at `history_max` (default 5) entries. This
is an in-memory store; disk persistence is an opt-in extension (see Feature
3.5).

**Data model (`app.rs`)**

```rust
pub struct TestSnapshot {
    pub name: String,
    pub passed: bool,
    pub status_code: Option<u16>,
    pub response_time_ms: Option<u64>,
}

pub struct RunSnapshot {
    pub timestamp: std::time::SystemTime,
    pub total_ms: u128,
    pub passed: usize,
    pub failed: usize,
    pub tests: Vec<TestSnapshot>,   // same order as the collection
}
```

Add to `App`:

```rust
pub history: std::collections::VecDeque<RunSnapshot>,
pub history_max: usize,   // default 5
```

`App::on_done` calls `self.snapshot_run(elapsed_ms)` which constructs a
`RunSnapshot` from the current `self.tests` state and pushes it to
`self.history`, popping the front if `len > history_max`.

```rust
impl App {
    fn snapshot_run(&mut self, elapsed_ms: u128) { ... }
}
```

**UI**: No changes in this feature — the data is consumed by Features 3.2–3.4.

**Keybindings**: None.

**Notes**

- `apply_rerun` does not clear `history`; history accumulates across reruns and
  browser-opened collections as long as the process lives.
- `RunSnapshot::timestamp` uses `SystemTime::now()` — no `chrono` dependency
  needed.

---

### Feature 3.2 — History overlay

**Description**: Press `H` to open a full-width overlay showing a table of past
runs. Each row shows: run number, formatted timestamp, passed/failed counts,
total time.

**Data model (`app.rs`)**

```rust
pub show_history: bool,
pub history_cursor: usize,
```

**UI (`ui.rs`)**

`render_history_overlay`:

```
┌─ Run History ────────────────────────────────────────────────────────┐
│  #   Date & Time             Passed  Failed  Total ms                │
│  1   2026-05-25 14:32:10      5        0      312 ms    (most recent) │
│  2   2026-05-25 14:28:44      4        1      298 ms                  │
│  3   2026-05-25 14:20:01      5        0      305 ms                  │
│                                                                       │
│  [j/k] navigate  [Esc] close  [Enter] jump to diff for selected run   │
└───────────────────────────────────────────────────────────────────────┘
```

Rendered with `centered_rect(80, 60, f.area())`. History list rendered with
`ListState` (stateful widget).

Timestamp formatting: `format_system_time(t: SystemTime) -> String` using
`std::time::UNIX_EPOCH` arithmetic — no `chrono` dep; output format
`YYYY-MM-DD HH:MM:SS` derived from `Duration::as_secs()` decomposition.

**Keybindings**

| Key     | Context         | Action                                            |
| ------- | --------------- | ------------------------------------------------- |
| `H`     | Main view       | `app.show_history = true; app.history_cursor = 0` |
| `j`/`↓` | History overlay | `app.history_cursor++`                            |
| `k`/`↑` | History overlay | `app.history_cursor--`                            |
| `Esc`   | History overlay | `app.show_history = false`                        |
| `Enter` | History overlay | Open diff for selected run (Feature 3.3)          |

---

### Feature 3.3 — Diff mode

**Description**: Press `D` on a test in the main view (or `Enter` in the history
overlay) to open a side-by-side diff overlay comparing the current response body
with the most recent (or history-selected) run's body.

**Data model (`app.rs`)**

```rust
pub show_diff: bool,
pub diff_scroll: u16,
pub diff_content: Vec<DiffLine>,   // pre-computed on open

pub enum DiffLine {
    Equal(String),
    Removed(String),
    Added(String),
    Header(String),   // @@ ... @@ hunk header
}
```

`diff_content` is computed once when the overlay opens; it is not recomputed on
scroll. Computing it involves:

1. Get `current_body: &str` from `app.tests[ti].result?.response?.body`.
2. Get `prev_body: &str` from the most recent `RunSnapshot` (or the
   history-cursor-selected snapshot).
3. Call `compute_diff(current_body, prev_body) -> Vec<DiffLine>`.

**Diff algorithm (`diff.rs`)**

Add a new source file `src/diff.rs` with a simple Myers diff over line vectors:

```rust
pub fn compute_diff(old: &str, new: &str) -> Vec<DiffLine>;
```

If both bodies parse as JSON, normalise via `serde_json::to_string_pretty`
before diffing so key-order differences do not appear as changes.

Hunk context: show 3 lines of context around each changed hunk (standard unified
diff behaviour).

For large bodies (> 500 lines each), fall back to a `similar` crate diff for
performance. Add `similar` as a new dependency only if the plain Myers
implementation is too slow in practice.

**UI (`ui.rs`)**

`render_diff_overlay`:

```
┌─ Diff: GET /users  (current vs run #2) ──────────────────────────────┐
│  @@ -1,5 +1,5 @@                                                      │
│    {                                                                   │
│  -   "count": 4,                  (red, "- " prefix)                  │
│  +   "count": 5,                  (green, "+ " prefix)                │
│      "users": [                                                        │
│        ...                                                             │
│  [d/u] scroll  [Esc] close                                             │
└────────────────────────────────────────────────────────────────────────┘
```

Full-screen overlay: `centered_rect(90, 85, f.area())`. Each `DiffLine` variant
gets a colour:

| Variant   | Style                       |
| --------- | --------------------------- |
| `Equal`   | Default                     |
| `Removed` | `Color::Red`, prefix `- `   |
| `Added`   | `Color::Green`, prefix `+ ` |
| `Header`  | `Color::Cyan`               |

**Keybindings**

| Key     | Context                               | Action                            |
| ------- | ------------------------------------- | --------------------------------- |
| `D`     | Main view, cursor on test with result | Open diff vs latest history entry |
| `d`/`u` | Diff overlay                          | Scroll diff                       |
| `Esc`   | Diff overlay                          | `app.show_diff = false`           |

**Notes**

- If `app.history.is_empty()` when `D` is pressed, show an informational
  overlay: `No previous run available.`
- The diff works on string bodies; binary responses (detected by presence of
  null bytes) show `Binary content — diff not available.`

---

### Feature 3.4 — Trend indicator in test list

**Description**: After a run completes, the test list shows a small trend arrow
next to each test's response time comparing with the previous run: `↑` (slower),
`↓` (faster), `─` (same ±10%), or empty for new tests.

**Data model (`app.rs`)**

Add to `TestRow`:

```rust
pub trend: TrendDirection,

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum TrendDirection {
    #[default]
    None,
    Faster,
    Slower,
    Stable,
}
```

`App::on_done` computes trends after `snapshot_run`:

```rust
fn compute_trends(&mut self) {
    if let Some(prev) = self.history.get(1) {  // index 1 = second-most-recent
        for (i, test) in self.tests.iter_mut().enumerate() {
            let prev_ms = prev.tests.get(i)?.response_time_ms?;
            let curr_ms = test.response_time_ms()?;
            test.trend = classify_trend(prev_ms, curr_ms);
        }
    }
}
```

(The most-recent snapshot at index 0 was just pushed by `snapshot_run`; the
previous run is at index 1.)

`classify_trend(prev: u64, curr: u64) -> TrendDirection`: delta within ±10% →
`Stable`; curr > prev by > 10% → `Slower`; curr < prev by > 10% → `Faster`.

**UI (`ui.rs`)**

In `render_list`, after the response-time span add a trend span:

```rust
let trend_span = match t.trend {
    TrendDirection::Faster => Span::styled(" ↓", Style::new().fg(Color::Green)),
    TrendDirection::Slower => Span::styled(" ↑", Style::new().fg(Color::Red)),
    TrendDirection::Stable => Span::styled(" ─", Style::new().fg(Color::DarkGray)),
    TrendDirection::None   => Span::raw(""),
};
```

**Keybindings**: None.

---

### Feature 3.5 — Persist history to disk (optional)

**Description**: If `--history-file <path>` is passed (or configured via `-c`
config), history is loaded on startup and written after each run.

**Data model**

Serialise `VecDeque<RunSnapshot>` as a JSON array via `serde` derives on
`RunSnapshot` and `TestSnapshot`. Add `#[derive(Serialize, Deserialize)]` to
both structs.

**`main.rs` changes**

- `parse_args` recognises `--history-file <path>`.
- `App::new` loads the file if provided: deserialise JSON → prepend to
  `self.history`.
- `App::on_done` (after `snapshot_run`) writes `self.history` to disk if
  `history_file` is `Some`.

Store `history_file: Option<PathBuf>` in `App`.

**Keybindings**: None.

**Notes**

- Default path `~/.hello_tui/history.json` is created on first write (create
  directory if absent via `std::fs::create_dir_all`). Only used when
  `--history-file` is not given but user has the default path set in their
  config file.
- Write errors are logged to `app.debug` and ignored.

---

### New dependencies (Phase 3)

| Crate     | Version   | Reason                                                                            |
| --------- | --------- | --------------------------------------------------------------------------------- |
| `serde`   | workspace | Derive `Serialize`/`Deserialize` on `RunSnapshot`, `TestSnapshot`                 |
| `similar` | `2`       | Optional fallback diff for large bodies (add only if plain Myers is insufficient) |

`serde` is already a workspace dep; adding it to `hello_tui/Cargo.toml` is the
only change for the base case.

---

### Test strategy (Phase 3)

**Unit tests (`src/diff.rs`)**

- `compute_diff("a\nb\nc", "a\nX\nc")` → one `Removed("b")`, one `Added("X")`,
  two `Equal` context lines.
- JSON normalisation: `compute_diff(r#"{"b":1,"a":2}"#, r#"{"a":2,"b":1}"#)` →
  all `Equal` (key-order independent).
- Empty old body vs non-empty new body → all `Added` lines.
- Both bodies identical → all `Equal` lines.

**Unit tests (`app.rs`)**

- `App::snapshot_run` after two test results → `history.len() == 1`;
  `snapshot.passed == 1`, `snapshot.failed == 1`.
- `history_max = 2`: after three runs, `history.len() == 2` and oldest is
  discarded.
- `compute_trends` with previous run slower → `TrendDirection::Slower`.
- `classify_trend(100, 109)` → `Stable` (within 10%).

**Unit tests (history serialisation)**

- Round-trip: serialise a `RunSnapshot`, deserialise, assert field equality.

---

## Phase 4 — Request editor and advanced views

### Goal

Provide power-user features: an in-TUI editor for `.http` files, a DAG-based
flow view for `FlowDef` collections, a mock-server status panel, and a dual-
pane layout for comparing two collections side by side.

---

### Feature 4.1 — Inline request editor

**Description**: Press `E` (capital E, i.e., `Shift+e`) on a test to open a
full-screen editor for the source `.http` file containing that test. The editor
is a minimal line-based text editor with normal and insert modes. `:w` saves and
reruns the test; `Esc` from normal mode closes without saving.

**Data model (`app.rs`)**

```rust
pub show_editor: bool,
pub editor: Option<FileEditor>,

pub struct FileEditor {
    pub path: PathBuf,
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub mode: EditorMode,
    pub dirty: bool,
    pub command_buf: String,   // accumulates ":" commands
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditorMode {
    Normal,
    Insert,
    Command,  // ":" command mode
}
```

To map a `TestRow` back to its source file path, store
`source_path: Option<PathBuf>` in `TestRow` (populated from the `Sources` entry
when constructing `TestRow` in `App::new`).

`App::new` receives the flat `Vec<TestCase>` alongside `Vec<FolderSpec>` (as
planned in Phase 2). Each `TestCase` in `hello_core::types::TestCase` does not
carry the source path. The source path comes from the `Sources` vec in
`main.rs`; align test-to-source by index using the folder structure.

Simpler: pass a parallel `Vec<PathBuf>` (one per `TestRow`) into `App::new`.

**UI (`ui.rs`)**

`render_editor`:

```
┌─ NORMAL  src/requests.http ─────────────────────────────────── :w save :q quit ┐
│  1  ### Login                                                                   │
│  2  ### @param base_url https://api.example.com                                 │
│  3                                                                               │
│  4  POST {{base_url}}/auth/login                         ← cursor line          │
│  5  Content-Type: application/json                                               │
│  6                                                                               │
│  7  {"username": "alice", "password": "s3cr3t"}                                 │
│ ...                                                                              │
│                                                                                  │
│  -- INSERT --   Col 12                                                           │
└──────────────────────────────────────────────────────────────────────────────────┘
```

Full-screen overlay via `f.area()` directly. Line numbers in the left gutter
(`Constraint::Length(5)`). Cursor line highlighted. Mode indicator in the title.
Status line at the bottom shows mode name, column, and dirty indicator `[+]`.

**Keybindings (editor)**

| Mode      | Key             | Action                                       |
| --------- | --------------- | -------------------------------------------- |
| Normal    | `i`             | Enter Insert mode                            |
| Normal    | `Esc`           | Close editor (if not dirty); prompt if dirty |
| Normal    | `:`             | Enter Command mode                           |
| Normal    | `j`/`k`/`↑`/`↓` | Move cursor row                              |
| Normal    | `h`/`l`/`←`/`→` | Move cursor col                              |
| Normal    | `0`             | Go to line start                             |
| Normal    | `$`             | Go to line end                               |
| Normal    | `G`             | Go to last line                              |
| Normal    | `gg`            | Go to first line (two-key sequence)          |
| Insert    | `Esc`           | Return to Normal mode                        |
| Insert    | Printable chars | Insert at cursor                             |
| Insert    | `Backspace`     | Delete char left                             |
| Insert    | `Enter`         | Split line                                   |
| Command   | `:w` → `Enter`  | Save file and rerun                          |
| Command   | `:q` → `Enter`  | Quit editor (prompt if dirty)                |
| Command   | `:wq` → `Enter` | Save and quit                                |
| Command   | `Esc`           | Cancel command                               |
| Main view | `E` (Shift+e)   | Open editor on test's source file            |

**Notes**

- `FileEditor::save` writes `self.lines.join("\n")` to `self.path`.
- After save, `main.rs` calls `rerun(app, sources)` to re-parse and re-run.
- The `gg` two-key sequence is tracked via `command_buf`; first `g` is stored,
  second `g` executes. Any other key after first `g` clears `command_buf`.
- The editor handles only ASCII + UTF-8 single-codepoint characters. Multi- byte
  editing works correctly because `lines[row]` is a `String`; cursor column is
  tracked as a `char` index via `chars().count()`.
- Syntax highlighting is out of scope; all text rendered in default style except
  the current cursor position (inverted).

---

### Feature 4.2 — Flow view

**Description**: Press `F` to switch to a top-level Flow view mode when the
loaded collection is a `.flow` file. The flow is rendered as a directed acyclic
graph using box-drawing characters. Navigation moves between nodes.

**Data model (`app.rs`)**

```rust
pub mode: AppMode,

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum AppMode {
    #[default]
    Tests,
    Flow,
    Editor,   // inline request editor from Feature 4.1
}

pub flow_def: Option<hello_client::FlowDef>,
pub flow_cursor: usize,   // index into FlowDef::nodes
```

`FlowDef` is available from `hello_client` (re-exported from
`hello_client::flow`). The flow is parsed via
`hello_client::flow_runner::parse_flow` when a `.flow` file is opened.

**UI (`ui.rs`)**

`render_flow` replaces the test list + detail panel area entirely when
`app.mode == AppMode::Flow`. It renders a text-based DAG:

```
┌─ Flow: User Onboarding ──────────────────────────────────────────────┐
│                                                                       │
│   [start]                                                             │
│      │                                                                │
│   ┌──┴──────────────┐                                                 │
│   │ register        │  ← cursor (highlighted)                        │
│   │ auth/register   │                                                 │
│   └──┬──────────────┘                                                 │
│      │                                                                │
│   ┌──┴──────────────┐  ┌─────────────────┐                           │
│   │ send_email      │  │ create_profile  │  (parallel group)          │
│   └──┬──────────────┘  └─────────────────┘                           │
│      └──────┬──────────────────┘                                      │
│   ┌──┴──────────────┐                                                 │
│   │ finalize        │                                                 │
│   └─────────────────┘                                                 │
│                                                                       │
│   [j/k] navigate  [Enter] jump to test  [F/Esc] back to test list    │
└───────────────────────────────────────────────────────────────────────┘
```

Layout algorithm: topological sort of `FlowDef::nodes`, assign each to a row.
Parallel groups are placed on the same row with horizontal spacing. Draw box-
drawing characters (`┌`, `┐`, `└`, `┘`, `─`, `│`) using ratatui `Canvas` or
manual `Paragraph` row construction.

Use manual `Paragraph` approach: pre-render the DAG to a `Vec<String>` (a
character grid) and display via `Paragraph::new`. This avoids the complexity of
`Canvas`.

**Keybindings (Flow view)**

| Key     | Action                                            |
| ------- | ------------------------------------------------- |
| `F`     | Toggle Flow view / Tests view (`app.mode` cycles) |
| `Esc`   | Return to Tests view                              |
| `j`/`k` | Move `flow_cursor` through `FlowDef::nodes`       |
| `Enter` | Jump to the corresponding test in the Tests view  |

**Notes**

- The flow cursor highlights the box for the selected node.
- `Enter` sets `app.mode = AppMode::Tests` and positions `app.cursor` on the
  matching test name. The match is by test name (exact string match against
  `FlowNode::step.file` stem).
- If `app.flow_def.is_none()`, pressing `F` shows a brief status message:
  `No flow definition loaded.`

---

### Feature 4.3 — Mock server panel

**Description**: Toggle `M` to show a right-side panel with `hello_server`
status when a mock server is running in the same workspace. The panel shows
active routes, hit counts, and recent requests.

**Data model (`app.rs`)**

```rust
pub show_server_panel: bool,
pub server_panel: ServerPanel,

pub struct ServerPanel {
    pub port: u16,
    pub routes: Vec<ServerRoute>,
    pub recent_requests: VecDeque<ServerRequest>,
}

pub struct ServerRoute {
    pub method: String,
    pub path: String,
    pub hit_count: u64,
}

pub struct ServerRequest {
    pub timestamp_secs: u64,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub latency_ms: u64,
}
```

Communication with `hello_server`: `hello_server` exposes an internal HTTP stats
endpoint on `localhost:{admin_port}/stats` (a new endpoint to be added to
`hello_server`). `hello_tui` polls this endpoint every 2 seconds from a
background thread using a plain `std::net::TcpStream` + minimal HTTP/1.1 GET to
avoid pulling in `reqwest` for TUI-internal use.

Polling thread sends `RunnerEvent::ServerStats(Box<ServerPanel>)` to the main
event loop.

```rust
// New variant in event.rs:
RunnerEvent::ServerStats(Box<ServerPanel>),
```

Spawned only when `--server-port <port>` is passed on the CLI.

**UI (`ui.rs`)**

When `show_server_panel` is true, the outer layout splits the horizontal space:

```rust
let [list_area, server_area] = Layout::horizontal([
    Constraint::Fill(1),
    Constraint::Length(40),
]).areas(area);
```

`render_server_panel` in `server_area`:

```
┌─ Mock Server :8080 ─────────────────┐
│ Routes                               │
│  GET  /users           42 hits       │
│  POST /users            8 hits       │
│  GET  /users/{id}      17 hits       │
│                                      │
│ Recent requests                      │
│  14:32:10  GET /users      200  3ms  │
│  14:32:08  POST /users     201 12ms  │
│                                      │
└──────────────────────────────────────┘
```

**Keybindings**

| Key | Action                         |
| --- | ------------------------------ |
| `M` | Toggle `app.show_server_panel` |

**Notes**

- The `hello_server` stats endpoint is a minimal extension: a new route
  `GET /~stats` (prefixed with `~` to avoid colliding with user routes) that
  returns JSON of the form:
  ```json
  {
    "routes": [{ "method": "GET", "path": "/users", "hits": 42 }],
    "recent": [
      {
        "ts": 1748102530,
        "method": "GET",
        "path": "/users",
        "status": 200,
        "ms": 3
      }
    ]
  }
  ```
- This requires a small addition to `hello_server/src/main.rs` (currently empty
  — the server is planned but not yet implemented).
- If the polling request fails (server not running), the panel shows
  `Server not reachable.` and continues polling.

---

### Feature 4.4 — Multi-pane layout

**Description**: Press `|` to split the test list vertically into two panes,
each showing a different collection. `Tab` switches focus between panes. The
focused pane receives all key events; the unfocused pane shows its last state.

**Data model (`app.rs`)**

```rust
pub pane: PaneLayout,

pub enum PaneLayout {
    Single,
    Dual {
        right: Box<App>,    // second App instance
        focus: PaneFocus,
    },
}

pub enum PaneFocus { Left, Right }
```

Nesting a full `Box<App>` avoids duplicating the state machine. The `right`
`App` is driven by the same background runner infrastructure: it has its own
`mpsc::Receiver<RunnerEvent>` stored in `main.rs` as `Option<rx_right>`.

`main.rs` maintains:

```rust
let mut rx_left: mpsc::Receiver<RunnerEvent> = ...;
let mut rx_right: Option<mpsc::Receiver<RunnerEvent>> = None;
```

Key events are dispatched to `app` (left) or `app.pane.right()` depending on
`focus`.

**UI (`ui.rs`)**

```rust
fn render(f: &mut Frame, app: &App) {
    match &app.pane {
        PaneLayout::Single => render_single(f, app, f.area()),
        PaneLayout::Dual { right, focus } => {
            let [left_area, right_area] = Layout::horizontal([
                Constraint::Fill(1),
                Constraint::Fill(1),
            ]).areas(f.area());
            render_single(f, app, left_area);
            render_single(f, right, right_area);
            // Draw focus indicator border
        },
    }
}
```

`render_single(f, app, area)` is the existing `render` function extracted to
take an explicit `area`.

A focus indicator: the focused pane's `Tests` block border uses
`Style::new().fg(Color::Yellow)`; the unfocused pane uses `Color::DarkGray`.

**Keybindings**

| Key         | Action                                                  |
| ----------- | ------------------------------------------------------- | -------------------------------------------------------------- |
| `           | `                                                       | Toggle dual-pane; opens file browser to pick second collection |
| `Tab`       | Switch focus between Left / Right pane                  |
| `q` / `Esc` | Quit if Left focused; close right pane if Right focused |

**Notes**

- `Tab` conflicts with the detail-panel tab cycling from Phase 1. Resolution:
  `Tab` switches pane focus only when dual-pane is active; otherwise it cycles
  detail tabs.
- Opening the second collection reuses the file browser (`app.open_browser`),
  but the selected path spawns a second `spawn_runner` and creates the `right`
  `App`.
- The second `App` uses the same `params` as the primary `App` by default
  (copied at split time).
- Memory implication: two `App` instances means doubled `Vec<TestRow>`. This is
  acceptable for typical collection sizes (< 500 tests).

---

### New dependencies (Phase 4)

| Crate    | Version | Reason                                       |
| -------- | ------- | -------------------------------------------- |
| None new | —       | All functionality uses existing deps + `std` |

The mock server panel polling uses `std::net::TcpStream` for minimal HTTP/1.1
without adding `reqwest` to `hello_tui`. The flow DAG rendering uses plain
string manipulation. The request editor uses only `crossterm` + `ratatui`.

If `similar` was not added in Phase 3, no additional dependencies are needed.

---

### Test strategy (Phase 4)

**Unit tests — `FileEditor`**

- `FileEditor::insert_char` at column 0 of a non-empty line → char prepended.
- `FileEditor::split_line` at mid-line → two correct lines.
- `FileEditor::save` writes `lines.join("\n")` to a temp file.
- Two-key `gg` sequence: first `g` sets `command_buf = "g"`; second `g` resets
  `cursor_row = 0`.

**Unit tests — flow DAG layout**

- `render_flow_to_grid` on a two-step sequential `FlowDef` → grid contains both
  step names.
- Parallel group → both node names appear on the same row index.

**Unit tests — dual-pane**

- `PaneLayout::Dual { focus: Left }` → key dispatch goes to left `App`.
- `Tab` key with `Dual` active → toggles `focus`.
- `Tab` key with `Single` → cycles `detail_tab` (Phase 1 behaviour preserved).

**Integration / smoke tests**

- Open a `.flow` file; press `F`; verify Flow view renders without panic.
- Open editor on a test; make a change; press `:w`; verify file on disk is
  updated and the runner restarts.
- Open dual pane; load two different collections; verify each list updates
  independently.

---

## Architecture notes

### Overlay rendering order

Overlays are rendered in this priority order (highest = rendered last = on top):

1. `render_browser` (if `show_browser`)
2. `render_param_editor` (if `show_params`)
3. `render_history_overlay` (if `show_history`) — Phase 3
4. `render_diff_overlay` (if `show_diff`) — Phase 3
5. `render_editor` (if `show_editor`) — Phase 4
6. `render_debug_overlay` (if `show_debug`)

Only one overlay should be open at a time. `main.rs` enforces mutual exclusion:
opening any overlay closes all others.

### Key dispatch priority

`main.rs` checks overlays in `run_tui` in this order:

```rust
if app.show_editor    { handle_editor_key(...)    }
else if app.show_diff      { handle_diff_key(...)      }
else if app.show_history   { handle_history_key(...)   }
else if app.show_browser   { handle_browser_key(...)   }
else if app.show_params    { handle_param_editor_key(...) }
else if app.mode == AppMode::Flow { handle_flow_key(...) }
else                           { handle_main_key(...)   }
```

### `App::rebuild_tree` is the single tree source

All filtering (Phase 2), dual-pane (Phase 4), and rerun (all phases) go through
`rebuild_tree`. No caller should mutate `app.tree` directly.

### `detail_scroll` migration (Phase 1)

`detail_scroll: u16` becomes `detail_scroll: [u16; 5]`. All existing call sites
(`scroll_detail`, `select_up`, `select_down`, `apply_rerun`) must be updated.
The migration is mechanical and contained to `app.rs` and one call site in
`main.rs`.

### RunnerEvent extensibility

`RunnerEvent` grows with each phase:

```rust
pub enum RunnerEvent {
    // Phase 0 (existing)
    TestStarted(usize),
    TestFinished(usize, Box<TestResult>),
    Done { elapsed_ms: u128 },
    Error(String),
    // Phase 2
    Cancelled,
    // Phase 4
    ServerStats(Box<ServerPanel>),
}
```

All `match ev` arms in `main.rs` must be updated as variants are added. Use
`#[non_exhaustive]` on the enum to get compile-time enforcement.

### V8 / LocalSet constraint

Every `spawn_runner` and `spawn_single_runner` call follows the same pattern
established in the existing `runner.rs`:

- Spawn an OS thread.
- Build a `tokio::runtime::Builder::new_current_thread()` runtime.
- Run a `LocalSet` inside `block_on`.

This guarantees `!Send` V8 handles stay on the same OS thread. No async is
introduced in the TUI thread. The `CancelToken` from Phase 2 uses
`Arc<AtomicBool>` which is `Send + Sync` and safe to share across the thread
boundary.

### Crate boundary

`hello_tui` depends on `hello_client` only. It must not depend on
`hello_sandbox` directly. All sandbox types used in `TestResult` (`RunMetrics`,
`SandboxEvent`) are re-exported through `hello_client`. If new types from
`hello_sandbox` are needed in the TUI, add re-exports to
`hello_client::http_runner` first.

### Cargo feature flags

No feature flags are planned. All Phase 1–4 features are compiled in
unconditionally to avoid conditional compilation complexity in the TUI. The
`similar` crate (Phase 3 optional) is added as a regular dependency when
adopted; it is small (no native deps) and does not meaningfully affect compile
time.
