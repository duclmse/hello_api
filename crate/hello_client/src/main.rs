use clap::{Args, Parser, Subcommand, ValueEnum};
use hello_client::{
    BrunoAdapter, CollectionResult, CurlAdapter, OpenApiAdapter, OpenCollectionAdapter,
    PostmanAdapter, TestCase, runner,
};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ─── Clap CLI definition ──────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "hello_client", about = "HTTP Request Runner")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// .http file to run (positional shorthand — use --from for other formats)
    #[arg(value_name = "FILE")]
    file: Option<PathBuf>,

    /// Input file or directory (any adapter-supported format; auto-detected).
    /// Use --format to override detection.
    #[arg(long = "from", value_name = "FILE")]
    from_file: Option<PathBuf>,

    /// Output file or directory for conversion (requires --from or FILE).
    /// Format is auto-detected from the extension; use --format to override.
    #[arg(long = "to", value_name = "FILE")]
    to_file: Option<PathBuf>,

    /// Collection format — http, postman, bruno, curl, opencollection, openapi.
    /// When converting (--to), this sets the OUTPUT format.
    /// When running (no --to), this is an INPUT hint (e.g. --format curl).
    #[arg(long, value_name = "FORMAT")]
    format: Option<String>,

    /// Run only requests whose name contains PATTERN (case-insensitive)
    #[arg(short = 'n', long, value_name = "PATTERN")]
    name: Option<String>,

    /// Configuration file path
    #[arg(short = 'c', long)]
    config: Option<PathBuf>,

    /// Variable substitution, e.g. key=value (repeatable)
    #[arg(short = 'p', long = "param", value_name = "KEY=VALUE")]
    params: Vec<String>,

    /// Enable verbose output
    #[arg(short = 'v', long)]
    verbose: bool,

    /// Request timeout in seconds
    #[arg(short = 't', long, default_value_t = 60)]
    timeout: u64,

    /// Results display format: json, plain, pretty
    #[arg(short = 'f', long = "output-format", value_enum, default_value_t = OutputFormatArg::Pretty)]
    output_format: OutputFormatArg,

    /// With --to <dir> --format http: write one .http file per request
    #[arg(long)]
    split: bool,

    /// Write pm.visualizer HTML output to this directory
    #[arg(long, value_name = "DIR")]
    visualize_dir: Option<PathBuf>,

    /// Write each response (status + headers + body) to FILE.
    /// For a collection, each response goes to FILE/<name>.<ext>.
    /// Per-request ### @param output annotations take precedence.
    #[arg(short = 'o', long = "out", value_name = "FILE")]
    out: Option<PathBuf>,

    /// Skip HTTP fetch; replay the synthetic response from FILE instead.
    #[arg(long, value_name = "FILE")]
    offline: Option<PathBuf>,

    /// Parse the collection and print request names without sending any requests.
    #[arg(long)]
    dry_run: bool,

    /// Print per-phase timing (pre-script, fetch, post-script) for each request.
    #[arg(long)]
    metrics: bool,

    /// Script to run before every request in the collection.
    #[arg(long, value_name = "FILE")]
    collection_pre_script: Option<PathBuf>,

    /// Script to run after every request in the collection.
    #[arg(long, value_name = "FILE")]
    collection_post_script: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Split a .http file into one file per request entry
    Split(SplitArgs),
    /// Merge multiple .http files (or a directory) into one
    Merge(MergeArgs),
}

#[derive(Args)]
struct SplitArgs {
    /// Input .http file to split
    #[arg(value_name = "FILE")]
    file: PathBuf,

    /// Output directory (created if absent)
    #[arg(short = 'o', long, value_name = "DIR")]
    output: PathBuf,
}

#[derive(Args)]
struct MergeArgs {
    /// Input .http files or a single directory of .http files
    #[arg(value_name = "INPUT", required = true)]
    inputs: Vec<PathBuf>,

    /// Output file (default: stdout)
    #[arg(short = 'o', long, value_name = "FILE")]
    output: Option<PathBuf>,
}

#[derive(Clone, ValueEnum)]
enum OutputFormatArg {
    Json,
    Plain,
    Pretty,
}

// ─── Internal config ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum OutputFormat {
    Json,
    Plain,
    Pretty,
}

#[derive(Debug, Clone)]
pub struct CliConfig {
    pub config_file: Option<PathBuf>,
    /// Positional FILE — used as the .http source when `--from` is absent.
    pub request_file: PathBuf,
    /// `--from FILE` — input file for any adapter-supported format.
    pub from_file: Option<PathBuf>,
    /// `--to FILE` — output destination for conversion.
    pub to_file: Option<PathBuf>,
    /// `--format` — collection format override (output when converting, input hint otherwise).
    pub format: Option<String>,
    /// `--name` — case-insensitive substring filter for request names.
    pub name_filter: Option<String>,
    pub params: HashMap<String, String>,
    pub verbose: bool,
    pub timeout: u64,
    pub output_format: OutputFormat,
    pub split: bool,
    pub visualize_dir: Option<PathBuf>,
    pub out: Option<PathBuf>,
    /// Path to a synthetic response file (`--offline`).
    pub offline: Option<PathBuf>,
    pub dry_run: bool,
    pub metrics: bool,
    pub collection_pre_script: Option<PathBuf>,
    pub collection_post_script: Option<PathBuf>,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            config_file: None,
            request_file: PathBuf::from("requests.http"),
            from_file: None,
            to_file: None,
            format: None,
            name_filter: None,
            params: HashMap::new(),
            verbose: false,
            timeout: 60,
            output_format: OutputFormat::Pretty,
            split: false,
            visualize_dir: None,
            out: None,
            offline: None,
            dry_run: false,
            metrics: false,
            collection_pre_script: None,
            collection_post_script: None,
        }
    }
}

impl CliConfig {
    fn from_cli(cli: &Cli) -> Result<Self, String> {
        let mut params = HashMap::new();
        for p in &cli.params {
            if let Some((k, v)) = p.split_once('=') {
                params.insert(k.to_string(), v.to_string());
            } else {
                return Err(format!("Invalid param format: {}. Use key=value", p));
            }
        }

        let format = if let Some(fmt) = &cli.format {
            let fmt = fmt.to_lowercase();
            if ![
                "curl",
                "http",
                "postman",
                "bruno",
                "opencollection",
                "openapi",
            ]
            .contains(&fmt.as_str())
            {
                return Err(format!(
                    "Invalid --format: {}. Use curl, http, postman, bruno, opencollection, or openapi",
                    fmt
                ));
            }
            Some(fmt)
        } else {
            None
        };

        Ok(Self {
            config_file: cli.config.clone(),
            request_file: cli.file.clone().unwrap_or_else(|| PathBuf::from("requests.http")),
            from_file: cli.from_file.clone(),
            to_file: cli.to_file.clone(),
            format,
            name_filter: cli.name.clone(),
            params,
            verbose: cli.verbose,
            timeout: cli.timeout,
            output_format: match cli.output_format {
                OutputFormatArg::Json => OutputFormat::Json,
                OutputFormatArg::Plain => OutputFormat::Plain,
                OutputFormatArg::Pretty => OutputFormat::Pretty,
            },
            split: cli.split,
            visualize_dir: cli.visualize_dir.clone(),
            out: cli.out.clone(),
            offline: cli.offline.clone(),
            dry_run: cli.dry_run,
            metrics: cli.metrics,
            collection_pre_script: cli.collection_pre_script.clone(),
            collection_post_script: cli.collection_post_script.clone(),
        })
    }

    pub fn merge_with_file_config(&mut self, file_config: &FileConfig) {
        if self.timeout == 30 && file_config.timeout != 30 {
            self.timeout = file_config.timeout;
        }
        if !self.verbose && file_config.verbose {
            self.verbose = file_config.verbose;
        }
        for (key, value) in &file_config.default_params {
            self.params.entry(key.clone()).or_insert(value.clone());
        }
        if let (None, Some(s)) = (&self.collection_pre_script, &file_config.collection_pre_script) {
            self.collection_pre_script = Some(PathBuf::from(s));
        }
        if let (None, Some(s)) = (&self.collection_post_script, &file_config.collection_post_script)
        {
            self.collection_post_script = Some(PathBuf::from(s));
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileConfig {
    pub timeout: u64,
    pub verbose: bool,
    pub default_params: HashMap<String, String>,
    pub base_url: Option<String>,
    pub collection_pre_script: Option<String>,
    pub collection_post_script: Option<String>,
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            timeout: 30,
            verbose: false,
            default_params: HashMap::new(),
            base_url: None,
            collection_pre_script: None,
            collection_post_script: None,
        }
    }
}

impl FileConfig {
    pub fn from_file(path: &PathBuf) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read config file: {}", e))?;
        Self::parse(&content)
    }

    pub fn parse(content: &str) -> Result<Self, String> {
        if content.trim_start().starts_with('{') {
            return Self::parse_json(content);
        }
        Self::parse_native(content)
    }

    fn parse_native(content: &str) -> Result<Self, String> {
        let mut config = Self::default();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim();
                match key {
                    "timeout" => {
                        config.timeout = value
                            .parse()
                            .map_err(|_| format!("Invalid timeout value: {}", value))?;
                    },
                    "verbose" => {
                        config.verbose = value
                            .parse()
                            .map_err(|_| format!("Invalid verbose value: {}", value))?;
                    },
                    "base_url" => {
                        config.base_url = Some(value.to_string());
                    },
                    "collection_pre_script" => {
                        config.collection_pre_script = Some(value.to_string());
                    },
                    "collection_post_script" => {
                        config.collection_post_script = Some(value.to_string());
                    },
                    _ if key.starts_with("param.") => {
                        config.default_params.insert(key[6..].to_string(), value.to_string());
                    },
                    _ => return Err(format!("Unknown config key: {}", key)),
                }
            }
        }
        Ok(config)
    }

    fn parse_json(content: &str) -> Result<Self, String> {
        let root: serde_json::Value =
            serde_json::from_str(content).map_err(|e| format!("env JSON parse error: {e}"))?;
        let obj = root.as_object().ok_or("env JSON must be an object")?;

        let mut config = Self::default();

        if let Some(v) = obj.get("timeout").and_then(|v| v.as_u64()) {
            config.timeout = v;
        }
        if let Some(v) = obj.get("verbose").and_then(|v| v.as_bool()) {
            config.verbose = v;
        }
        if let Some(v) = obj.get("base_url").and_then(|v| v.as_str()) {
            config.base_url = Some(v.to_string());
        }
        if let Some(v) = obj.get("collection_pre_script").and_then(|v| v.as_str()) {
            config.collection_pre_script = Some(v.to_string());
        }
        if let Some(v) = obj.get("collection_post_script").and_then(|v| v.as_str()) {
            config.collection_post_script = Some(v.to_string());
        }
        if let Some(params) = obj.get("param").and_then(|v| v.as_object()) {
            for (k, v) in params {
                if let Some(s) = v.as_str() {
                    config.default_params.insert(k.clone(), s.to_string());
                } else {
                    config.default_params.insert(k.clone(), v.to_string());
                }
            }
        }

        Ok(config)
    }
}

// ─── Format detection ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum CollectionFormat {
    Http,
    Postman,
    Bruno,
    OpenCollection,
    OpenApi,
}

fn detect_format(path: &Path) -> Result<CollectionFormat, String> {
    if path.is_dir() {
        return Ok(CollectionFormat::Bruno);
    }
    match path.extension().and_then(|e| e.to_str()) {
        Some("json") => {
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("Cannot read {:?}: {}", path, e))?;
            if is_opencollection_json(&content) {
                Ok(CollectionFormat::OpenCollection)
            } else {
                Ok(CollectionFormat::Postman)
            }
        },
        Some("bru") => Ok(CollectionFormat::Bruno),
        Some("http") | Some("rest") => Ok(CollectionFormat::Http),
        Some("yaml") | Some("yml") => {
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("Cannot read {:?}: {}", path, e))?;
            if is_openapi_yaml(&content) {
                Ok(CollectionFormat::OpenApi)
            } else {
                Ok(CollectionFormat::OpenCollection)
            }
        },
        ext => Err(format!(
            "Cannot detect format for {:?}. Provide a .json (Postman/OpenCollection), .bru (Bruno), .http, .yaml/.yml (OpenAPI or OpenCollection), or a directory.",
            ext.unwrap_or("<none>")
        )),
    }
}

fn is_opencollection_json(content: &str) -> bool {
    content.chars().take(512).collect::<String>().contains("\"opencollection\"")
}

fn is_openapi_yaml(content: &str) -> bool {
    content.lines().take(20).any(|l| {
        let t = l.trim();
        t.starts_with("openapi:") || t.starts_with("swagger:")
    })
}

// ─── Import ───────────────────────────────────────────────────────────────────

fn import_collection(
    path: &PathBuf,
    format: CollectionFormat,
) -> Result<(String, Vec<TestCase>), String> {
    match format {
        CollectionFormat::Http => {
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("Failed to read {:?}: {}", path, e))?;
            let base_dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
            let cases = runner::parse_collection(&content, &HashMap::new(), base_dir)?;
            let name =
                path.file_stem().and_then(|s| s.to_str()).unwrap_or("collection").to_string();
            Ok((name, cases))
        },
        CollectionFormat::Postman => {
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("Failed to read {:?}: {}", path, e))?;
            let col = PostmanAdapter::import(&content)
                .map_err(|e| format!("Postman import error: {:?}", e))?;
            Ok((col.name, col.tests))
        },
        CollectionFormat::Bruno => {
            let cases = BrunoAdapter::import_dir(path)
                .map_err(|e| format!("Bruno import error: {:?}", e))?;
            let name =
                path.file_name().and_then(|s| s.to_str()).unwrap_or("collection").to_string();
            Ok((name, cases))
        },
        CollectionFormat::OpenCollection => {
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("Failed to read {:?}: {}", path, e))?;
            let col = OpenCollectionAdapter::import(&content)
                .map_err(|e| format!("OpenCollection import error: {}", e))?;
            Ok((col.name, col.tests))
        },
        CollectionFormat::OpenApi => {
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("Failed to read {:?}: {}", path, e))?;
            let col = OpenApiAdapter::import(&content)
                .map_err(|e| format!("OpenAPI import error: {}", e))?;
            Ok((col.name, col.tests))
        },
    }
}

// ─── .http serializer ─────────────────────────────────────────────────────────

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
            out.push_str("\n> {%\n");
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

// ─── Split .http export ───────────────────────────────────────────────────────

fn name_to_path(name: &str) -> PathBuf {
    let parts: Vec<&str> = name.split('/').collect();
    let sanitize = |s: &str| {
        s.trim()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    let mut path = PathBuf::new();
    for part in &parts[..parts.len().saturating_sub(1)] {
        path.push(sanitize(part));
    }
    let filename = sanitize(parts.last().unwrap_or(&"request"));
    path.push(format!("{}.http", filename));
    path
}

fn export_http_split(cases: Vec<TestCase>, dir: &PathBuf) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Failed to create output dir {:?}: {}", dir, e))?;
    let mut count = 0;
    for tc in &cases {
        let rel = name_to_path(&tc.name);
        let path = dir.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create dir {:?}: {}", parent, e))?;
        }
        let text = serialize_to_http(std::slice::from_ref(tc));
        std::fs::write(&path, &text).map_err(|e| format!("Failed to write {:?}: {}", path, e))?;
        count += 1;
    }
    eprintln!("Wrote {} .http file(s) to {:?}", count, dir);
    Ok(())
}

// ─── Export ───────────────────────────────────────────────────────────────────

fn export_collection(
    name: &str,
    cases: Vec<TestCase>,
    to_format: &str,
    output: Option<&PathBuf>,
    split: bool,
) -> Result<(), String> {
    match to_format {
        "http" => {
            if split {
                let dir = output.ok_or_else(|| "--split requires -o <directory>.".to_string())?;
                return export_http_split(cases, dir);
            }
            let text = serialize_to_http(&cases);
            match output {
                Some(path) => std::fs::write(path, &text)
                    .map_err(|e| format!("Failed to write {:?}: {}", path, e))?,
                None => print!("{}", text),
            }
        },
        "postman" => {
            let json = PostmanAdapter::export(name, &cases, &HashMap::new());
            match output {
                Some(path) => std::fs::write(path, &json)
                    .map_err(|e| format!("Failed to write {:?}: {}", path, e))?,
                None => println!("{}", json),
            }
        },
        "bruno" => {
            let dir = output.ok_or_else(|| {
                "Bruno output requires -o <directory>. Bruno writes one .bru file per request."
                    .to_string()
            })?;
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("Failed to create output dir {:?}: {}", dir, e))?;
            for tc in &cases {
                let bru = BrunoAdapter::export(tc);
                let filename = tc
                    .name
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == '-' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect::<String>();
                let path = dir.join(format!("{}.bru", filename));
                std::fs::write(&path, &bru)
                    .map_err(|e| format!("Failed to write {:?}: {}", path, e))?;
            }
            eprintln!("Wrote {} .bru file(s) to {:?}", cases.len(), dir);
        },
        "curl" => {
            let lines: Vec<String> = cases.iter().map(CurlAdapter::export).collect();
            let text = lines.join("\n\n");
            match output {
                Some(path) => std::fs::write(path, format!("{}\n", text))
                    .map_err(|e| format!("Failed to write {:?}: {}", path, e))?,
                None => println!("{}", text),
            }
        },
        "opencollection" => {
            let json = OpenCollectionAdapter::export(name, &cases, &HashMap::new());
            match output {
                Some(path) => std::fs::write(path, &json)
                    .map_err(|e| format!("Failed to write {:?}: {}", path, e))?,
                None => print!("{}", json),
            }
        },
        "openapi" => {
            let yaml = OpenApiAdapter::export(name, &cases);
            match output {
                Some(path) => std::fs::write(path, &yaml)
                    .map_err(|e| format!("Failed to write {:?}: {}", path, e))?,
                None => print!("{}", yaml),
            }
        },
        _ => return Err(format!("Unknown target format: {}", to_format)),
    }
    Ok(())
}

// ─── Format helpers ───────────────────────────────────────────────────────────

fn format_name(f: CollectionFormat) -> &'static str {
    match f {
        CollectionFormat::Http => "http",
        CollectionFormat::Postman => "postman",
        CollectionFormat::Bruno => "bruno",
        CollectionFormat::OpenCollection => "opencollection",
        CollectionFormat::OpenApi => "openapi",
    }
}

/// Detect output format from `hint` (explicit `--format`), falling back to the
/// output path extension or defaulting for ambiguous cases.
fn detect_output_format(path: &Path, hint: Option<&str>) -> Result<String, String> {
    if let Some(h) = hint {
        return Ok(h.to_lowercase());
    }
    if path.is_dir() || path.extension().is_none() {
        // Directory → Bruno; no extension → require --format
        if path.is_dir() {
            return Ok("bruno".into());
        }
        return Err(format!("Cannot detect output format for {:?}. Specify --format.", path));
    }
    match path.extension().and_then(|e| e.to_str()) {
        Some("http") | Some("rest") => Ok("http".into()),
        Some("json") => Ok("postman".into()),
        Some("sh") => Ok("curl".into()),
        Some("yaml") | Some("yml") => Ok("opencollection".into()),
        Some(ext) => {
            Err(format!("Cannot detect output format from extension '{}'. Specify --format.", ext))
        },
        None => unreachable!("extension-is-none already handled above"),
    }
}

// ─── Convert entrypoint ───────────────────────────────────────────────────────

/// Convert between collection formats: `--from FILE --to FILE [--format FMT]`.
fn convert_file(config: &CliConfig) -> Result<(), String> {
    let input = config.from_file.as_ref().unwrap_or(&config.request_file);
    let output = config.to_file.as_ref().unwrap();

    let from_fmt = detect_format(input)?;
    let to_fmt = detect_output_format(output, config.format.as_deref())?;

    let from_name = format_name(from_fmt);
    if from_name == to_fmt {
        return Err(format!(
            "Input and output formats are both '{}' — nothing to convert.",
            to_fmt
        ));
    }

    if config.verbose {
        eprintln!("Converting {} → {} ...", from_name, to_fmt);
    }
    if config.split && to_fmt != "http" {
        return Err("--split is only supported with --format http.".to_string());
    }

    let (name, cases) = import_collection(input, from_fmt)?;
    export_collection(&name, cases, &to_fmt, Some(output), config.split)
}

fn split_http_raw(content: &str) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut current_lines: Vec<&str> = Vec::new();
    let mut current_name = String::new();

    for line in content.lines() {
        if line.starts_with("###") {
            if !current_lines.is_empty() {
                let chunk = current_lines.join("\n");
                if !chunk.trim().is_empty() {
                    entries.push((current_name.clone(), chunk));
                }
                current_lines.clear();
            }
            current_name = line.trim_start_matches('#').trim().to_string();
            current_lines.push(line);
        } else {
            current_lines.push(line);
        }
    }
    if !current_lines.is_empty() {
        let chunk = current_lines.join("\n");
        if !chunk.trim().is_empty() {
            entries.push((current_name, chunk));
        }
    }
    entries
}

fn cmd_split(args: &SplitArgs) -> Result<(), String> {
    let content = std::fs::read_to_string(&args.file)
        .map_err(|e| format!("Failed to read {:?}: {}", args.file, e))?;
    let entries = split_http_raw(&content);
    if entries.is_empty() {
        return Err(format!("No request entries found in {:?}", args.file));
    }
    std::fs::create_dir_all(&args.output)
        .map_err(|e| format!("Failed to create {:?}: {}", args.output, e))?;
    let mut count = 0;
    for (name, chunk) in &entries {
        let rel = name_to_path(if name.is_empty() { "request" } else { name });
        let path = args.output.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create {:?}: {}", parent, e))?;
        }
        std::fs::write(&path, format!("{}\n", chunk.trim_end()))
            .map_err(|e| format!("Failed to write {:?}: {}", path, e))?;
        count += 1;
    }
    eprintln!("Wrote {} .http file(s) to {:?}", count, args.output);
    Ok(())
}

fn collect_http_files(inputs: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    if inputs.len() == 1 && inputs[0].is_dir() {
        let dir = &inputs[0];
        let mut files: Vec<PathBuf> = Vec::new();
        collect_http_files_recursive(dir, &mut files)
            .map_err(|e| format!("Failed to read dir {:?}: {}", dir, e))?;
        files.sort();
        if files.is_empty() {
            return Err(format!("No .http files found in {:?}", dir));
        }
        Ok(files)
    } else {
        for p in inputs {
            if !p.exists() {
                return Err(format!("File not found: {:?}", p));
            }
        }
        Ok(inputs.to_vec())
    }
}

fn collect_http_files_recursive(dir: &PathBuf, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_http_files_recursive(&path, out)?;
        } else if matches!(path.extension().and_then(|e| e.to_str()), Some("http") | Some("rest")) {
            out.push(path);
        }
    }
    Ok(())
}

fn cmd_merge(args: &MergeArgs) -> Result<(), String> {
    let files = collect_http_files(&args.inputs)?;
    let mut parts: Vec<String> = Vec::with_capacity(files.len());
    for path in &files {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {:?}: {}", path, e))?;
        let trimmed = content.trim_end().to_string();
        if !trimmed.is_empty() {
            parts.push(trimmed);
        }
    }
    let merged = parts.join("\n\n\n");
    match &args.output {
        Some(path) => {
            std::fs::write(path, format!("{}\n", merged))
                .map_err(|e| format!("Failed to write {:?}: {}", path, e))?;
            eprintln!("Merged {} file(s) into {:?}", files.len(), path);
        },
        None => println!("{}", merged),
    }
    Ok(())
}

/// Read a curl command string from `--from FILE`, positional FILE, or stdin.
fn read_curl_input(config: &CliConfig) -> Result<String, String> {
    let path = config.from_file.as_ref().unwrap_or(&config.request_file);
    let path_str = path.to_str().unwrap_or("");
    if path.exists() {
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read {:?}: {}", path, e))
    } else if path_str != "requests.http" {
        // Positional argument treated as an inline curl command string.
        Ok(path_str.to_string())
    } else {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("Failed to read stdin: {}", e))?;
        Ok(buf)
    }
}

/// Run requests from a `--from FILE` source or a curl string (`--format curl`).
async fn run_from_source(
    config: &CliConfig,
    opts: runner::RunOpts<'_>,
) -> Result<CollectionResult, String> {
    if config.format.as_deref() == Some("curl") {
        let input = read_curl_input(config)?;
        let http = CurlAdapter::to_http(&input, None).map_err(|e| e.to_string())?;
        let cases = runner::parse_collection(&http, &config.params, std::path::Path::new("."))?;
        runner::run_test_cases(cases, &config.params, opts).await
    } else {
        let path = config
            .from_file
            .as_ref()
            .ok_or_else(|| "--from FILE is required when not running an .http file".to_string())?;
        let fmt = detect_format(path)?;
        let (_, cases) = import_collection(path, fmt)?;
        runner::run_test_cases(cases, &config.params, opts).await
    }
}

// ─── main ─────────────────────────────────────────────────────────────────────

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Split(args)) => {
            if let Err(e) = cmd_split(args) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
            return;
        },
        Some(Commands::Merge(args)) => {
            if let Err(e) = cmd_merge(args) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
            return;
        },
        None => {},
    }

    let mut config = match CliConfig::from_cli(&cli) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        },
    };

    if let Some(config_path) = &config.config_file.clone() {
        match FileConfig::from_file(config_path) {
            Ok(file_config) => config.merge_with_file_config(&file_config),
            Err(e) => eprintln!("Warning: Failed to load config file: {}", e),
        }
    }

    // Conversion mode: --to FILE
    if config.to_file.is_some() {
        if let Err(e) = convert_file(&config) {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
        return;
    }

    if config.verbose {
        if let Some(ref f) = config.from_file {
            eprintln!("Input file: {:?}", f);
        } else {
            eprintln!("Request file: {:?}", config.request_file);
        }
        eprintln!("Params: {:?}", config.params);
    }

    // Dry-run: parse the collection and list requests without executing them.
    if config.dry_run {
        let cases_result = if config.from_file.is_some() || config.format.as_deref() == Some("curl")
        {
            if config.format.as_deref() == Some("curl") {
                read_curl_input(&config)
                    .and_then(|input| CurlAdapter::to_http(&input, None).map_err(|e| e.to_string()))
                    .and_then(|http| {
                        runner::parse_collection(&http, &config.params, std::path::Path::new("."))
                    })
            } else {
                let path = config.from_file.as_ref().unwrap();
                detect_format(path)
                    .and_then(|fmt| import_collection(path, fmt))
                    .map(|(_, cases)| cases)
            }
        } else {
            std::fs::read_to_string(&config.request_file)
                .map_err(|e| format!("Failed to read {:?}: {}", config.request_file, e))
                .and_then(|content| {
                    let base_dir =
                        config.request_file.parent().unwrap_or(std::path::Path::new("."));
                    runner::parse_collection(&content, &config.params, base_dir)
                })
        };
        match cases_result {
            Ok(mut cases) => {
                if let Some(filter) = config.name_filter.as_deref() {
                    let f = filter.to_lowercase();
                    cases.retain(|tc| tc.name.to_lowercase().contains(&f));
                }
                println!("{} request(s):", cases.len());
                for tc in &cases {
                    println!("  {} {}", tc.request.method, tc.request.url);
                    println!("    name: {}", tc.name);
                }
            },
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            },
        }
        return;
    }

    let rt =
        tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");

    let result = rt.block_on(async {
        let local = tokio::task::LocalSet::new();
        let opts = runner::RunOpts {
            response_file: config.offline.as_deref().and_then(|p| p.to_str()),
            collection_pre_script: config.collection_pre_script.as_deref().and_then(|p| p.to_str()),
            collection_post_script: config
                .collection_post_script
                .as_deref()
                .and_then(|p| p.to_str()),
            save_response: config.out.as_deref().and_then(|p| p.to_str()),
            name_filter: config.name_filter.as_deref(),
        };
        if config.from_file.is_some() || config.format.as_deref() == Some("curl") {
            local.run_until(run_from_source(&config, opts)).await
        } else {
            local.run_until(runner::run_file(&config.request_file, &config.params, opts)).await
        }
    });

    match result {
        Ok(collection) => {
            write_visualizer_files(&collection, &config);
            print_results(&collection, &config);
            if collection.failed > 0 {
                std::process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        },
    }
}

// ─── Output formatting ────────────────────────────────────────────────────────

/// Write `pm.visualizer` HTML files for any results that produced one.
///
/// Defaults to the current directory when `--visualize-dir` is not supplied.
/// Writes one `<sanitized-name>.html` file per test that called `pm.visualizer.set(…)`.
fn write_visualizer_files(collection: &CollectionResult, config: &CliConfig) {
    let has_visualizers = collection.results.iter().any(|r| r.visualizer_html.is_some());
    if !has_visualizers {
        return;
    }
    let default_dir = PathBuf::from(".");
    let dir = config.visualize_dir.as_ref().unwrap_or(&default_dir);
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("warning: could not create --visualize-dir {:?}: {}", dir, e);
        return;
    }
    for r in &collection.results {
        if let Some(ref html) = r.visualizer_html {
            let filename = r
                .name
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>();
            let path = dir.join(format!("{}.html", filename));
            if let Err(e) = std::fs::write(&path, html) {
                eprintln!("warning: could not write visualizer {:?}: {}", path, e);
            } else {
                eprintln!("visualizer: wrote {:?}", path);
            }
        }
    }
}

fn print_results(collection: &CollectionResult, config: &CliConfig) {
    match config.output_format {
        OutputFormat::Json => print_json(collection),
        OutputFormat::Plain => print_plain(collection, config),
        OutputFormat::Pretty => print_pretty(collection, config),
    }
}

fn print_verbose_detail(r: &hello_client::TestResult) {
    println!("      → {} {}", r.request.method, r.request.url);
    for (k, v) in &r.request.headers {
        println!("        {}: {}", k, v);
    }
    if let Some(ref body) = r.request.body {
        let preview = body.chars().take(256).collect::<String>();
        if body.len() > 256 {
            println!("        {} … [{} bytes]", preview, body.len());
        } else {
            println!("        {}", preview);
        }
    }
    if let Some(ref resp) = r.response {
        println!("      ← {}  ({} bytes)", resp.status, resp.body.len());
        for (k, v) in &resp.headers {
            println!("        {}: {}", k, v);
        }
        if !resp.body.is_empty() {
            const BODY_LIMIT: usize = 2048;
            let body = &resp.body;
            if body.len() > BODY_LIMIT {
                let preview: String = body.chars().take(BODY_LIMIT).collect();
                println!("        {} … [{} bytes total]", preview, body.len());
            } else {
                println!("        {}", body);
            }
        }
    }
    for log in &r.logs {
        println!("      log  {}", log);
    }
}

fn print_phase_timings_pretty(t: &hello_client::PhaseTimings) {
    let phases: &[(&str, Option<u64>)] = &[
        ("col-pre", t.collection_pre_ms),
        ("pre", t.pre_ms),
        ("fetch", t.fetch_ms),
        ("post", t.post_ms),
        ("col-post", t.collection_post_ms),
    ];
    let parts: Vec<String> =
        phases.iter().filter_map(|(name, ms)| ms.map(|v| format!("{}: {}ms", name, v))).collect();
    if !parts.is_empty() {
        println!("      timing  {}", parts.join("  ·  "));
    }
}

fn print_pretty(collection: &CollectionResult, config: &CliConfig) {
    println!(
        "\n{} passed, {} failed  ({:.0}ms)\n",
        collection.passed,
        collection.failed,
        collection.total_duration.as_millis(),
    );
    for r in &collection.results {
        let icon = if r.passed { "✓" } else { "✗" };
        println!(
            "  {} {}  ({}ms)",
            icon,
            r.name,
            r.response.as_ref().map_or(0, |h| h.response_time_ms),
        );
        if config.verbose {
            print_verbose_detail(r);
        }
        if config.metrics {
            print_phase_timings_pretty(&r.phase_timings);
        }
        if !r.failures.is_empty() {
            for f in &r.failures {
                println!("      - {}", f);
            }
        }
        if let Some(ref path) = r.output_written {
            println!("      → saved response to {}", path);
        }
        if r.visualizer_html.is_some() {
            println!("      → visualizer HTML written");
        }
    }
    println!();
}

fn print_plain(collection: &CollectionResult, config: &CliConfig) {
    for r in &collection.results {
        let status = if r.passed { "PASS" } else { "FAIL" };
        println!("{} {}", status, r.name);
        for f in &r.failures {
            println!("  {}", f);
        }
        if config.verbose {
            print_verbose_detail(r);
        }
        if config.metrics {
            let t = &r.phase_timings;
            let phases: &[(&str, Option<u64>)] = &[
                ("col-pre", t.collection_pre_ms),
                ("pre", t.pre_ms),
                ("post", t.post_ms),
                ("col-post", t.collection_post_ms),
            ];
            let parts: Vec<String> = phases
                .iter()
                .filter_map(|(name, ms)| ms.map(|v| format!("{}={}ms", name, v)))
                .collect();
            if !parts.is_empty() {
                println!("  timing {}", parts.join(" "));
            }
        }
    }
    println!("passed={} failed={}", collection.passed, collection.failed);
}

fn print_json(collection: &CollectionResult) {
    let results: Vec<serde_json::Value> = collection
        .results
        .iter()
        .map(|r| {
            let t = &r.phase_timings;
            serde_json::json!({
                "name": r.name,
                "passed": r.passed,
                "failures": r.failures,
                "request": {
                    "method": r.request.method,
                    "url": r.request.url,
                    "headers": r.request.headers,
                    "body": r.request.body,
                },
                "status": r.response.as_ref().map(|h| h.status),
                "response_time_ms": r.response.as_ref().map(|h| h.response_time_ms),
                "response_headers": r.response.as_ref().map(|h| &h.headers),
                "response_body": r.response.as_ref().map(|h| &h.body),
                "logs": r.logs,
                "output_written": r.output_written,
                "has_visualizer": r.visualizer_html.is_some(),
                "phase_timings": {
                    "collection_pre_ms": t.collection_pre_ms,
                    "pre_ms": t.pre_ms,
                    "fetch_ms": t.fetch_ms,
                    "post_ms": t.post_ms,
                    "collection_post_ms": t.collection_post_ms,
                },
            })
        })
        .collect();
    let out = serde_json::json!({
        "passed": collection.passed,
        "failed": collection.failed,
        "total_ms": collection.total_duration.as_millis(),
        "results": results,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config_file() {
        let content = r#"
# Configuration file
timeout = 60
verbose = true
base_url = https://api.example.com
param.api_key = secret123
param.user = admin
"#;
        let config = FileConfig::parse(content).unwrap();
        assert_eq!(config.timeout, 60);
        assert!(config.verbose);
        assert_eq!(config.base_url, Some("https://api.example.com".to_string()));
        assert_eq!(config.default_params.get("api_key"), Some(&"secret123".to_string()));
        assert_eq!(config.default_params.get("user"), Some(&"admin".to_string()));
    }

    #[test]
    fn test_parse_config_file_json() {
        let content = r#"{
  "timeout": 60,
  "verbose": true,
  "base_url": "https://api.example.com",
  "param": {
    "api_key": "secret123",
    "user": "admin"
  }
}"#;
        let config = FileConfig::parse(content).unwrap();
        assert_eq!(config.timeout, 60);
        assert!(config.verbose);
        assert_eq!(config.base_url, Some("https://api.example.com".to_string()));
        assert_eq!(config.default_params.get("api_key"), Some(&"secret123".to_string()));
        assert_eq!(config.default_params.get("user"), Some(&"admin".to_string()));
    }

    #[test]
    fn test_parse_config_json_minimal() {
        let content = r#"{ "base_url": "https://httpbin.org" }"#;
        let config = FileConfig::parse(content).unwrap();
        assert_eq!(config.base_url, Some("https://httpbin.org".to_string()));
        assert_eq!(config.timeout, 30);
        assert!(!config.verbose);
        assert!(config.default_params.is_empty());
    }

    #[test]
    fn test_merge_configs() {
        let file_content = r#"
timeout = 60
verbose = true
param.user = file_user
param.token = file_token
"#;
        let mut cli_config = CliConfig {
            params: {
                let mut m = HashMap::new();
                m.insert("user".to_string(), "cli_user".to_string());
                m
            },
            timeout: 45,
            ..CliConfig::default()
        };

        let file_config = FileConfig::parse(file_content).unwrap();
        cli_config.merge_with_file_config(&file_config);

        assert_eq!(cli_config.params.get("user"), Some(&"cli_user".to_string()));
        assert_eq!(cli_config.timeout, 45);
        assert_eq!(cli_config.params.get("token"), Some(&"file_token".to_string()));
        assert!(cli_config.verbose);
    }
}
