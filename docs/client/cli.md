# CLI Reference

`hello_client` ships a binary that runs `.http` request files and prints
results.

## Synopsis

```
hello_client [OPTIONS] [FILE]
hello_client split <FILE> -o <DIR>
hello_client merge <INPUT>... [-o FILE]
```

## Global Options

| Flag                              | Short | Description                                                                       | Default         |
| --------------------------------- | ----- | --------------------------------------------------------------------------------- | --------------- |
| `FILE`                            |       | Input `.http` file to run (positional shorthand)                                  | `requests.http` |
| `--from <FILE>`                   |       | Input file in any supported format (auto-detected or overridden by `--format`)     | —               |
| `--to <FILE\|DIR>`                |       | Output file or directory destination for collection conversion                    | —               |
| `--format <FORMAT>`               |       | Collection format: `http`, `postman`, `bruno`, `curl`, `opencollection`, `openapi` (Sets output format when converting; input hint otherwise) | — |
| `--config <FILE>`                 | `-c`  | Config file path (flat `key = value` or JSON)                                     | —               |
| `--param <KEY=VALUE>`             | `-p`  | Variable substitution (repeatable)                                                | —               |
| `--verbose`                       | `-v`  | Enable verbose output (prints request/response details)                           | false           |
| `--timeout <SECS>`                | `-t`  | Per-request timeout in seconds                                                    | 60              |
| `--output-format <FMT>`           | `-f`  | Results display format: `json`, `plain`, `pretty`                                 | `pretty`        |
| `--split`                         |       | With `--to <DIR>` and `--format http`: write one `.http` file per request          | false           |
| `--visualize-dir <DIR>`           |       | Write `pm.visualizer` HTML files here                                             | —               |
| `--out <FILE\|DIR>`               | `-o`  | Write each response body to FILE (or FILE/`<name>` for a collection)              | —               |
| `--offline <FILE>`                |       | Skip HTTP fetch; load synthetic response from FILE instead                        | —               |
| `--name <PATTERN>`                | `-n`  | Run only requests whose name contains PATTERN (case-insensitive)                  | —               |
| `--dry-run`                       |       | Parse the collection and list request names without sending requests              | false           |
| `--metrics`                       |       | Print per-phase timing (pre-script, fetch, post-script) for each request          | false           |
| `--collection-pre-script <FILE>`  |       | Script to run before every request in the collection                              | —               |
| `--collection-post-script <FILE>` |       | Script to run after every request in the collection                               | —               |

## Subcommands

### `split`

Split a `.http` file into one file per request entry.

```
hello_client split <FILE> -o <DIR>
```

```bash
hello_client split collection.http -o ./split/
```

### `merge`

Merge multiple `.http` files (or a directory) into one.

```
hello_client merge <INPUT>... [-o FILE]
```

```bash
hello_client merge ./split/ -o merged.http
hello_client merge a.http b.http c.http -o all.http
```

## Config File

A config file (passed via `-c`) provides defaults that CLI flags override.

**Flat `key = value` format:**

```ini
timeout = 60
verbose = true
base_url = https://api.example.com

param.token        = my-secret
param.env          = production

collection_pre_script  = ./scripts/setup.js
collection_post_script = ./scripts/teardown.js
```

**JSON format** (file must start with `{`):

```json
{
  "timeout": 60,
  "base_url": "https://api.example.com",
  "param": { "token": "my-secret" },
  "collection_pre_script":  "./scripts/setup.js",
  "collection_post_script": "./scripts/teardown.js"
}
```

`param.<key>` entries are forwarded as template variables for `{{key}}`
substitution in the `.http` file.

`collection_pre_script` / `collection_post_script` apply a JS script to every
request in the collection. CLI `--collection-pre-script` / `--collection-post-script`
flags override these when both are set. These paths can also be declared inside
the `.http` file itself with `### @param collection-pre-script ./setup.js`.

## Output Formats

### `pretty` (default)

Human-readable summary:

```
PASS  get-user (142ms)
FAIL  create-post (89ms)
  - expected status 201, got 400
  - body missing "id" field

Results: 1 passed, 1 failed (231ms total)
```

### `plain`

Same information without ANSI codes; suitable for CI logs.

### `json`

Prints a JSON `CollectionResult` object:

```json
{
  "passed": 1,
  "failed": 1,
  "total_duration_ms": 231,
  "results": [
    { "name": "get-user", "passed": true, "..." },
    { "name": "create-post", "passed": false, "failures": ["..."] }
  ]
}
```

## `--format curl` — Import and run a curl command

Positionally pass a curl command string or stream it via stdin, specifying `--format curl` to import and run it directly.

```bash
# Inline curl command → runs immediately, output in pretty format
hello_client --format curl "curl https://api.example.com/users -H 'Authorization: Bearer tok'"

# Inline curl command → runs immediately, output in JSON format
hello_client --format curl "curl https://api.example.com/users" -f json

# Stream curl command via stdin → convert directly to .http format
pbpaste | hello_client --format curl --to request.http
```

When no file is provided as a source, the positional argument is treated as the inline curl command string. If omitted, stdin is read instead.

## `-o` / `--out` — Persist response bodies

Write each HTTP response body to disk. Per-request `### @param output <path>` annotations in the `.http` file take precedence.

```bash
# Save response of a single request
hello_client request.http -o response.json

# Save response of each test in a collection to a directory
hello_client tests.http -o ./responses/
```

For collections, each response body is written to `<out>/<sanitized-test-name>`.

## Collection Conversion (`--to`)

Convert any supported HTTP test collection format to another format, specifying the output destination using `--to <FILE|DIR>` and format override using `--format <FORMAT>`.

```bash
# Postman -> .http (format auto-detected from .http extension)
hello_client collection.json --to requests.http

# .http -> Postman (format auto-detected from .json extension)
hello_client requests.http --to collection.json

# .http -> Bruno directory (target directory is created; Bruno format chosen because output is a directory)
hello_client requests.http --to ./bruno-collection/

# Bruno directory -> OpenAPI YAML (format auto-detected from .yaml)
hello_client ./bruno-collection/ --to openapi-spec.yaml

# .http -> split .http files (one per request) in a directory
hello_client requests.http --to ./split/ --format http --split
```

Supported formats for conversion: `http`, `postman`, `bruno`, `curl`, `opencollection`, `openapi`.

## Exit Codes

| Code | Meaning                                                     |
| ---- | ----------------------------------------------------------- |
| `0`  | All tests passed                                            |
| `1`  | One or more tests failed, or a parse/runtime error occurred |

## Examples

```bash
# Run all requests in requests.http against a local server
hello_client --param base_url=http://localhost:3000

# Run a specific file with a token, json output format
hello_client auth_tests.http -p token=abc123 -f json

# Use a config file and override timeout
hello_client -c prod.env -t 10

# Verbose mode to see request/response bodies
hello_client -v smoke_tests.http

# Import a curl command from clipboard and convert to .http
pbpaste | hello_client --format curl --to request.http

# Import curl and run immediately (runs the request and prints JSON output)
hello_client --format curl "curl https://httpbin.org/get" -f json

# Save response body to a file
hello_client request.http -o ./response.json

# Save each response in a collection to a directory
hello_client tests.http -o ./snapshots/

# Replay recorded response without making real HTTP calls
hello_client tests.http --offline ./fixtures/response.json

# Collection scripts via config file (no CLI flags needed)
hello_client -c suite.env tests.http

# CLI flags override config file scripts
hello_client -c suite.env tests.http --collection-pre-script ./override_setup.js
```

## Source

- `src/main.rs` — CLI entry point.
- Config parsing in `FileConfig`.
- Output formatting in `print_pretty()`, `print_plain()`, and `print_json()`.
