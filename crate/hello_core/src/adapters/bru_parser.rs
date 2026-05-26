//! nom-based parser for the Bruno `.bru` file format.
//!
//! The `.bru` format consists of named sections delimited by balanced braces:
//!
//! ```text
//! meta {
//!   name: Get User
//!   seq: 1
//! }
//!
//! get {
//!   url: https://api.example.com/users/{{id}}
//! }
//!
//! headers {
//!   Authorization: Bearer {{token}}
//! }
//! ```
//!
//! # Entry points
//!
//! - [`parse_sections`] — split a `.bru` file into `(name, content)` sections
//! - [`parse_kv`] — parse `key: value` lines from a section's body

use nom::{
    Err, IResult, Parser,
    bytes::complete::{tag, take_till, take_while1},
    character::{
        char,
        complete::{line_ending, multispace0, not_line_ending, space0},
    },
    combinator::opt,
    error::{Error, ErrorKind},
    multi::many0,
};

// ─── Public types ─────────────────────────────────────────────────────────────

/// A raw section parsed from a `.bru` file.
#[derive(Debug, Clone)]
pub struct BruSection {
    pub name: String,
    pub content: String,
}

// ─── Core: balanced-brace content ─────────────────────────────────────────────

/// Parse the content between matched outer braces `{ ... }`.
///
/// The input must begin with `{`. Returns the text between the outer `{` and
/// `}`, handling arbitrarily nested `{...}` (e.g. JSON bodies).
fn balanced_braces(input: &str) -> IResult<&str, &str> {
    let (inner, _) = char('{').parse(input)?;

    let mut depth = 1usize;

    for (byte_pos, ch) in inner.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let content = &inner[..byte_pos];
                    let rest = &inner[byte_pos + 1..]; // skip closing '}'
                    return Ok((rest, content));
                }
            },
            _ => {},
        }
    }

    // Unmatched opening brace
    Err(Err::Error(Error::new(input, ErrorKind::Tag)))
}

// ─── Section parser ───────────────────────────────────────────────────────────

/// Parse one section: `name { ... }`.
///
/// Leading whitespace (blank lines, spaces) before the section name is skipped.
/// Returns an error if there is no non-empty name before a `{`.
fn section(input: &str) -> IResult<&str, BruSection> {
    // Skip any blank lines / whitespace between sections.
    let (input, _) = multispace0(input)?;

    if input.is_empty() {
        return Err(Err::Error(Error::new(input, ErrorKind::Eof)));
    }

    // Section name = everything up to `{` on the same line (trimmed).
    let (input, name_raw) =
        take_while1(|c: char| c != '{' && c != '\n' && c != '\r').parse(input)?;
    let name = name_raw.trim();

    if name.is_empty() {
        return Err(Err::Error(Error::new(input, ErrorKind::Verify)));
    }

    // Parse balanced braces; content may span multiple lines.
    let (input, content) = balanced_braces(input)?;

    Ok((
        input,
        BruSection {
            name: name.to_string(),
            content: content.to_string(),
        },
    ))
}

/// Parse a full `.bru` file into a list of raw sections.
///
/// Any text that cannot be parsed as a section is silently ignored (the
/// parser stops collecting at the first unrecognised input and returns what
/// it has so far).
pub fn parse_sections(input: &str) -> Vec<BruSection> {
    many0(section).parse(input).map(|(_, sections)| sections).unwrap_or_default()
}

// ─── KV content parser ────────────────────────────────────────────────────────

/// Parse one line from a section body.
///
/// Returns `None` for blank lines, comment lines (`//`, `#`), and disabled
/// lines (prefixed with `~`). Returns `Some((key, value))` for valid KV pairs.
fn kv_line(input: &str) -> IResult<&str, Option<(String, String)>> {
    // Skip leading indent spaces.
    let (input, _) = space0(input)?;

    // Blank line.
    if input.starts_with('\n') || input.starts_with('\r') {
        let (input, _) = line_ending(input)?;
        return Ok((input, None));
    }

    if input.is_empty() {
        return Err(Err::Error(Error::new(input, ErrorKind::Eof)));
    }

    // Comment lines: `//` or `#`.
    if input.starts_with("//") || input.starts_with('#') {
        let (input, _) = not_line_ending(input)?;
        let (input, _) = opt(line_ending).parse(input)?;
        return Ok((input, None));
    }

    // Disabled lines: prefixed with `~`.
    if input.starts_with('~') {
        let (input, _) = not_line_ending(input)?;
        let (input, _) = opt(line_ending).parse(input)?;
        return Ok((input, None));
    }

    // KV pair: `key: value`
    // Key = everything up to the first `:` (or end of line).
    let (input, key_raw) = take_till(|c: char| c == ':' || c == '\n' || c == '\r').parse(input)?;
    let key = key_raw.trim();

    // If no `:` follows, skip the line.
    if !input.starts_with(':') || key.is_empty() {
        let (input, _) = not_line_ending(input)?;
        let (input, _) = opt(line_ending).parse(input)?;
        return Ok((input, None));
    }

    let (input, _) = tag(":").parse(input)?;
    let (input, _) = space0(input)?;
    let (input, value_raw) = not_line_ending(input)?;
    let (input, _) = opt(line_ending).parse(input)?;

    Ok((input, Some((key.to_string(), value_raw.trim().to_string()))))
}

/// Parse `key: value` lines from a section's raw body content.
///
/// Skips blank lines, comment lines (`//`, `#`), and disabled lines (`~`).
/// Returns only the active, enabled key-value pairs.
pub fn parse_kv(content: &str) -> Vec<(String, String)> {
    many0(kv_line)
        .parse(content)
        .map(|(_, opts)| opts.into_iter().flatten().collect())
        .unwrap_or_default()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_section() {
        let input = "meta {\n  name: Test\n  seq: 1\n}\n";
        let sections = parse_sections(input);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].name, "meta");
        assert!(sections[0].content.contains("name: Test"));
    }

    #[test]
    fn section_with_colon_in_name() {
        let input = "body:json {\n  {\"key\": \"value\"}\n}\n";
        let sections = parse_sections(input);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].name, "body:json");
    }

    #[test]
    fn nested_braces_in_content() {
        let input = "body:json {\n  {\"a\": {\"b\": 1}}\n}\n";
        let sections = parse_sections(input);
        assert_eq!(sections.len(), 1);
        assert!(sections[0].content.contains("\"b\": 1"));
    }

    #[test]
    fn multiple_sections() {
        let input = "meta {\n  name: A\n}\n\nget {\n  url: https://example.com\n}\n";
        let sections = parse_sections(input);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].name, "meta");
        assert_eq!(sections[1].name, "get");
    }

    #[test]
    fn kv_skips_comments_and_disabled() {
        let content = "  // a comment\n  # another comment\n  ~disabled: val\n  active: yes\n";
        let kv = parse_kv(content);
        assert_eq!(kv, vec![("active".to_string(), "yes".to_string())]);
    }

    #[test]
    fn kv_value_with_colon() {
        // URL values contain ':' — should not split on the second colon
        let content = "  url: https://api.example.com/path\n";
        let kv = parse_kv(content);
        assert_eq!(kv.len(), 1);
        assert_eq!(kv[0].0, "url");
        assert_eq!(kv[0].1, "https://api.example.com/path");
    }

    #[test]
    fn balanced_braces_nested() {
        let input = "{ outer { inner } end }rest";
        let (rest, content) = balanced_braces(input).unwrap();
        assert_eq!(content, " outer { inner } end ");
        assert_eq!(rest, "rest");
    }

    #[test]
    fn balanced_braces_unmatched_returns_error() {
        let input = "{ unclosed";
        assert!(balanced_braces(input).is_err());
    }
}
