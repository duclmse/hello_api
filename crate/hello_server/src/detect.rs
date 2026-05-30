//! Format auto-detection for spec files.

use std::path::Path;

use anyhow::bail;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    HttpFile,
    Bruno,
    OpenApi,
    Postman,
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::HttpFile => "http",
            Self::Bruno => "bruno",
            Self::OpenApi => "openapi",
            Self::Postman => "postman",
        })
    }
}

/// Determine the spec format.
///
/// Resolution order:
/// 1. `hint` (from `--format` CLI flag) overrides everything.
/// 2. File extension.
/// 3. Content sniffing for `.yaml` / `.json`.
/// 4. Directory → Bruno.
pub fn detect_format(path: &Path, hint: Option<&str>) -> anyhow::Result<Format> {
    if let Some(h) = hint {
        return match h.to_lowercase().as_str() {
            "http" => Ok(Format::HttpFile),
            "bruno" => Ok(Format::Bruno),
            "openapi" | "swagger" => Ok(Format::OpenApi),
            "postman" => Ok(Format::Postman),
            other => bail!("unknown format hint: {other}; valid: http, bruno, openapi, postman"),
        };
    }

    if path.is_dir() {
        return Ok(Format::Bruno);
    }

    match path.extension().and_then(|e| e.to_str()) {
        Some("http") => return Ok(Format::HttpFile),
        Some("bru") => return Ok(Format::Bruno),
        Some("yaml") | Some("yml") => {
            let head = read_head(path)?;
            if sniff_openapi(&head) {
                return Ok(Format::OpenApi);
            }
            bail!("could not identify YAML format; try --format openapi");
        },
        Some("json") => {
            let head = read_head(path)?;
            if sniff_openapi(&head) {
                return Ok(Format::OpenApi);
            }
            if sniff_postman(&head) {
                return Ok(Format::Postman);
            }
            bail!("could not identify JSON format; try --format openapi or --format postman");
        },
        other => bail!(
            "unrecognised extension {:?}; use --format to specify",
            other.unwrap_or("(none)")
        ),
    }
}

fn read_head(path: &Path) -> anyhow::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; 4096];
    let n = f.read(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
}

fn sniff_openapi(head: &str) -> bool {
    head.contains("openapi:") || head.contains("swagger:") || head.contains("\"openapi\"") || head.contains("\"swagger\"")
}

fn sniff_postman(head: &str) -> bool {
    (head.contains("\"info\"") && head.contains("\"schema\""))
        && (head.contains("postman") || head.contains("\"item\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn tmp(ext: &str, content: &str) -> NamedTempFile {
        let mut f = tempfile::Builder::new().suffix(ext).tempfile().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn hint_overrides_extension() {
        let f = tmp(".json", "{}");
        assert_eq!(detect_format(f.path(), Some("http")).unwrap(), Format::HttpFile);
    }

    #[test]
    fn http_extension() {
        let f = tmp(".http", "GET /\n");
        assert_eq!(detect_format(f.path(), None).unwrap(), Format::HttpFile);
    }

    #[test]
    fn yaml_openapi() {
        let f = tmp(".yaml", "openapi: 3.0.0\npaths: {}");
        assert_eq!(detect_format(f.path(), None).unwrap(), Format::OpenApi);
    }

    #[test]
    fn json_postman() {
        let f = tmp(
            ".json",
            r#"{"info":{"name":"x","schema":"https://schema.getpostman.com/json/collection/v2.1.0/"},"item":[]}"#,
        );
        assert_eq!(detect_format(f.path(), None).unwrap(), Format::Postman);
    }

    #[test]
    fn directory_is_bruno() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect_format(dir.path(), None).unwrap(), Format::Bruno);
    }
}
