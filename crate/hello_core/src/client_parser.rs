use nom::multi::many1;
use nom::{
    IResult, Parser,
    branch::alt,
    bytes::complete::{tag, take_until, take_while, take_while1},
    character::{
        anychar, char,
        complete::{line_ending, not_line_ending, space0, space1},
    },
    combinator::{map, map_res, opt, peek, recognize, value},
    error::Error,
    multi::many0,
    sequence::{delimited, pair, preceded, terminated},
};
use std::{collections::HashMap, str};

use crate::http_request::{
    Body, FormField, FormFieldValue, HttpRequest, MultipartPart, PartContent, RequestEntry,
    RequestLine, Script, Url, UrlSegment,
};
use crate::metadata::metadata;

// Parse variable placeholder like {{host}} or {{a_value}}
fn variable(input: &str) -> IResult<&str, UrlSegment<'_>> {
    map(
        delimited(
            preceded(tag("{{"), space0),
            take_while1(|c: char| c.is_alphanumeric() || c == '_'),
            terminated(space0, tag("}}")),
        ),
        UrlSegment::Variable,
    )
    .parse(input)
}

// Parse HTTP method (GET, POST, PUT, etc.)
fn method(input: &str) -> IResult<&str, &str> {
    take_while1(|c: char| c.is_ascii_alphabetic()).parse(input)
}

// Parse host part (before first /)
fn host_part(input: &'_ str) -> IResult<&'_ str, Vec<UrlSegment<'_>>> {
    terminated(
        many0(alt((
            variable,
            map(
                take_while1(|c: char| {
                    c != '/' && c != '?' && c != '{' && c != ' ' && c != '\n' && c != '\r'
                }),
                |s: &str| UrlSegment::Text(s),
            ),
        ))),
        opt(line_ending),
    )
    .parse(input)
}

// Parse path (starts with /)
fn endpoint(input: &'_ str) -> IResult<&'_ str, Vec<UrlSegment<'_>>> {
    let (input, path) = many0(alt((
        variable,
        map(
            take_while1(|c: char| c != '?' && c != '{' && c != ' ' && c != '\n' && c != '\r'),
            |s: &str| UrlSegment::Text(s),
        ),
    )))
    .parse(input)?;

    Ok((input, path))
}

// Parse a single query parameter with optional variables
fn query_param(input: &'_ str) -> IResult<&'_ str, Vec<UrlSegment<'_>>> {
    let (input, sep) = map(alt((tag("?"), tag("&"))), UrlSegment::Text).parse(input)?;

    // Parse key (can contain variables)
    let (input, key) = many1(alt((
        variable,
        map(take_while1(|c: char| c != '=' && c != '{' && c != '\n' && c != '\r'), |s: &str| {
            UrlSegment::Text(s)
        }),
    )))
    .parse(input)?;

    // Parse value (can contain variables)
    let (input, value) = opt((
        map(delimited(space0, tag("="), space0), UrlSegment::Text),
        many1(alt((
            variable,
            map(
                take_while1(|c: char| c != '&' && c != '{' && c != '\n' && c != '\r' && c != ' '),
                |s: &str| UrlSegment::Text(s),
            ),
        ))),
    ))
    .parse(input)?;

    let params = vec![sep] //
        .into_iter()
        .chain(key)
        .chain(value.map_or(vec![], |(eq, val)| vec![eq].into_iter().chain(val).collect()))
        .collect();
    Ok((input, params))
}

// Parse a comment line: optional whitespace + (# or //) + rest of line + newline.
// Comment lines are silently discarded from query-param continuation blocks.
fn comment_line(input: &str) -> IResult<&str, ()> {
    value((), (space0, alt((tag("//"), tag("#"))), not_line_ending, line_ending)).parse(input)
}

// Parse query parameters (can be on multiple lines).
// Lines starting with `#` or `//` (after optional whitespace) are treated as
// comments and skipped.
//
// Handles IntelliJ-style multiline URLs where query params appear on the NEXT
// line after the path:
//
//   GET https://api.example.com/users
//       ?page=1
//       &limit=20
//
// The newline between the path and the first `?` is consumed only when the
// following line actually starts a query param (`peek` ensures backtracking
// if it doesn't, so non-continuation lines are left untouched).
fn query_params(input: &str) -> IResult<&str, Vec<UrlSegment<'_>>> {
    // Consume the line-break before a continuation query-param block, if present.
    let (input, _) =
        opt(terminated(line_ending, peek(preceded(space0, alt((tag("?"), tag("&")))))))
            .parse(input)?;

    map(
        many0(alt((
            value(vec![], comment_line),
            map(terminated(preceded(space0, query_param), opt(line_ending)), |v| v),
        ))),
        |s| s.into_iter().flatten().collect::<_>(),
    )
    .parse(input)
}

// Parse HTTP version
fn http_version(input: &str) -> IResult<&str, Option<&str>> {
    opt(preceded(
        space1,
        recognize((tag("HTTP/"), take_while1(|c: char| c.is_ascii_digit() || c == '.'))),
    ))
    .parse(input)
}

fn url<'a>(input: &'a str) -> IResult<&'a str, Url<'a>> {
    map((host_part, endpoint, query_params), |(host, path, query_params)| Url::Segments {
        host,
        path,
        query_params,
    })
    .parse(input)
}

// Parse request line (can span multiple lines with query params)
pub fn request_line(input: &'_ str) -> IResult<&'_ str, RequestLine<'_>> {
    let (input, (method, _, url, http_version, _)) =
        (method, space1, url, http_version, many0(line_ending)).parse(input)?;
    let request_line = RequestLine {
        method,
        url,
        http_version,
    };
    Ok((input, request_line))
}

// Parse a single header — key never crosses a line boundary.
// Fails immediately on a blank line so that `many0(header)` stops at the
// headers/body separator and does not consume body content as spurious headers.
fn header(input: &str) -> IResult<&str, Option<(&str, &str)>> {
    // Blank line → not a header; caller's opt(line_ending) will consume it.
    if input.starts_with('\n') || input.starts_with("\r\n") || input.starts_with('\r') {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Char,
        )));
    }
    let (input, header) = terminated(
        opt(pair(
            terminated(take_while1(|c: char| c != ':' && c != '\n' && c != '\r'), tag(": ")),
            not_line_ending,
        )),
        line_ending,
    )
    .parse(input)?;
    Ok((input, header))
}

// Parse all headers
fn headers(input: &str) -> IResult<&str, HashMap<&str, &str>> {
    map_res(
        terminated(many0(header), opt(line_ending)),
        |vec| -> Result<HashMap<&str, &str>, Error<&str>> {
            let mut map = HashMap::with_capacity(vec.len());
            for (key, value) in vec.into_iter().flatten() {
                map.insert(key, value);
            }
            Ok(map)
        },
    )
    .parse(input)
}

fn is_post_script_line(input: &str) -> IResult<&str, ()> {
    let (input, _) = peek(char('>')).parse(input)?;
    Ok((input, ()))
}

fn is_entry_separator(input: &str) -> IResult<&str, ()> {
    value((), peek(tag("###"))).parse(input)
}

// Parse the optional HTTP body
fn body(input: &str) -> IResult<&str, Option<String>> {
    let mut result = String::new();
    let mut remaining = input;

    loop {
        // Stop at post-script marker or next entry separator
        if is_post_script_line(remaining).is_ok() || is_entry_separator(remaining).is_ok() {
            break;
        }

        let line = alt((
            terminated::<_, _, nom::error::Error<&str>, _, _>(
                take_while(|c: char| c != '\n' && c != '\r'),
                line_ending,
            ),
            recognize(many0(anychar)),
        ))
        .parse(remaining);
        match line {
            Ok((rest, content)) => {
                if content.is_empty() && rest.is_empty() {
                    break;
                }
                result.push_str(content);
                if !rest.is_empty() {
                    result.push('\n');
                }
                remaining = rest;
            },
            Err(_) => break,
        }
    }

    let body = result.trim();
    if body.is_empty() {
        Ok((remaining, None))
    } else {
        Ok((remaining, Some(body.to_owned())))
    }
}

// Parse a complete HTTP request
pub fn http_request(input: &'_ str) -> IResult<&'_ str, HttpRequest<'_>> {
    let (input, (request_line, headers, raw_body)) = (request_line, headers, body).parse(input)?;

    let parsed_body = raw_body.map(interpret_body);
    let body = coerce_body_by_content_type(&headers, parsed_body);

    Ok((
        input,
        HttpRequest {
            request_line,
            headers,
            body,
        },
    ))
}

/// Classify a raw body string into the appropriate [`Body`] variant.
fn interpret_body(raw: String) -> Body {
    let trimmed = raw.trim();
    // Single-line `< path` → file reference body
    if !trimmed.contains('\n')
        && let Some(path) = trimmed.strip_prefix('<')
    {
        return Body::File(path.trim().to_string());
    }
    // First non-empty line starts with `--` → try multipart
    if trimmed.starts_with("--")
        && let Some(mp) = parse_multipart_body(trimmed)
    {
        return mp;
    }
    // `[form]` block → multipart/form-data shorthand
    if trimmed.starts_with("[form]") {
        return parse_form_block(trimmed);
    }
    // `[form-urlencoded]` block → application/x-www-form-urlencoded shorthand
    if trimmed.starts_with("[form-urlencoded]") {
        return parse_form_urlencoded_block(trimmed);
    }
    Body::Raw(raw)
}

/// Parse a multipart/form-data body (the `--boundary` block format).
///
/// Returns `None` if the text doesn't look like valid multipart, so the
/// caller can fall back to `Body::Raw`.
fn parse_multipart_body(raw: &str) -> Option<Body> {
    let lines: Vec<&str> = raw.lines().collect();

    // First line must be `--{boundary}`
    let boundary = lines.first()?.trim().strip_prefix("--")?.trim().to_string();
    if boundary.is_empty() || boundary.ends_with("--") {
        return None;
    }

    let part_marker = format!("--{}", boundary);
    let end_marker = format!("--{}--", boundary);

    let mut parts: Vec<MultipartPart> = Vec::new();
    let mut i = 1usize; // skip the first boundary line

    loop {
        if i >= lines.len() {
            break;
        }
        let l = lines[i].trim_end_matches('\r').trim();
        if l == end_marker {
            break;
        }

        // Parse this part's headers (up to the first blank line).
        let mut headers: Vec<(String, String)> = Vec::new();
        while i < lines.len() {
            let l = lines[i].trim_end_matches('\r');
            if l.trim().is_empty() {
                i += 1;
                break;
            }
            if let Some(colon) = l.find(": ") {
                headers.push((l[..colon].to_string(), l[colon + 2..].to_string()));
            }
            i += 1;
        }

        // Parse this part's content (until the next boundary or end marker).
        let mut content_lines: Vec<&str> = Vec::new();
        while i < lines.len() {
            let l = lines[i].trim_end_matches('\r');
            if l.trim() == part_marker || l.trim() == end_marker {
                break;
            }
            content_lines.push(l);
            i += 1;
        }

        let content_str = content_lines.join("\n").trim().to_string();
        let content = if let Some(path) = content_str.strip_prefix("< ") {
            PartContent::File(path.trim().to_string())
        } else {
            PartContent::Text(content_str)
        };

        parts.push(MultipartPart { headers, content });

        // Advance past the part separator (if present) before the next part.
        if i < lines.len() && lines[i].trim_end_matches('\r').trim() == part_marker {
            i += 1;
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(Body::Multipart { boundary, parts })
    }
}

/// Split `value[; attr1=v1[; attr2=v2]]` into `(value, attrs)`.
///
/// The first `; ` (semicolon-space) marks the boundary between the value and its
/// attributes. Attribute values may be quoted with `"`.
fn split_value_and_attrs(s: &str) -> (String, Vec<(String, String)>) {
    let Some(semi) = s.find("; ") else {
        return (s.to_string(), vec![]);
    };
    let value = s[..semi].to_string();
    let attrs = s[semi + 2..]
        .split("; ")
        .filter_map(|part| {
            let eq = part.find('=')?;
            let key = part[..eq].trim().to_string();
            let val = part[eq + 1..].trim().trim_matches('"').to_string();
            if key.is_empty() { None } else { Some((key, val)) }
        })
        .collect();
    (value, attrs)
}

/// Parse `key = value[; attr=v]*` lines into [`FormField`] entries.
/// Blank lines and lines starting with `#` are skipped.
fn parse_form_field_lines(text: &str) -> Vec<FormField> {
    let mut fields: Vec<FormField> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(eq) = line.find(" = ") {
            let name = line[..eq].trim().to_string();
            let rest = line[eq + 3..].trim();
            let (val_str, attrs) = split_value_and_attrs(rest);
            let value = if let Some(path) = val_str.strip_prefix("< ") {
                FormFieldValue::File(path.trim().to_string())
            } else {
                FormFieldValue::Text(val_str)
            };
            if !name.is_empty() {
                fields.push(FormField { name, value, attrs });
            }
        }
    }
    fields
}

/// Parse `key = value` lines into `(name, value)` tuples for `FormUrlEncoded`.
fn parse_urlencoded_field_lines(text: &str) -> Vec<(String, String)> {
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(eq) = line.find(" = ") {
            let name = line[..eq].trim().to_string();
            let value = line[eq + 3..].trim().to_string();
            if !name.is_empty() {
                fields.push((name, value));
            }
        }
    }
    fields
}

/// Parse a `[form]` shorthand body into `Body::Form`.
fn parse_form_block(raw: &str) -> Body {
    let text = raw.lines().skip(1).collect::<Vec<_>>().join("\n");
    Body::Form { fields: parse_form_field_lines(&text) }
}

/// Parse a `[form-urlencoded]` shorthand body into `Body::FormUrlEncoded`.
fn parse_form_urlencoded_block(raw: &str) -> Body {
    let text = raw.lines().skip(1).collect::<Vec<_>>().join("\n");
    Body::FormUrlEncoded { fields: parse_urlencoded_field_lines(&text) }
}

/// Returns true when `text` contains at least one `key = value` form field line.
fn looks_like_form_fields(text: &str) -> bool {
    text.lines().any(|line| {
        let t = line.trim();
        !t.is_empty() && !t.starts_with('#') && t.contains(" = ")
    })
}

/// Returns true when `text` looks like YAML-style multipart fields using colon
/// separators (`key: value` or `key:` array headers).
///
/// Distinguished from JSON/XML bodies and from `key = value` form fields.
fn looks_like_yaml_multipart(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') || trimmed.starts_with('<') {
        return false;
    }
    trimmed.lines().any(|line| {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with('-') {
            return false;
        }
        if let Some(pos) = t.find(':') {
            let key = &t[..pos];
            !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '[' || c == ']')
        } else {
            false
        }
    })
}

/// Parse a YAML-style multipart body into [`FormField`] entries.
///
/// Two patterns are supported:
/// - `key: value` — simple text or `< file` field.
/// - `key:` followed by indented list items (`    - prop: val …`) — produces one
///   `FormField` per list item; the `file` sub-property (or first sub-property)
///   becomes the field value and remaining sub-properties become attrs.
fn parse_yaml_multipart_body(text: &str) -> Vec<FormField> {
    let mut fields: Vec<FormField> = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        i += 1;

        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            continue;
        }

        let indent = line.len() - line.trim_start().len();

        // Bare array-field header: `name:` with no value after the colon.
        if !trimmed.contains(": ") && trimmed.ends_with(':') {
            let name = trimmed[..trimmed.len() - 1].trim().to_string();
            if name.is_empty() {
                continue;
            }
            // Collect list items indented deeper than this header line.
            while i < lines.len() {
                let item_line = lines[i];
                let item_trimmed = item_line.trim();
                let item_indent = item_line.len() - item_line.trim_start().len();
                if item_indent <= indent || !item_trimmed.starts_with("- ") {
                    break;
                }
                i += 1;

                // Parse the first sub-property from the `- key: value` line.
                let first_str = item_trimmed.strip_prefix("- ").unwrap_or("").trim();
                let mut sub_props: Vec<(String, String)> = Vec::new();
                if let Some(c) = first_str.find(": ") {
                    sub_props.push((
                        first_str[..c].trim().to_string(),
                        first_str[c + 2..].trim().to_string(),
                    ));
                } else if !first_str.contains(": ") && first_str.ends_with(':') {
                    sub_props.push((
                        first_str[..first_str.len() - 1].trim().to_string(),
                        String::new(),
                    ));
                }

                // Parse continuation sub-properties aligned past the `- `.
                while i < lines.len() {
                    let sub_line = lines[i];
                    let sub_trimmed = sub_line.trim();
                    let sub_indent = sub_line.len() - sub_line.trim_start().len();
                    if sub_trimmed.is_empty()
                        || sub_trimmed.starts_with("- ")
                        || sub_indent <= item_indent
                    {
                        break;
                    }
                    i += 1;
                    if let Some(c) = sub_trimmed.find(": ") {
                        sub_props.push((
                            sub_trimmed[..c].trim().to_string(),
                            sub_trimmed[c + 2..].trim().to_string(),
                        ));
                    }
                }

                // The `file` sub-property (or the first sub-property) becomes the value;
                // all remaining sub-properties become attrs.
                let value_key = sub_props
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("file"))
                    .map(|(k, _)| k.clone())
                    .or_else(|| sub_props.first().map(|(k, _)| k.clone()));

                if let Some(ref vk) = value_key {
                    let val_str = sub_props
                        .iter()
                        .find(|(k, _)| k == vk)
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default();
                    let form_value = if let Some(path) = val_str.strip_prefix("< ") {
                        FormFieldValue::File(path.trim().to_string())
                    } else {
                        FormFieldValue::Text(val_str)
                    };
                    let attrs: Vec<(String, String)> =
                        sub_props.into_iter().filter(|(k, _)| k != vk).collect();
                    fields.push(FormField { name: name.clone(), value: form_value, attrs });
                }
            }
        } else if let Some(colon) = trimmed.find(": ") {
            // Simple `key: value` field.
            let name = trimmed[..colon].trim().to_string();
            let value_str = trimmed[colon + 2..].trim();
            let value = if let Some(path) = value_str.strip_prefix("< ") {
                FormFieldValue::File(path.trim().to_string())
            } else {
                FormFieldValue::Text(value_str.to_string())
            };
            fields.push(FormField { name, value, attrs: vec![] });
        }
    }

    fields
}

/// When `Content-Type: multipart/form-data` (no boundary) or
/// `Content-Type: application/x-www-form-urlencoded` (or the shorthand
/// `form-urlencoded`) is present and the body looks like field lines,
/// coerce `Body::Raw` to the appropriate shorthand variant.
fn coerce_body_by_content_type(headers: &HashMap<&str, &str>, body: Option<Body>) -> Option<Body> {
    let ct = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.to_ascii_lowercase());
    let Some(ct) = ct else { return body; };

    match body {
        Some(Body::Raw(raw)) => {
            let trimmed = raw.trim();
            if ct.starts_with("multipart/form-data") && !ct.contains("boundary=") {
                if looks_like_form_fields(trimmed) {
                    Some(Body::Form { fields: parse_form_field_lines(trimmed) })
                } else if looks_like_yaml_multipart(trimmed) {
                    Some(Body::Form { fields: parse_yaml_multipart_body(trimmed) })
                } else {
                    Some(Body::Raw(raw))
                }
            } else if ct.starts_with("application/x-www-form-urlencoded")
                || ct == "form-urlencoded"
            {
                if looks_like_form_fields(trimmed) {
                    Some(Body::FormUrlEncoded { fields: parse_urlencoded_field_lines(trimmed) })
                } else {
                    Some(Body::Raw(raw))
                }
            } else {
                Some(Body::Raw(raw))
            }
        },
        other => other,
    }
}

// Parse pre-request inline script block: < {% ... %}
fn parse_pre_inline_script(input: &'_ str) -> IResult<&'_ str, Script<'_>> {
    let (input, (_, _, script)) =
        (tag("<"), space0, delimited(tag("{%"), take_until("%}"), tag("%}"))).parse(input)?;
    Ok((input, Script::Inline(script.trim())))
}

// Parse pre-request file script reference: < filename.js
fn parse_pre_file_script(input: &'_ str) -> IResult<&'_ str, Script<'_>> {
    let (input, (_, _, file_name)) = (tag("<"), space0, not_line_ending).parse(input)?;
    Ok((input, Script::File(file_name.trim())))
}

// Parse post-response inline script block: > {% ... %}
fn parse_inline_script(input: &'_ str) -> IResult<&'_ str, Script<'_>> {
    let (input, (_, _, script)) =
        (tag(">"), space0, delimited(tag("{%"), take_until("%}"), tag("%}"))).parse(input)?;
    Ok((input, Script::Inline(script.trim())))
}

// Parse post-response file script reference: > filename.js
fn parse_file_script(input: &'_ str) -> IResult<&'_ str, Script<'_>> {
    let (input, (_, _, file_name)) = (tag(">"), space0, not_line_ending).parse(input)?;
    Ok((input, Script::File(file_name.trim())))
}

// Parse pre-request script if present (< notation)
fn pre_script(input: &'_ str) -> IResult<&'_ str, Option<Script<'_>>> {
    opt(terminated(
        alt((parse_pre_inline_script, parse_pre_file_script)), //
        line_ending,
    ))
    .parse(input)
}

// Parse post-response script if present (> notation)
fn post_script(input: &'_ str) -> IResult<&'_ str, Option<Script<'_>>> {
    opt(terminated(
        alt((parse_inline_script, parse_file_script)), //
        line_ending,
    ))
    .parse(input)
}

fn empty_line(input: &str) -> IResult<&str, ()> {
    value((), pair(space0, line_ending)).parse(input)
}

// Parse a complete request entry (metadata + request + scripts)
fn request_entry(input: &'_ str) -> IResult<&'_ str, RequestEntry<'_>> {
    let (input, (_, metadata, _, pre_script, _, request, post_script, _)) = (
        many0(empty_line),
        metadata,
        many0(empty_line),
        pre_script,
        many0(empty_line),
        http_request,
        post_script,
        many0(empty_line),
    )
        .parse(input)?;

    Ok((
        input,
        RequestEntry {
            metadata,
            pre_script,
            request,
            post_script,
        },
    ))
}

// Parse a collection of request entries
pub fn request_collection(input: &'_ str) -> IResult<&'_ str, Vec<RequestEntry<'_>>> {
    many0(request_entry).parse(input)
}

impl<'a> From<&'a str> for Url<'a> {
    fn from(val: &'a str) -> Self {
        match url.parse(val) {
            Ok(_) => todo!(),
            Err(_) => todo!(),
        }
    }
}

// Unit tests
#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, fs::File};

    #[test]
    pub fn test_comment_param() {
        println!("starting...");

        let (_input, req_line) = match request_line
            .parse("GET {{ host }}/something?o=0\n#    &{{ a }}=b&{{ b }}\n    HTTP/1.1")
        {
            Ok(it) => it,
            Err(err) => {
                println!("err = {}", err);
                return;
            },
        };
        println!("m={};\nep={:?};\nv={:?}", req_line.method, req_line.url, req_line.http_version);
    }

    #[test]
    #[ignore]
    fn main() {
        println!("starting...");
        let mut contents: Vec<u8> = Vec::new();

        {
            use std::io::Read;
            let filename = env::args().nth(2).expect("File to read");
            println!("Reading file '{}'", filename);
            let mut file = File::open(filename).expect("Failed to open file");
            let size = file.read_to_end(&mut contents).unwrap();
            println!("Read {} bytes from file", size);
        }

        let buf = &contents[..];
        match request_collection(str::from_utf8(buf).unwrap()) {
            Ok((_, entries)) => {
                println!("Found {} request entries", entries.len());

                for (i, entry) in entries.iter().enumerate() {
                    println!("\nRequest Entry #{}", i + 1);
                    println!(
                        "Request: {} {:?}",
                        entry.request.request_line.method, entry.request.request_line.url
                    );
                    println!("Headers: {} headers", entry.request.headers.len());
                }
            },
            Err(e) => println!("Error parsing collection: {:?}", e),
        }
    }

    #[test]
    fn test_parse_metadata() {
        let input = "### Get User Profile\n### #auth #json\n### This is a description";
        let (_, metadata) = metadata(input).unwrap();
        println!("{:?}", metadata);
        assert_eq!(metadata.description.first(), Some(&"Get User Profile"));
    }

    #[test]
    fn test_parse_inline_script() {
        let input = "> {%\n  const token = 'abc';\n%}";
        let (_, script) = parse_inline_script(input).unwrap();

        match script {
            Script::Inline(content) => {
                assert_eq!(content, "const token = 'abc';")
            },
            _ => panic!("Expected inline script"),
        }
    }

    #[test]
    fn test_parse_file_script() {
        let input = "> my-script.js\n";
        let (_, script) = parse_file_script(input).unwrap();

        match script {
            Script::File(filename) => assert_eq!(filename, "my-script.js"),
            _ => panic!("Expected file script"),
        }
    }

    #[test]
    fn test_parse_request_entry_with_inline_scripts() {
        let input = r#"### Get User Info #auth Get user information

< {% const token = getToken(); %}

GET /api/{{user}} HTTP/1.1
Host: example.com

> {% console.log(response); %}
"#;

        let (_, entry) = request_entry(input).unwrap();

        assert_eq!(entry.request.request_line.method, "GET");
        assert_eq!(entry.request.request_line.get_verbatim_endpoint(), "/api/{{user}}");

        match &entry.pre_script {
            Some(Script::Inline(content)) => {
                assert_eq!(content, &"const token = getToken();")
            },
            _ => panic!("Expected inline pre-script"),
        }

        match &entry.post_script {
            Some(Script::Inline(content)) => {
                assert_eq!(content, &"console.log(response);")
            },
            _ => panic!("Expected inline post-script"),
        }
    }

    #[test]
    fn test_parse_request_entry_with_file_scripts() {
        let input = r#"### Get User Info #auth
### Get user information

< setup.js

GET /api/user HTTP/1.1
Host: example.com

> handle-response.js
"#;

        let (_, entry) = request_entry(input).unwrap();

        match &entry.pre_script {
            Some(Script::File(filename)) => assert_eq!(filename, &"setup.js"),
            _ => panic!("Expected file pre-script"),
        }

        match &entry.post_script {
            Some(Script::File(filename)) => {
                assert_eq!(filename, &"handle-response.js")
            },
            _ => panic!("Expected file post-script"),
        }
    }

    // ── Headers ───────────────────────────────────────────────────────────────

    #[test]
    fn parse_single_header() {
        let input = "Authorization: Bearer token123\n\n";
        let (_, h) = headers(input).unwrap();
        assert_eq!(h.get("Authorization"), Some(&"Bearer token123"));
    }

    #[test]
    fn parse_multiple_headers() {
        let input = "Content-Type: application/json\nAccept: */*\n\n";
        let (_, h) = headers(input).unwrap();
        assert_eq!(h.get("Content-Type"), Some(&"application/json"));
        assert_eq!(h.get("Accept"), Some(&"*/*"));
    }

    #[test]
    fn parse_empty_headers() {
        let input = "\n";
        let (_, h) = headers(input).unwrap();
        assert!(h.is_empty());
    }

    // ── Body ──────────────────────────────────────────────────────────────────

    #[test]
    fn parse_plain_body() {
        let input = "hello world\n";
        let (_, b) = body(input).unwrap();
        assert_eq!(b.as_deref(), Some("hello world"));
    }

    #[test]
    fn parse_json_body() {
        let input = "{\"key\":\"value\",\"n\":1}\n";
        let (_, b) = body(input).unwrap();
        assert_eq!(b.as_deref(), Some("{\"key\":\"value\",\"n\":1}"));
    }

    #[test]
    fn parse_empty_body_returns_none() {
        let input = "";
        let (_, b) = body(input).unwrap();
        assert!(b.is_none());
    }

    #[test]
    fn parse_body_stops_at_post_script_marker() {
        // Body should stop when it encounters a line starting with ">"
        let input = "some body\n> {% return results(); %}\n";
        let (remaining, b) = body(input).unwrap();
        assert_eq!(b.as_deref(), Some("some body"));
        // The ">" line should be left in remaining
        assert!(remaining.starts_with('>'), "remaining={:?}", remaining);
    }

    // ── Query params ──────────────────────────────────────────────────────────

    #[test]
    fn parse_single_query_param() {
        let input = "?key=value";
        let (_, segments) = query_params(input).unwrap();
        let joined: String = segments
            .iter()
            .map(|s| match s {
                UrlSegment::Text(t) => *t,
                UrlSegment::Variable(_) => "VAR",
            })
            .collect();
        assert!(joined.contains("key") && joined.contains("value"), "segments={:?}", joined);
    }

    #[test]
    fn parse_multiple_query_params() {
        let input = "?a=1&b=2";
        let (_, segments) = query_params(input).unwrap();
        assert!(!segments.is_empty());
    }

    #[test]
    fn parse_query_param_with_variable() {
        let input = "?token={{auth_token}}";
        let (_, segments) = query_params(input).unwrap();
        let has_var = segments.iter().any(|s| matches!(s, UrlSegment::Variable("auth_token")));
        assert!(has_var, "expected Variable(auth_token) in {:?}", segments);
    }

    #[test]
    fn multiline_url_query_params_joined() {
        let input = "GET https://api.example.com/users\n    ?page=1\n    &limit=20\n\n";
        let (_, entries) = request_collection(input).unwrap();
        assert_eq!(entries.len(), 1);
        let url = match &entries[0].request.request_line.url {
            Url::Segments {
                host,
                path,
                query_params,
            } => {
                let mut s = String::new();
                for seg in host.iter().chain(path.iter()).chain(query_params.iter()) {
                    if let UrlSegment::Text(t) = seg {
                        s.push_str(t);
                    }
                }
                s
            },
            Url::Raw(r) => r.to_string(),
        };
        assert!(url.contains("?page=1"), "expected ?page=1 in URL: {url}");
        assert!(url.contains("&limit=20"), "expected &limit=20 in URL: {url}");
        assert!(!url.contains('\n'), "URL should not contain newline: {url}");
    }

    #[test]
    fn multiline_url_does_not_consume_next_header() {
        let input = "GET https://api.example.com/users\n    ?page=1\nAuthorization: Bearer tok\n\n";
        let (_, entries) = request_collection(input).unwrap();
        assert_eq!(entries.len(), 1);
        let has_auth = entries[0] //
            .request
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("Authorization"));
        assert!(has_auth, "Authorization header should be parsed after multiline URL");
    }

    // ── HTTP method ───────────────────────────────────────────────────────────

    #[test]
    fn parse_get_method() {
        let (_, m) = method("GET /").unwrap();
        assert_eq!(m, "GET");
    }

    #[test]
    fn parse_delete_method() {
        let (_, m) = method("DELETE /").unwrap();
        assert_eq!(m, "DELETE");
    }

    // ── Full collection ───────────────────────────────────────────────────────

    #[test]
    fn parse_collection_multiple_entries() {
        // The body parser is greedy: without a `> {% %}` post-script marker to
        // terminate it, body consumes all remaining text as the first entry's body.
        // Additionally, `take_until(": ")` in the header parser spans multiple
        // lines, so intermediate entries must NOT contain ": " header-like text.
        // The safe multi-entry format is: no headers, each entry terminated by
        // a `> {% %}` post-script (except the last entry which hits EOF).
        let input = concat!(
            "GET https://example.com/one HTTP/1.1\n\n",
            "> {% done %}\n\n",
            "POST https://example.com/two HTTP/1.1\n\n",
            "> {% done %}\n\n",
            "DELETE https://example.com/three HTTP/1.1\n\n",
        );
        let (_, entries) = request_collection(input).unwrap();
        assert_eq!(entries.len(), 3, "expected 3 entries, got {}", entries.len());
        assert_eq!(entries[0].request.request_line.method, "GET");
        assert_eq!(entries[1].request.request_line.method, "POST");
        assert_eq!(entries[2].request.request_line.method, "DELETE");
    }

    #[test]
    fn parse_collection_empty_input() {
        let (_, entries) = request_collection("").unwrap();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn parse_collection_entry_without_scripts() {
        let input = "PUT https://example.com/item HTTP/1.1\nContent-Type: text/plain\n\nupdate\n";
        let (_, entries) = request_collection(input).unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert!(entry.pre_script.is_none());
        assert!(entry.post_script.is_none());
        assert_eq!(entry.request.body, Some(Body::Raw("update".to_string())));
    }

    // ── Variable parser ───────────────────────────────────────────────────────

    #[test]
    fn parse_variable_simple() {
        let (_, seg) = variable("{{host}}rest").unwrap();
        assert_eq!(seg, UrlSegment::Variable("host"));
    }

    #[test]
    fn parse_variable_with_underscore() {
        let (_, seg) = variable("{{auth_token}}").unwrap();
        assert_eq!(seg, UrlSegment::Variable("auth_token"));
    }

    #[test]
    fn parse_variable_with_spaces_inside_braces() {
        let (_, seg) = variable("{{ host }}").unwrap();
        assert_eq!(seg, UrlSegment::Variable("host"));
    }

    // ── HTTP version ──────────────────────────────────────────────────────────

    #[test]
    fn parse_http_version_1_1() {
        let (_, v) = http_version(" HTTP/1.1").unwrap();
        assert_eq!(v, Some("HTTP/1.1"));
    }

    #[test]
    fn parse_http_version_absent() {
        let (_, v) = http_version("").unwrap();
        assert!(v.is_none());
    }

    #[test]
    fn parse_http_version_2() {
        let (_, v) = http_version(" HTTP/2").unwrap();
        assert_eq!(v, Some("HTTP/2"));
    }

    // ── Body variants ─────────────────────────────────────────────────────────

    #[test]
    fn body_file_reference_single_line() {
        let input = "< ./data.json\n";
        let (_, b) = body(input).unwrap();
        assert_eq!(b.map(interpret_body), Some(Body::File("./data.json".to_string())));
    }

    #[test]
    fn body_file_reference_no_trailing_space() {
        let input = "<./payload.xml\n";
        let (_, b) = body(input).unwrap();
        assert_eq!(b.map(interpret_body), Some(Body::File("./payload.xml".to_string())));
    }

    #[test]
    fn body_raw_json_not_misidentified() {
        let input = "{\"key\":\"value\"}\n";
        let (_, b) = body(input).unwrap();
        assert_eq!(b.map(interpret_body), Some(Body::Raw("{\"key\":\"value\"}".to_string())));
    }

    #[test]
    fn body_multiline_file_stays_raw() {
        // A body with two lines where only the first starts with `<` is Raw.
        let input = "< file1.json\nmore text\n";
        let (_, b) = body(input).unwrap();
        assert!(matches!(b.map(interpret_body), Some(Body::Raw(_))));
    }

    #[test]
    fn multipart_two_text_parts() {
        let raw = "\
--boundary\r\n\
Content-Disposition: form-data; name=\"field1\"\r\n\
\r\n\
value1\r\n\
--boundary\r\n\
Content-Disposition: form-data; name=\"field2\"\r\n\
\r\n\
value2\r\n\
--boundary--";
        let result = parse_multipart_body(raw);
        assert!(result.is_some(), "expected Some, got None");
        let Body::Multipart { boundary, parts } = result.unwrap() else {
            panic!("expected Multipart");
        };
        assert_eq!(boundary, "boundary");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].content, PartContent::Text("value1".to_string()));
        assert_eq!(parts[1].content, PartContent::Text("value2".to_string()));
    }

    #[test]
    fn multipart_part_with_file_reference() {
        let raw = "\
--bound\r\n\
Content-Disposition: form-data; name=\"upload\"; filename=\"f.json\"\r\n\
Content-Type: application/json\r\n\
\r\n\
< ./f.json\r\n\
--bound--";
        let result = parse_multipart_body(raw);
        let Body::Multipart { parts, .. } = result.unwrap() else {
            panic!("expected Multipart");
        };
        assert_eq!(parts[0].content, PartContent::File("./f.json".to_string()));
        assert_eq!(parts[0].headers.len(), 2);
    }

    #[test]
    fn multipart_part_headers_parsed() {
        let raw = "\
--b\r\n\
Content-Disposition: form-data; name=\"x\"\r\n\
\r\n\
hello\r\n\
--b--";
        let Body::Multipart { parts, .. } = parse_multipart_body(raw).unwrap() else {
            panic!();
        };
        let (k, v) = &parts[0].headers[0];
        assert_eq!(k, "Content-Disposition");
        assert_eq!(v, "form-data; name=\"x\"");
    }

    #[test]
    fn full_request_with_file_body() {
        let input = "POST https://example.com/upload HTTP/1.1\nContent-Type: application/json\n\n< ./body.json\n";
        let (_, entries) = request_collection(input).unwrap();
        assert_eq!(entries[0].request.body, Some(Body::File("./body.json".to_string())));
    }

    #[test]
    fn full_request_with_multipart_body() {
        let input = concat!(
            "POST https://example.com/upload HTTP/1.1\n",
            "Content-Type: multipart/form-data; boundary=MyBound\n",
            "\n",
            "--MyBound\n",
            "Content-Disposition: form-data; name=\"title\"\n",
            "\n",
            "Hello\n",
            "--MyBound--\n",
        );
        let (_, entries) = request_collection(input).unwrap();
        assert!(
            matches!(&entries[0].request.body, Some(Body::Multipart { boundary, .. }) if boundary == "MyBound"),
            "got {:?}",
            entries[0].request.body
        );
    }

    // ── [form] shorthand ──────────────────────────────────────────────────────

    #[test]
    fn form_block_parses_text_fields() {
        let raw = "[form]\ntitle = Sample Upload\ndescription = A test\n";
        let body = interpret_body(raw.to_string());
        let Body::Form { fields } = body else {
            panic!("expected Body::Form, got {:?}", body);
        };
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "title");
        assert_eq!(fields[0].value, FormFieldValue::Text("Sample Upload".to_string()));
        assert_eq!(fields[1].name, "description");
        assert_eq!(fields[1].value, FormFieldValue::Text("A test".to_string()));
    }

    #[test]
    fn form_block_parses_file_field() {
        let raw = "[form]\nfile = < ./photo.jpg\ncaption = hello\n";
        let body = interpret_body(raw.to_string());
        let Body::Form { fields } = body else {
            panic!("expected Body::Form");
        };
        assert_eq!(fields[0].name, "file");
        assert_eq!(fields[0].value, FormFieldValue::File("./photo.jpg".to_string()));
        assert_eq!(fields[1].value, FormFieldValue::Text("hello".to_string()));
    }

    #[test]
    fn form_block_duplicate_field_names_produce_multiple_parts() {
        // Repeating a field name (e.g. files[]) must produce one FormField per
        // line — the Vec must NOT deduplicate.
        let raw = "[form]\nfiles[] = < ./a.txt; filename=a.txt\nfiles[] = < ./b.txt; filename=b.txt\n";
        let body = interpret_body(raw.to_string());
        let Body::Form { fields } = body else {
            panic!("expected Body::Form, got {:?}", body);
        };
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "files[]");
        assert_eq!(fields[0].value, FormFieldValue::File("./a.txt".to_string()));
        assert_eq!(fields[0].attrs, vec![("filename".to_string(), "a.txt".to_string())]);
        assert_eq!(fields[1].name, "files[]");
        assert_eq!(fields[1].value, FormFieldValue::File("./b.txt".to_string()));
        assert_eq!(fields[1].attrs, vec![("filename".to_string(), "b.txt".to_string())]);
    }

    #[test]
    fn form_block_skips_blank_and_comment_lines() {
        let raw = "[form]\n# a comment\n\nname = Alice\n\n# another\nrole = admin\n";
        let body = interpret_body(raw.to_string());
        let Body::Form { fields } = body else {
            panic!("expected Body::Form");
        };
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "name");
        assert_eq!(fields[1].name, "role");
    }

    #[test]
    fn form_block_empty_fields_list() {
        let raw = "[form]\n# just a comment\n";
        let body = interpret_body(raw.to_string());
        assert!(matches!(body, Body::Form { fields } if fields.is_empty()));
    }

    #[test]
    fn full_request_with_form_block() {
        let input =
            "POST https://example.com/upload HTTP/1.1\n\n[form]\ntitle = Hello\nfile = < ./f.txt\n";
        let (_, entries) = request_collection(input).unwrap();
        let body = &entries[0].request.body;
        assert!(
            matches!(body, Some(Body::Form { fields }) if fields.len() == 2),
            "got {:?}", body
        );
    }

    // ── [form] field attributes ───────────────────────────────────────────────

    #[test]
    fn form_block_field_with_filename_attr() {
        let raw = "[form]\nfile = < ./photo.jpg; filename=photo.jpg\n";
        let Body::Form { fields } = interpret_body(raw.to_string()) else {
            panic!("expected Body::Form");
        };
        assert_eq!(fields[0].name, "file");
        assert_eq!(fields[0].value, FormFieldValue::File("./photo.jpg".to_string()));
        assert_eq!(fields[0].attrs, vec![("filename".to_string(), "photo.jpg".to_string())]);
    }

    #[test]
    fn form_block_field_with_multiple_attrs() {
        let raw = "[form]\nfile = < ./img.png; filename=img.png; Content-Type=image/png\n";
        let Body::Form { fields } = interpret_body(raw.to_string()) else {
            panic!("expected Body::Form");
        };
        assert_eq!(fields[0].attrs.len(), 2);
        assert_eq!(fields[0].attrs[0], ("filename".to_string(), "img.png".to_string()));
        assert_eq!(fields[0].attrs[1], ("Content-Type".to_string(), "image/png".to_string()));
    }

    #[test]
    fn form_block_text_field_with_content_type_attr() {
        let raw = "[form]\ndata = {\"k\":1}; Content-Type=application/json\n";
        let Body::Form { fields } = interpret_body(raw.to_string()) else {
            panic!("expected Body::Form");
        };
        assert_eq!(fields[0].value, FormFieldValue::Text("{\"k\":1}".to_string()));
        assert_eq!(fields[0].attrs, vec![("Content-Type".to_string(), "application/json".to_string())]);
    }

    #[test]
    fn form_block_field_no_attrs_has_empty_attrs_vec() {
        let raw = "[form]\ntitle = Hello\n";
        let Body::Form { fields } = interpret_body(raw.to_string()) else {
            panic!("expected Body::Form");
        };
        assert!(fields[0].attrs.is_empty());
    }

    // ── header-based detection ────────────────────────────────────────────────

    #[test]
    fn content_type_multipart_without_boundary_coerces_body_to_form() {
        let input = concat!(
            "POST https://example.com/upload HTTP/1.1\n",
            "Content-Type: multipart/form-data\n",
            "\n",
            "title = Sample Upload\n",
            "description = A test\n",
        );
        let (_, entries) = request_collection(input).unwrap();
        assert!(
            matches!(&entries[0].request.body, Some(Body::Form { fields }) if fields.len() == 2),
            "got {:?}", entries[0].request.body
        );
    }

    #[test]
    fn content_type_urlencoded_coerces_body_to_form_urlencoded() {
        let input = concat!(
            "POST https://example.com/login HTTP/1.1\n",
            "Content-Type: application/x-www-form-urlencoded\n",
            "\n",
            "username = alice\n",
            "password = s3cr3t\n",
        );
        let (_, entries) = request_collection(input).unwrap();
        assert!(
            matches!(&entries[0].request.body,
                Some(Body::FormUrlEncoded { fields }) if fields.len() == 2),
            "got {:?}", entries[0].request.body
        );
    }

    #[test]
    fn content_type_multipart_with_boundary_does_not_coerce() {
        // When a boundary is already set, keep raw body.
        let input = concat!(
            "POST https://example.com/upload HTTP/1.1\n",
            "Content-Type: multipart/form-data; boundary=MyBound\n",
            "\n",
            "title = not form fields\n",
        );
        let (_, entries) = request_collection(input).unwrap();
        assert!(
            matches!(&entries[0].request.body, Some(Body::Raw(_))),
            "got {:?}", entries[0].request.body
        );
    }

    #[test]
    fn body_without_form_fields_not_coerced() {
        // A plain-text body (no " = " pattern) must not be coerced to Body::Form
        // even when Content-Type: multipart/form-data is present.
        // Note: avoid "key: value" shaped lines — those are consumed as headers.
        let input = concat!(
            "POST https://example.com/ HTTP/1.1\n",
            "Content-Type: multipart/form-data\n",
            "\n",
            "just some raw text with no equals sign\n",
        );
        let (_, entries) = request_collection(input).unwrap();
        assert!(
            matches!(&entries[0].request.body, Some(Body::Raw(_))),
            "got {:?}", entries[0].request.body
        );
    }

    // ── YAML-style multipart body ─────────────────────────────────────────────

    #[test]
    fn yaml_multipart_simple_text_field() {
        let raw = "label: batch upload\ncaption: hello world\n";
        let fields = parse_yaml_multipart_body(raw);
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "label");
        assert_eq!(fields[0].value, FormFieldValue::Text("batch upload".to_string()));
        assert_eq!(fields[1].name, "caption");
        assert_eq!(fields[1].value, FormFieldValue::Text("hello world".to_string()));
    }

    #[test]
    fn yaml_multipart_simple_file_field() {
        let raw = "upload: < ./data.json\n";
        let fields = parse_yaml_multipart_body(raw);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "upload");
        assert_eq!(fields[0].value, FormFieldValue::File("./data.json".to_string()));
    }

    #[test]
    fn yaml_multipart_array_file_fields_with_attrs() {
        let raw = concat!(
            "files:\n",
            "    - file: < ./file1.txt\n",
            "      filename: file1.txt\n",
            "      Content-Type: text/plain\n",
            "    - file: < ./file2.txt\n",
            "      filename: file2.txt\n",
            "      Content-Type: text/plain\n",
        );
        let fields = parse_yaml_multipart_body(raw);
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "files");
        assert_eq!(fields[0].value, FormFieldValue::File("./file1.txt".to_string()));
        assert_eq!(
            fields[0].attrs,
            vec![
                ("filename".to_string(), "file1.txt".to_string()),
                ("Content-Type".to_string(), "text/plain".to_string()),
            ]
        );
        assert_eq!(fields[1].value, FormFieldValue::File("./file2.txt".to_string()));
    }

    #[test]
    fn yaml_multipart_mixed_simple_and_array_fields() {
        let raw = concat!(
            "label: batch upload\n",
            "files:\n",
            "    - file: < ./a.txt\n",
            "      filename: a.txt\n",
            "      Content-Type: text/plain\n",
        );
        let fields = parse_yaml_multipart_body(raw);
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "label");
        assert_eq!(fields[0].value, FormFieldValue::Text("batch upload".to_string()));
        assert_eq!(fields[1].name, "files");
        assert_eq!(fields[1].value, FormFieldValue::File("./a.txt".to_string()));
    }

    #[test]
    fn content_type_multipart_without_boundary_coerces_yaml_body_to_form() {
        let input = concat!(
            "POST https://example.com/upload HTTP/1.1\n",
            "Content-Type: multipart/form-data\n",
            "\n",
            "label: batch upload\n",
            "files:\n",
            "    - file: < ./file1.txt\n",
            "      filename: file1.txt\n",
            "      Content-Type: text/plain\n",
            "    - file: < ./file2.txt\n",
            "      filename: file2.txt\n",
            "      Content-Type: text/plain\n",
        );
        let (_, entries) = request_collection(input).unwrap();
        let body = &entries[0].request.body;
        let Some(Body::Form { fields }) = body else {
            panic!("expected Body::Form, got {:?}", body);
        };
        assert_eq!(fields.len(), 3, "label + 2 file parts");
        assert_eq!(fields[0].name, "label");
        assert_eq!(fields[1].name, "files");
        assert_eq!(fields[1].value, FormFieldValue::File("./file1.txt".to_string()));
        assert_eq!(fields[2].value, FormFieldValue::File("./file2.txt".to_string()));
    }

    // ── form-urlencoded shorthand Content-Type ────────────────────────────────

    #[test]
    fn content_type_form_urlencoded_shorthand_coerces_to_form_urlencoded() {
        let input = concat!(
            "POST https://example.com/post HTTP/1.1\n",
            "Content-Type: form-urlencoded\n",
            "\n",
            "username = alice\n",
            "name = Alice\n",
            "role = admin\n",
        );
        let (_, entries) = request_collection(input).unwrap();
        let body = &entries[0].request.body;
        assert!(
            matches!(body, Some(Body::FormUrlEncoded { fields }) if fields.len() == 3),
            "got {:?}", body
        );
    }

    // ── [form-urlencoded] shorthand ───────────────────────────────────────────

    #[test]
    fn form_urlencoded_block_parses_fields() {
        let raw = "[form-urlencoded]\nusername = alice\npassword = s3cr3t\n";
        let body = interpret_body(raw.to_string());
        let Body::FormUrlEncoded { fields } = body else {
            panic!("expected Body::FormUrlEncoded, got {:?}", body);
        };
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0], ("username".to_string(), "alice".to_string()));
        assert_eq!(fields[1], ("password".to_string(), "s3cr3t".to_string()));
    }

    #[test]
    fn form_urlencoded_block_skips_blank_and_comment_lines() {
        let raw = "[form-urlencoded]\n# login\n\nuser = bob\n\npass = x\n";
        let body = interpret_body(raw.to_string());
        let Body::FormUrlEncoded { fields } = body else {
            panic!("expected Body::FormUrlEncoded");
        };
        assert_eq!(fields.len(), 2);
    }

    #[test]
    fn full_request_with_form_urlencoded_block() {
        let input =
            "POST https://example.com/login HTTP/1.1\n\n[form-urlencoded]\nuser = alice\n";
        let (_, entries) = request_collection(input).unwrap();
        assert!(
            matches!(&entries[0].request.body, Some(Body::FormUrlEncoded { fields }) if fields.len() == 1),
            "got {:?}", entries[0].request.body
        );
    }
}
