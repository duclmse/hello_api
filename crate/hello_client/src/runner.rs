//! Bridge between the nom `.http` file parser and [`HttpTestRunner`].
//!
//! Converts parsed [`RequestEntry`] values into [`TestCase`]s and runs them
//! through the sandbox.

use std::collections::HashMap;
use std::path::Path;

use crate::http_runner::{CollectionResult, HttpTestRunner};
use hello_core::client_parser::request_collection;
use hello_core::http_request::{
    Body, MultipartPart, PartContent, RequestEntry, Script, Url, UrlSegment,
};
use hello_core::{HttpRequest as SandboxRequest, TestCase};

// ─── URL resolution ───────────────────────────────────────────────────────────

/// Resolve a parsed [`Url`] to a plain string, substituting `{{var}}`
/// placeholders from `params`.
///
/// `Url::Raw` is returned verbatim. `Url::Segments` concatenates host, path,
/// and query param segments in order.
fn resolve_url(url: &Url<'_>, params: &HashMap<String, String>) -> String {
    match url {
        // Collapse multiline URLs: join lines, strip leading whitespace.
        Url::Raw(raw) => raw.lines().map(str::trim).filter(|l| !l.is_empty()).collect(),
        Url::Segments {
            host,
            path,
            query_params,
        } => {
            let mut out = String::new();
            for seg in host.iter().chain(path.iter()).chain(query_params.iter()) {
                match seg {
                    UrlSegment::Text(t) => out.push_str(t),
                    UrlSegment::Variable(v) => match params.get(*v) {
                        Some(val) => out.push_str(val),
                        None => {
                            out.push_str("{{");
                            out.push_str(v);
                            out.push_str("}}");
                        },
                    },
                }
            }
            out
        },
    }
}

/// Extract the scheme+host prefix from a resolved URL string for use in the
/// HTTP allowlist (e.g. `"https://api.example.com"`).
fn url_prefix(url: &str) -> Option<String> {
    // Find the end of "scheme://host" — stop at the third `/` or end.
    let after_scheme = url.find("://").map(|i| i + 3)?;
    let rest = &url[after_scheme..];
    let host_end = rest.find('/').map(|i| after_scheme + i).unwrap_or(url.len());
    Some(url[..host_end].to_string())
}

/// Sanitize a test name into a filesystem-safe filename (no extension added).
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ─── Script loading with import rewriting ────────────────────────────────────

/// Load a script and rewrite any relative `import` specifiers to `sandbox:`
/// equivalents, collecting the dep files to register as modules.
///
/// Returns `(source, modules)` where `modules` is a list of
/// `(sandbox:specifier, source)` pairs for all transitive deps.
///
/// Relative imports (`./foo.js`, `../bar.js`) are rewritten to
/// `sandbox:path/relative/to/base_dir`. Any `sandbox:` or absolute specifiers
/// in the script are left untouched.
pub fn load_script_with_deps(
    script: &Script<'_>,
    base_dir: &Path,
) -> Result<(String, Vec<(String, String)>), String> {
    let mut modules: Vec<(String, String)> = Vec::new();
    let mut registered: std::collections::HashSet<String> = Default::default();
    let (src, _) = load_script_with_deps_inner(script, base_dir, &mut registered, &mut modules)?;
    Ok((src, modules))
}

/// Inner variant that shares `registered`/`modules` across multiple script loads.
fn load_script_with_deps_inner(
    script: &Script<'_>,
    base_dir: &Path,
    registered: &mut std::collections::HashSet<String>,
    modules: &mut Vec<(String, String)>,
) -> Result<(String, ()), String> {
    // Canonicalize root once so strip_prefix works even when temp dirs are symlinks.
    let root = base_dir.canonicalize().unwrap_or_else(|_| base_dir.to_path_buf());
    let src = match script {
        Script::Inline(src) => {
            let src = src.trim().to_string();
            rewrite_and_collect(&src, &root, &root, registered, modules)?
        },
        Script::File(rel_path) => {
            let abs = base_dir.join(rel_path.trim());
            let abs = abs.canonicalize().unwrap_or(abs);
            let src = std::fs::read_to_string(&abs)
                .map_err(|e| format!("Failed to read script {:?}: {}", abs, e))?;
            let file_dir = abs.parent().map(Path::new).unwrap_or_else(|| root.as_path());
            rewrite_and_collect(&src, file_dir, &root, registered, modules)?
        },
    };
    Ok((src, ()))
}

/// Recursively rewrite relative imports in `source` and populate `modules`.
///
/// - `file_dir`: directory of the current file (for resolving `./` and `../`).
/// - `root_dir`: `.http` file directory (for computing `sandbox:` specifiers).
fn rewrite_and_collect(
    source: &str,
    file_dir: &Path,
    root_dir: &Path,
    registered: &mut std::collections::HashSet<String>,
    modules: &mut Vec<(String, String)>,
) -> Result<String, String> {
    let mut out = String::with_capacity(source.len() + 64);
    let trailing_newline = source.ends_with('\n');

    for line in source.lines() {
        let rewritten = rewrite_import_line(line, file_dir, root_dir, registered, modules)?;
        out.push_str(&rewritten);
        out.push('\n');
    }

    if !trailing_newline && out.ends_with('\n') {
        out.pop();
    }
    Ok(out)
}

/// Attempt to rewrite a single line's relative import specifier to `sandbox:`.
fn rewrite_import_line(
    line: &str,
    file_dir: &Path,
    root_dir: &Path,
    registered: &mut std::collections::HashSet<String>,
    modules: &mut Vec<(String, String)>,
) -> Result<String, String> {
    let trimmed = line.trim();
    // Fast reject: only process lines that look like import statements.
    if !trimmed.starts_with("import") {
        return Ok(line.to_string());
    }

    for &q in &['"', '\''] {
        // Find the last ` from "` or ` from '` (handles `import ... from "..."`)
        let marker = format!(" from {q}");
        if let Some(from_idx) = line.rfind(&marker) {
            let spec_start = from_idx + marker.len();
            if let Some(close) = line[spec_start..].find(q) {
                let spec = &line[spec_start..spec_start + close];
                if spec.starts_with("./") || spec.starts_with("../") {
                    let abs = file_dir.join(spec);
                    let abs = abs.canonicalize().unwrap_or(abs);

                    // Compute sandbox: specifier relative to root_dir.
                    let rel = abs.strip_prefix(root_dir).unwrap_or(abs.as_path());
                    let sandbox_spec =
                        format!("sandbox:{}", rel.to_string_lossy().replace('\\', "/"));

                    // Recursively process dep (depth-first, avoids cycles via `registered`).
                    if !registered.contains(&sandbox_spec) {
                        registered.insert(sandbox_spec.clone());
                        let dep_src = std::fs::read_to_string(&abs).map_err(|e| {
                            format!("Cannot read import {:?}: {}", abs.display(), e)
                        })?;
                        let dep_dir = abs.parent().map(Path::new).unwrap_or(root_dir);
                        let rewritten_dep =
                            rewrite_and_collect(&dep_src, dep_dir, root_dir, registered, modules)?;
                        modules.push((sandbox_spec.clone(), rewritten_dep));
                    }

                    // Rewrite the line.
                    let prefix = &line[..from_idx + marker.len()];
                    let suffix = &line[spec_start + close..]; // starts with `q`
                    return Ok(format!("{prefix}{sandbox_spec}{suffix}"));
                }
                break; // found a `from "..."` but not relative — leave unchanged
            }
        }
    }
    Ok(line.to_string())
}

/// Scan a script source for names already imported from `sandbox:*` modules.
///
/// Used to filter the auto-import prelude so we don't produce duplicate
/// binding `SyntaxError`s when the user already has an explicit import.
pub fn scan_sandbox_imports(source: &str) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("import") {
            continue;
        }
        // Only care about `sandbox:` specifiers.
        let is_sandbox = trimmed.contains("\"sandbox:") || trimmed.contains("'sandbox:");
        if !is_sandbox {
            continue;
        }
        // Extract names from `import { a, b as c } from "sandbox:..."`.
        if let (Some(open), Some(close)) = (trimmed.find('{'), trimmed.find('}')) {
            let inner = &trimmed[open + 1..close];
            for part in inner.split(',') {
                let binding = part.split_whitespace().last().unwrap_or("");
                if !binding.is_empty() {
                    names.insert(binding.to_string());
                }
            }
        }
    }
    names
}

// ─── Body resolution ──────────────────────────────────────────────────────────

/// Resolve a parsed [`Body`] to a plain string suitable for the sandbox fetch.
///
/// `Body::File` and multipart `PartContent::File` entries are read from disk
/// relative to `base_dir` at this point, so the sandbox never sees raw paths.
fn resolve_body(body: Option<Body>, base_dir: &Path) -> Result<Option<String>, String> {
    match body {
        None => Ok(None),
        Some(Body::Raw(s)) => Ok(Some(s)),
        Some(Body::File(path)) => {
            let abs = base_dir.join(&path);
            let content = std::fs::read_to_string(&abs)
                .map_err(|e| format!("Failed to read body file {:?}: {}", abs, e))?;
            Ok(Some(content))
        },
        Some(Body::Multipart { boundary, parts }) => {
            Ok(Some(build_multipart_body(&boundary, &parts, base_dir)?))
        },
    }
}

/// Serialize a list of [`MultipartPart`]s into a standards-compliant
/// multipart body string using CRLF line endings.
fn build_multipart_body(
    boundary: &str,
    parts: &[MultipartPart],
    base_dir: &Path,
) -> Result<String, String> {
    let mut out = String::new();
    for part in parts {
        out.push_str(&format!("--{}\r\n", boundary));
        for (k, v) in &part.headers {
            out.push_str(&format!("{}: {}\r\n", k, v));
        }
        out.push_str("\r\n");
        match &part.content {
            PartContent::Text(text) => {
                out.push_str(text);
                out.push_str("\r\n");
            },
            PartContent::File(path) => {
                let abs = base_dir.join(path);
                let content = std::fs::read_to_string(&abs)
                    .map_err(|e| format!("Failed to read part file {:?}: {}", abs, e))?;
                out.push_str(&content);
                if !content.ends_with('\n') {
                    out.push_str("\r\n");
                }
            },
        }
    }
    out.push_str(&format!("--{}--\r\n", boundary));
    Ok(out)
}

// ─── Content-Type detection ───────────────────────────────────────────────────

/// Infer a `Content-Type` value from the body when the user hasn't set one.
///
/// Returns `None` for `Body::Raw` whose content doesn't look like JSON or XML
/// (e.g. plain text, URL-encoded form) — those require an explicit header.
fn detect_content_type(body: &Option<Body>) -> Option<String> {
    match body {
        None => None,
        Some(Body::Multipart { boundary, .. }) => {
            Some(format!("multipart/form-data; boundary={}", boundary))
        },
        Some(Body::Raw(s)) => detect_from_text(s.trim_start()),
        Some(Body::File(path)) => {
            // Prefer extension-based detection; don't read the file yet.
            let ext = std::path::Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            match ext.as_str() {
                "json" => Some("application/json".to_string()),
                "xml" => Some("application/xml".to_string()),
                "html" | "htm" => Some("text/html".to_string()),
                "txt" => Some("text/plain".to_string()),
                "csv" => Some("text/csv".to_string()),
                "graphql" | "gql" => Some("application/json".to_string()),
                _ => None,
            }
        },
    }
}

/// Detect JSON or XML from the first non-whitespace character of a raw body.
fn detect_from_text(s: &str) -> Option<String> {
    if s.starts_with('{') || s.starts_with('[') {
        Some("application/json".to_string())
    } else if s.starts_with('<') {
        Some("application/xml".to_string())
    } else {
        None
    }
}

// ─── Conversion ───────────────────────────────────────────────────────────────

/// Convert a single parsed [`RequestEntry`] into a [`TestCase`].
///
/// Script file paths are resolved relative to `base_dir` (usually the directory
/// containing the `.http` file). Variable placeholders in the URL and header
/// values are resolved from `params`.
pub fn entry_to_test_case(
    entry: RequestEntry<'_>,
    params: &HashMap<String, String>,
    base_dir: &Path,
) -> Result<TestCase, String> {
    let url = resolve_url(&entry.request.request_line.url, params);
    let method = entry.request.request_line.method.to_uppercase();
    let mut headers: Vec<(String, String)> =
        entry.request.headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();

    // Auto-inject Content-Type when the user hasn't set one.
    let has_content_type = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
    if !has_content_type && let Some(ct) = detect_content_type(&entry.request.body) {
        headers.push(("Content-Type".to_string(), ct));
    }

    let body = resolve_body(entry.request.body, base_dir)?;

    let request = SandboxRequest {
        url,
        method,
        headers,
        body,
    };

    // Load scripts and discover transitive relative-import dependencies.
    // Modules are deduplicated across both scripts via the shared `registered` set.
    let mut modules: Vec<(String, String)> = Vec::new();
    let mut registered: std::collections::HashSet<String> = Default::default();

    let pre_script = if let Some(s) = entry.pre_script.as_ref() {
        let (src, _) = load_script_with_deps_inner(s, base_dir, &mut registered, &mut modules)?;
        Some(src)
    } else {
        None
    };

    let post_script = if let Some(s) = entry.post_script.as_ref() {
        let (src, _) = load_script_with_deps_inner(s, base_dir, &mut registered, &mut modules)?;
        Some(src)
    } else {
        None
    };

    let desc = entry.metadata.description.join(" ");
    let name = if desc.trim().is_empty() {
        format!("{} {}", request.method, request.url)
    } else {
        desc.trim().to_string()
    };

    let output_file = entry.metadata.params.get("output").map(|s| s.to_string());
    let response_file = entry.metadata.params.get("response-file").map(|s| s.to_string());

    Ok(TestCase {
        name,
        request,
        pre_script,
        post_script,
        modules,
        output_file,
        response_file,
        ..Default::default()
    })
}

// ─── Collection-level script extraction ──────────────────────────────────────

/// Load a single script file relative to `base_dir`.
fn load_file_script(
    path: &str,
    base_dir: &Path,
) -> Result<(String, Vec<(String, String)>), String> {
    use hello_core::http_request::Script;
    load_script_with_deps(&Script::File(path), base_dir)
}

/// Loaded script source + transitive module dependencies.
type ScriptBundle = Option<(String, Vec<(String, String)>)>;

/// Scan `content` for `### @param collection-pre-script` and
/// `### @param collection-post-script` lines and load the referenced files.
///
/// Returns `(pre, post)` where each is `Some((source, modules))` when the param
/// is present and the file can be loaded, or `None` otherwise.
fn extract_collection_scripts(content: &str, base_dir: &Path) -> (ScriptBundle, ScriptBundle) {
    let mut pre_path: Option<String> = None;
    let mut post_path: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("### @param collection-pre-script ") {
            pre_path = Some(rest.trim().to_string());
        } else if let Some(rest) = trimmed.strip_prefix("### @param collection-post-script ") {
            post_path = Some(rest.trim().to_string());
        }
    }

    let pre = pre_path.as_deref().and_then(|p| load_file_script(p, base_dir).ok());
    let post = post_path.as_deref().and_then(|p| load_file_script(p, base_dir).ok());
    (pre, post)
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Parse `content` as a `.http` collection and return the [`TestCase`] list
/// **without** running any requests.
///
/// This is the building block for UIs that need to display test names before
/// execution starts. Variable placeholders in URLs and headers are resolved
/// from `params`; script file references are resolved relative to `base_dir`.
pub fn parse_collection(
    content: &str,
    params: &HashMap<String, String>,
    base_dir: &Path,
) -> Result<Vec<TestCase>, String> {
    let (_, entries) = request_collection(content).map_err(|e| format!("Parse error: {:?}", e))?;
    entries.into_iter().map(|e| entry_to_test_case(e, params, base_dir)).collect()
}

/// Parse `content` as a `.http` collection, convert each entry to a
/// [`TestCase`], and run them through an [`HttpTestRunner`].
///
/// `params` supplies values for `{{variable}}` placeholders in URLs and headers.
/// `base_dir` is used to resolve `> script.js` file references.
///
/// URL prefixes are auto-extracted from all resolved request URLs and added to
/// the sandbox allowlist so the fetch phase can reach the target hosts.
///
/// **Must be called from a `tokio::task::LocalSet`.**
pub async fn run_collection_from_str(
    content: &str,
    params: &HashMap<String, String>,
    base_dir: &Path,
) -> Result<CollectionResult, String> {
    run_collection_from_str_with_opts(content, params, base_dir, RunOpts::default()).await
}

/// Options for collection run functions.
#[derive(Default)]
pub struct RunOpts<'a> {
    /// Skip HTTP fetch for all requests and load the response from this path.
    pub response_file: Option<&'a str>,
    /// CLI-supplied collection pre-script path (overrides `### @collection-pre-script`).
    pub collection_pre_script: Option<&'a str>,
    /// CLI-supplied collection post-script path (overrides `### @collection-post-script`).
    pub collection_post_script: Option<&'a str>,
    /// Write each response body to this path. For a single request the path is
    /// used as-is; for multiple requests it becomes a directory prefix.
    pub save_response: Option<&'a str>,
    /// Case-insensitive substring filter — only test cases whose name contains
    /// this string are run. `None` means run all.
    pub name_filter: Option<&'a str>,
}

/// Like [`run_collection_from_str`] but with extra run-time options.
pub async fn run_collection_from_str_with_opts(
    content: &str,
    params: &HashMap<String, String>,
    base_dir: &Path,
    opts: RunOpts<'_>,
) -> Result<CollectionResult, String> {
    let mut test_cases = parse_collection(content, params, base_dir)?;

    if let Some(rf) = opts.response_file {
        for tc in &mut test_cases {
            tc.response_file = Some(rf.to_string());
        }
    }

    if let Some(filter) = opts.name_filter {
        let f = filter.to_lowercase();
        test_cases.retain(|tc| tc.name.to_lowercase().contains(&f));
        if test_cases.is_empty() {
            return Err(format!("No requests matched name filter: {:?}", filter));
        }
    }

    if let Some(out) = opts.save_response {
        let multi = test_cases.len() > 1;
        for tc in &mut test_cases {
            if tc.output_file.is_none() {
                tc.output_file = Some(if multi {
                    format!("{}/{}", out.trim_end_matches('/'), sanitize_filename(&tc.name))
                } else {
                    out.to_string()
                });
            }
        }
    }

    // Extract collection scripts from the .http content, then let CLI paths override.
    let (mut col_pre, mut col_post) = extract_collection_scripts(content, base_dir);
    if let Some(p) = opts.collection_pre_script {
        col_pre = load_file_script(p, base_dir).ok();
    }
    if let Some(p) = opts.collection_post_script {
        col_post = load_file_script(p, base_dir).ok();
    }

    let allowed_prefixes: Vec<String> =
        test_cases.iter().filter_map(|tc| url_prefix(&tc.request.url)).collect();

    let mut builder = HttpTestRunner::builder().allowed_prefixes(allowed_prefixes);
    for (k, v) in params {
        builder = builder.env(k.clone(), v.clone());
    }
    if let Some((src, mods)) = col_pre {
        builder = builder.collection_pre_script(src, mods);
    }
    if let Some((src, mods)) = col_post {
        builder = builder.collection_post_script(src, mods);
    }
    let mut runner = builder.build().map_err(|e| format!("Runner build error: {}", e))?;

    runner.run_collection(test_cases).await.map_err(|e| format!("Collection run error: {}", e))
}

/// Read a `.http` file at `path`, parse it, and run all requests.
///
/// `params` supplies values for `{{variable}}` placeholders.
///
/// **Must be called from a `tokio::task::LocalSet`.**
pub async fn run_file(
    path: &Path,
    params: &HashMap<String, String>,
    opts: RunOpts<'_>,
) -> Result<CollectionResult, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read {:?}: {}", path, e))?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    run_collection_from_str_with_opts(&content, params, base_dir, opts).await
}

/// Run a pre-imported list of [`TestCase`]s (e.g. from a Postman or Bruno adapter).
///
/// Unlike [`run_file`], no `.http` parsing happens — the cases are used directly.
/// Collection-level scripts from `opts` are applied.
///
/// **Must be called from a `tokio::task::LocalSet`.**
pub async fn run_test_cases(
    mut cases: Vec<TestCase>,
    params: &HashMap<String, String>,
    opts: RunOpts<'_>,
) -> Result<CollectionResult, String> {
    if let Some(filter) = opts.name_filter {
        let f = filter.to_lowercase();
        cases.retain(|tc| tc.name.to_lowercase().contains(&f));
        if cases.is_empty() {
            return Err(format!("No requests matched name filter: {:?}", filter));
        }
    }

    if let Some(rf) = opts.response_file {
        for tc in &mut cases {
            tc.response_file = Some(rf.to_string());
        }
    }

    if let Some(out) = opts.save_response {
        let multi = cases.len() > 1;
        for tc in &mut cases {
            if tc.output_file.is_none() {
                tc.output_file = Some(if multi {
                    format!("{}/{}", out.trim_end_matches('/'), sanitize_filename(&tc.name))
                } else {
                    out.to_string()
                });
            }
        }
    }

    let allowed_prefixes: Vec<String> =
        cases.iter().filter_map(|tc| url_prefix(&tc.request.url)).collect();

    let mut builder = HttpTestRunner::builder().allowed_prefixes(allowed_prefixes);
    for (k, v) in params {
        builder = builder.env(k.clone(), v.clone());
    }
    if let Some(p) = opts.collection_pre_script
        && let Ok((src, mods)) = load_file_script(p, Path::new("."))
    {
        builder = builder.collection_pre_script(src, mods);
    }
    if let Some(p) = opts.collection_post_script
        && let Ok((src, mods)) = load_file_script(p, Path::new("."))
    {
        builder = builder.collection_post_script(src, mods);
    }
    let mut runner = builder.build().map_err(|e| format!("Runner build error: {}", e))?;
    runner.run_collection(cases).await.map_err(|e| format!("Collection run error: {}", e))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn no_params() -> HashMap<String, String> {
        HashMap::new()
    }

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    // ── url_prefix ────────────────────────────────────────────────────────────

    #[test]
    fn url_prefix_extracts_scheme_and_host() {
        assert_eq!(
            url_prefix("https://api.example.com/v1/users"),
            Some("https://api.example.com".to_string())
        );
    }

    #[test]
    fn url_prefix_handles_no_path() {
        assert_eq!(
            url_prefix("https://api.example.com"),
            Some("https://api.example.com".to_string())
        );
    }

    #[test]
    fn url_prefix_handles_port() {
        assert_eq!(
            url_prefix("http://localhost:8080/api/v1"),
            Some("http://localhost:8080".to_string())
        );
    }

    #[test]
    fn url_prefix_no_scheme_returns_none() {
        assert_eq!(url_prefix("not-a-url"), None);
    }

    #[test]
    fn url_prefix_trailing_slash_only() {
        assert_eq!(url_prefix("https://example.com/"), Some("https://example.com".to_string()));
    }

    // ── resolve_url ───────────────────────────────────────────────────────────

    #[test]
    fn resolve_url_raw_passthrough() {
        let url = hello_core::http_request::Url::Raw("https://raw.example.com/path");
        assert_eq!(resolve_url(&url, &no_params()), "https://raw.example.com/path");
    }

    #[test]
    fn resolve_url_substitutes_variable() {
        use hello_core::http_request::{Url, UrlSegment};
        let url = Url::Segments {
            host: vec![UrlSegment::Variable("host")],
            path: vec![UrlSegment::Text("/api")],
            query_params: vec![],
        };
        let p = params(&[("host", "https://example.com")]);
        assert_eq!(resolve_url(&url, &p), "https://example.com/api");
    }

    #[test]
    fn resolve_url_missing_variable_keeps_placeholder() {
        use hello_core::http_request::{Url, UrlSegment};
        let url = Url::Segments {
            host: vec![UrlSegment::Variable("missing")],
            path: vec![UrlSegment::Text("/")],
            query_params: vec![],
        };
        let result = resolve_url(&url, &no_params());
        assert!(result.contains("{{missing}}"), "got: {}", result);
    }

    #[test]
    fn resolve_url_concatenates_segments() {
        use hello_core::http_request::{Url, UrlSegment};
        let url = Url::Segments {
            host: vec![UrlSegment::Text("https://example.com")],
            path: vec![UrlSegment::Text("/users/"), UrlSegment::Variable("id")],
            query_params: vec![],
        };
        let p = params(&[("id", "42")]);
        assert_eq!(resolve_url(&url, &p), "https://example.com/users/42");
    }

    #[test]
    fn resolve_url_with_query_params() {
        use hello_core::http_request::{Url, UrlSegment};
        let url = Url::Segments {
            host: vec![UrlSegment::Text("https://example.com")],
            path: vec![UrlSegment::Text("/search")],
            query_params: vec![UrlSegment::Text("?q="), UrlSegment::Variable("term")],
        };
        let p = params(&[("term", "rust")]);
        assert_eq!(resolve_url(&url, &p), "https://example.com/search?q=rust");
    }

    // ── entry_to_test_case ────────────────────────────────────────────────────

    fn parse_single(input: &str) -> hello_core::http_request::RequestEntry<'_> {
        let (_, mut entries) = hello_core::client_parser::request_collection(input).unwrap();
        entries.remove(0)
    }

    #[test]
    fn entry_name_from_metadata_description() {
        let input = "### Fetch User Profile\n\nGET https://example.com/user HTTP/1.1\n\n";
        let entry = parse_single(input);
        let tc = entry_to_test_case(entry, &no_params(), Path::new(".")).unwrap();
        assert_eq!(tc.name, "Fetch User Profile");
    }

    #[test]
    fn entry_name_fallback_to_method_and_url() {
        let input = "GET https://example.com/health HTTP/1.1\n\n";
        let entry = parse_single(input);
        let tc = entry_to_test_case(entry, &no_params(), Path::new(".")).unwrap();
        assert!(tc.name.starts_with("GET"), "name={}", tc.name);
        assert!(tc.name.contains("example.com"), "name={}", tc.name);
    }

    #[test]
    fn entry_method_is_uppercased() {
        let input = "post https://example.com/ HTTP/1.1\n\n";
        let entry = parse_single(input);
        let tc = entry_to_test_case(entry, &no_params(), Path::new(".")).unwrap();
        assert_eq!(tc.request.method, "POST");
    }

    #[test]
    fn entry_url_variable_substituted() {
        let input = "GET {{host}}/users HTTP/1.1\n\n";
        let entry = parse_single(input);
        let p = params(&[("host", "https://api.example.com")]);
        let tc = entry_to_test_case(entry, &p, Path::new(".")).unwrap();
        assert!(tc.request.url.contains("https://api.example.com"), "url={}", tc.request.url);
    }

    #[test]
    fn entry_url_missing_variable_keeps_placeholder() {
        let input = "GET {{host}}/items HTTP/1.1\n\n";
        let entry = parse_single(input);
        let tc = entry_to_test_case(entry, &no_params(), Path::new(".")).unwrap();
        assert!(tc.request.url.contains("{{host}}"), "url={}", tc.request.url);
    }

    #[test]
    fn entry_headers_forwarded() {
        let input = "GET https://example.com/ HTTP/1.1\nAuthorization: Bearer tok\n\n";
        let entry = parse_single(input);
        let tc = entry_to_test_case(entry, &no_params(), Path::new(".")).unwrap();
        assert!(
            tc.request.headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer tok"),
            "headers={:?}",
            tc.request.headers
        );
    }

    #[test]
    fn entry_body_forwarded() {
        let input =
            "POST https://example.com/ HTTP/1.1\nContent-Type: application/json\n\n{\"x\":1}\n";
        let entry = parse_single(input);
        let tc = entry_to_test_case(entry, &no_params(), Path::new(".")).unwrap();
        assert_eq!(tc.request.body.as_deref(), Some("{\"x\":1}"));
    }

    #[test]
    fn entry_inline_pre_script_loaded() {
        let input = "< {% const x = 1; %}\n\nGET https://example.com/ HTTP/1.1\n\n";
        let entry = parse_single(input);
        let tc = entry_to_test_case(entry, &no_params(), Path::new(".")).unwrap();
        assert!(tc.pre_script.as_deref().unwrap_or("").contains("const x = 1"));
    }

    #[test]
    fn entry_inline_post_script_loaded() {
        let input = "GET https://example.com/ HTTP/1.1\n\n> {% return results(); %}\n";
        let entry = parse_single(input);
        let tc = entry_to_test_case(entry, &no_params(), Path::new(".")).unwrap();
        assert!(tc.post_script.as_deref().unwrap_or("").contains("results"));
    }

    #[test]
    fn entry_missing_file_script_returns_error() {
        let input = "GET https://example.com/ HTTP/1.1\n\n> /nonexistent_path/script.js\n";
        let entry = parse_single(input);
        let result = entry_to_test_case(entry, &no_params(), Path::new("/tmp"));
        assert!(result.is_err(), "missing script file should return Err");
    }

    #[test]
    fn entry_no_pre_post_scripts() {
        let input = "GET https://example.com/ HTTP/1.1\n\n";
        let entry = parse_single(input);
        let tc = entry_to_test_case(entry, &no_params(), Path::new(".")).unwrap();
        assert!(tc.pre_script.is_none());
        assert!(tc.post_script.is_none());
    }

    // ── scan_sandbox_imports ──────────────────────────────────────────────────

    #[test]
    fn scan_imports_empty_source() {
        let imports = super::scan_sandbox_imports("");
        assert!(imports.is_empty());
    }

    #[test]
    fn scan_imports_single_named() {
        let src = r#"import { expect, results } from "sandbox:test";"#;
        let imports = super::scan_sandbox_imports(src);
        assert!(imports.contains("expect"), "{:?}", imports);
        assert!(imports.contains("results"), "{:?}", imports);
        assert!(!imports.contains("wrapResponse"), "{:?}", imports);
    }

    #[test]
    fn scan_imports_aliased_name() {
        let src = r#"import { expect as assert } from "sandbox:test";"#;
        let imports = super::scan_sandbox_imports(src);
        assert!(imports.contains("assert"), "{:?}", imports);
        assert!(!imports.contains("expect"), "{:?}", imports);
    }

    #[test]
    fn scan_imports_ignores_non_sandbox() {
        let src = r#"import { readFile } from "node:fs";
import { helper } from "./local.js";"#;
        let imports = super::scan_sandbox_imports(src);
        assert!(imports.is_empty(), "{:?}", imports);
    }

    #[test]
    fn scan_imports_multiple_sandbox_lines() {
        let src = r#"import { kv } from "sandbox:kv";
import { expect, results } from "sandbox:test";"#;
        let imports = super::scan_sandbox_imports(src);
        assert!(imports.contains("kv"), "{:?}", imports);
        assert!(imports.contains("expect"), "{:?}", imports);
        assert!(imports.contains("results"), "{:?}", imports);
    }

    // ── load_script_with_deps ─────────────────────────────────────────────────

    #[test]
    fn load_script_inline_no_deps() {
        let script = hello_core::http_request::Script::Inline("const x = 1; return x;");
        let (src, modules) = super::load_script_with_deps(&script, Path::new("/tmp")).unwrap();
        assert!(src.contains("const x = 1"));
        assert!(modules.is_empty(), "no deps expected: {:?}", modules);
    }

    #[test]
    fn load_script_inline_sandbox_import_left_unchanged() {
        let script = hello_core::http_request::Script::Inline(
            r#"import { expect } from "sandbox:test"; return 1;"#,
        );
        let (src, modules) = super::load_script_with_deps(&script, Path::new("/tmp")).unwrap();
        assert!(src.contains("sandbox:test"), "sandbox: import should be unchanged");
        assert!(modules.is_empty());
    }

    #[test]
    fn load_script_relative_import_rewritten() {
        let tmp = std::env::temp_dir().join(format!("runner_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let helper_path = tmp.join("helper.js");
        std::fs::write(&helper_path, "export const add = (a, b) => a + b;\n").unwrap();

        let script = hello_core::http_request::Script::Inline(
            r#"import { add } from "./helper.js";
return add(1, 2);"#,
        );
        let (src, modules) = super::load_script_with_deps(&script, &tmp).unwrap();

        assert!(src.contains("sandbox:helper.js"), "import should be rewritten: {src}");
        assert!(!src.contains("./helper.js"), "relative import should be gone: {src}");
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].0, "sandbox:helper.js");
        assert!(modules[0].1.contains("add"), "module source should be included");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn load_script_deduplicates_shared_dep() {
        let tmp = std::env::temp_dir().join(format!("runner_dedup_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let shared = tmp.join("shared.js");
        std::fs::write(&shared, "export const x = 1;\n").unwrap();

        let mut modules = Vec::new();
        let mut registered = std::collections::HashSet::new();

        let s1 = hello_core::http_request::Script::Inline(r#"import { x } from "./shared.js";"#);
        super::load_script_with_deps_inner(&s1, &tmp, &mut registered, &mut modules).unwrap();
        let count_after_first = modules.len();

        let s2 = hello_core::http_request::Script::Inline(r#"import { x } from "./shared.js";"#);
        super::load_script_with_deps_inner(&s2, &tmp, &mut registered, &mut modules).unwrap();

        assert_eq!(modules.len(), count_after_first, "shared dep should not be duplicated");
        std::fs::remove_dir_all(&tmp).ok();
    }

    // ── detect_content_type ───────────────────────────────────────────────────

    #[test]
    fn detect_json_object_body() {
        let body = Some(hello_core::http_request::Body::Raw(r#"{"key": "val"}"#.to_string()));
        assert_eq!(super::detect_content_type(&body).as_deref(), Some("application/json"));
    }

    #[test]
    fn detect_json_array_body() {
        let body = Some(hello_core::http_request::Body::Raw("[1,2,3]".to_string()));
        assert_eq!(super::detect_content_type(&body).as_deref(), Some("application/json"));
    }

    #[test]
    fn detect_xml_body() {
        let body = Some(hello_core::http_request::Body::Raw("<root><item/></root>".to_string()));
        assert_eq!(super::detect_content_type(&body).as_deref(), Some("application/xml"));
    }

    #[test]
    fn detect_plain_text_returns_none() {
        let body = Some(hello_core::http_request::Body::Raw("hello world".to_string()));
        assert_eq!(super::detect_content_type(&body), None);
    }

    #[test]
    fn detect_url_encoded_returns_none() {
        let body = Some(hello_core::http_request::Body::Raw("a=1&b=2".to_string()));
        assert_eq!(super::detect_content_type(&body), None);
    }

    #[test]
    fn detect_none_body_returns_none() {
        assert_eq!(super::detect_content_type(&None), None);
    }

    #[test]
    fn detect_file_json_extension() {
        let body = Some(hello_core::http_request::Body::File("./data.json".to_string()));
        assert_eq!(super::detect_content_type(&body).as_deref(), Some("application/json"));
    }

    #[test]
    fn detect_file_xml_extension() {
        let body = Some(hello_core::http_request::Body::File("request.xml".to_string()));
        assert_eq!(super::detect_content_type(&body).as_deref(), Some("application/xml"));
    }

    #[test]
    fn detect_file_unknown_extension_returns_none() {
        let body = Some(hello_core::http_request::Body::File("photo.png".to_string()));
        assert_eq!(super::detect_content_type(&body), None);
    }

    #[test]
    fn detect_multipart_injects_boundary() {
        let body = Some(hello_core::http_request::Body::Multipart {
            boundary: "WebBound".to_string(),
            parts: vec![],
        });
        assert_eq!(
            super::detect_content_type(&body).as_deref(),
            Some("multipart/form-data; boundary=WebBound"),
        );
    }

    #[test]
    fn entry_auto_injects_content_type_for_json_body() {
        let input = "POST https://example.com/ HTTP/1.1\n\n{\"x\":1}\n";
        let entry = parse_single(input);
        let tc = entry_to_test_case(entry, &no_params(), Path::new(".")).unwrap();
        assert!(
            tc.request.headers.iter().any(|(k, v)| k == "Content-Type" && v == "application/json"),
            "headers={:?}",
            tc.request.headers
        );
    }

    #[test]
    fn entry_does_not_override_explicit_content_type() {
        let input = "POST https://example.com/ HTTP/1.1\nContent-Type: text/plain\n\n{\"x\":1}\n";
        let entry = parse_single(input);
        let tc = entry_to_test_case(entry, &no_params(), Path::new(".")).unwrap();
        let ct: Vec<_> = tc
            .request
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .collect();
        assert_eq!(ct.len(), 1, "exactly one Content-Type");
        assert_eq!(ct[0].1, "text/plain");
    }

    #[test]
    fn entry_no_content_type_for_plain_text_body() {
        let input = "POST https://example.com/ HTTP/1.1\n\nhello world\n";
        let entry = parse_single(input);
        let tc = entry_to_test_case(entry, &no_params(), Path::new(".")).unwrap();
        assert!(
            !tc.request.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")),
            "should not inject Content-Type for plain text"
        );
    }
}
