# Runner Bridge

`src/runner.rs` bridges the `.http` file parser and the `HttpTestRunner`. It
converts parsed `RequestEntry` values into `TestCase` values and drives a
complete collection run.

## Public API

### `parse_collection`

```rust
pub fn parse_collection(
    content: &str,
    params: &HashMap<String, String>,
    base_dir: &Path,
) -> Result<Vec<TestCase>, String>
```

Parses `.http` content and converts each entry to a `TestCase` using
`entry_to_test_case`. Returns an error string on parse failure.

### `run_collection_from_str`

```rust
pub async fn run_collection_from_str(
    content: &str,
    params: &HashMap<String, String>,
    base_dir: &Path,
) -> Result<CollectionResult, String>
```

Parses `.http` content from a string and runs it with default `RunOpts`.
Automatically extracts `scheme://host` prefixes from all resolved request URLs
and passes them to `HttpTestRunner::builder().allowed_prefixes(...)`.

### `run_collection_from_str_with_opts`

```rust
pub async fn run_collection_from_str_with_opts(
    content: &str,
    params: &HashMap<String, String>,
    base_dir: &Path,
    opts: RunOpts<'_>,
) -> Result<CollectionResult, String>
```

Like `run_collection_from_str` but accepts `RunOpts` for extra control over the
run.

### `RunOpts`

```rust
pub struct RunOpts<'a> {
    /// Skip HTTP fetch and load the response from this path instead.
    pub response_file: Option<&'a str>,
    /// Collection pre-script path (overrides `### @collection-pre-script`).
    pub collection_pre_script: Option<&'a str>,
    /// Collection post-script path (overrides `### @collection-post-script`).
    pub collection_post_script: Option<&'a str>,
}
```

`RunOpts::default()` has all fields set to `None`.

### `run_file`

```rust
pub async fn run_file(
    path: &Path,
    params: &HashMap<String, String>,
    opts: RunOpts<'_>,
) -> Result<CollectionResult, String>
```

Reads a `.http` file from disk and delegates to
`run_collection_from_str_with_opts`. `base_dir` is derived from `path.parent()`.

### `entry_to_test_case`

```rust
pub fn entry_to_test_case(
    entry: RequestEntry<'_>,
    params: &HashMap<String, String>,
    base_dir: &Path,
) -> Result<TestCase, String>
```

Converts a single `RequestEntry` (from the parser) into a `TestCase` (for the
runner). Merges metadata `@param` values with the provided `params` map.
Resolves body file references, builds the multipart wire format, and
auto-injects `Content-Type` when not explicitly set.

---

## Internal Helpers

### `resolve_url`

Substitutes `{{variable}}` placeholders in a `Url::Segments` using the merged
params map. Falls back to `get_verbatim_endpoint()` (leaves placeholders intact)
if resolution fails.

### `url_prefix`

Extracts the `scheme://host` portion of a URL string. Used to build the
automatic HTTP allowlist.

```rust
url_prefix("https://api.example.com/v1/users") // → "https://api.example.com"
```

### `load_script_with_deps`

```rust
pub fn load_script_with_deps(
    script: &Script<'_>,
    base_dir: &Path,
) -> Result<(String, Vec<(String, String)>), String>
```

Reads a `Script` value and resolves its `import` dependencies:

- `Script::Inline(src)` → returns `src` + any modules scanned from it
- `Script::File(path)` → reads the file from disk, then recursively scans
  imports

Returns `(source, modules)` where `modules` is a list of `(specifier, source)`
pairs for all resolved `import "..."` statements.

### `scan_sandbox_imports`

```rust
pub fn scan_sandbox_imports(source: &str) -> std::collections::HashSet<String>
```

Scans a script source for `import ... from "..."` statements whose specifier
starts with `./` or `../`. Returns the set of relative path specifiers found.
Used by `load_script_with_deps` to auto-load local module dependencies.

### `detect_content_type` (internal)

Inspects `Option<Body>` and returns a `Content-Type` string when the body type
is unambiguous:

| Body                                 | Auto-detected Content-Type                 |
| ------------------------------------ | ------------------------------------------ |
| `Body::Multipart { boundary }`       | `multipart/form-data; boundary=<boundary>` |
| `Body::Raw` starting with `{` or `[` | `application/json`                         |
| `Body::Raw` starting with `<`        | `application/xml`                          |
| `Body::File("*.json")`               | `application/json`                         |
| `Body::File("*.xml")`                | `application/xml`                          |
| `Body::File("*.graphql"/"*.gql")`    | `application/json`                         |
| `Body::File("*.html"/"*.htm")`       | `text/html`                                |
| `Body::File("*.csv")`                | `text/csv`                                 |
| `Body::File("*.txt")`                | `text/plain`                               |

If `Content-Type` is already present in the request headers, auto-detection is
skipped.

---

## Execution Flow

```
run_file(path, params, opts)
  │
  ├─ read file to string
  └─ run_collection_from_str_with_opts(content, params, base_dir, opts)
       │
       ├─ parse_collection(content, params, base_dir)
       │    ├─ client_parser::request_collection(content)
       │    │    → Vec<RequestEntry>
       │    └─ for each entry: entry_to_test_case(entry, params, base_dir)
       │         → Vec<TestCase>
       │
       ├─ apply opts.response_file → TestCase::response_file
       │
       ├─ extract collection scripts (from .http metadata or opts paths)
       │
       ├─ extract allowlist: url_prefix for each request URL
       │
       ├─ HttpTestRunner::builder()
       │    .allowed_prefixes(allowlist)
       │    .env(params)
       │    .collection_pre_script(...)
       │    .collection_post_script(...)
       │    .build()
       │
       └─ runner.run_collection(test_cases)
            → CollectionResult
```

---

## Variable Precedence

When building a `TestCase`, params from three sources are merged in this order
(later wins):

1. Runner-level env (set via `HttpTestRunner::builder().env(...)`)
2. CLI `--param` flags (passed as `params` to `run_file`)
3. Entry-level `### @param name value` metadata

This means per-entry metadata can override CLI flags, which override runner
defaults.

---

## Source

`src/runner.rs` — public API plus unit tests covering URL prefix extraction,
variable substitution, content-type detection, and entry-to-test-case
conversion.
