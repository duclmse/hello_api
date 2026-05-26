use std::collections::HashMap;

use crate::metadata::Metadata;

#[derive(Debug)]
pub struct RequestEntry<'a> {
    pub metadata: Metadata<'a>,
    pub pre_script: Option<Script<'a>>,
    pub post_script: Option<Script<'a>>,
    pub request: HttpRequest<'a>,
}

#[derive(Debug)]
pub enum Script<'a> {
    Inline(&'a str),
    File(&'a str),
}

/// The body of an HTTP request, as parsed from the `.http` file.
#[derive(Debug, PartialEq, Clone)]
pub enum Body {
    /// Plain text, JSON, XML, or URL-encoded form — sent as-is.
    Raw(String),
    /// `< path/to/file` — entire file content sent as the body at run time.
    File(String),
    /// Standard multipart/form-data body (`--boundary` blocks).
    Multipart {
        boundary: String,
        parts: Vec<MultipartPart>,
    },
}

/// One part inside a multipart body.
#[derive(Debug, PartialEq, Clone)]
pub struct MultipartPart {
    /// Part-level headers (`Content-Disposition`, `Content-Type`, …).
    pub headers: Vec<(String, String)>,
    pub content: PartContent,
}

/// Content of a single multipart part.
#[derive(Debug, PartialEq, Clone)]
pub enum PartContent {
    /// Inline text (including inline JSON/XML).
    Text(String),
    /// `< path/to/file` — file content inserted at run time.
    File(String),
}

#[derive(Debug, PartialEq, Default)]
pub struct HttpRequest<'a> {
    pub request_line: RequestLine<'a>,
    pub headers: HashMap<&'a str, &'a str>,
    pub body: Option<Body>,
}

#[derive(Debug, PartialEq, Default)]
pub struct RequestLine<'a> {
    pub method: &'a str,
    pub url: Url<'a>,
    pub http_version: Option<&'a str>,
}

#[derive(Debug, PartialEq)]
pub enum Url<'a> {
    #[allow(dead_code)]
    Raw(&'a str),
    Segments {
        host: Vec<UrlSegment<'a>>,
        path: Vec<UrlSegment<'a>>,
        query_params: Vec<UrlSegment<'a>>,
    },
}

impl<'a> Default for Url<'a> {
    fn default() -> Self {
        todo!()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum UrlSegment<'a> {
    Text(&'a str),
    Variable(&'a str),
}

impl<'a> RequestLine<'a> {
    #[allow(dead_code)]
    pub(crate) fn get_verbatim_endpoint(self) -> String {
        match self.url {
            Url::Segments {
                host,
                path,
                query_params,
            } => {
                host.into_iter() //
                    .chain(path)
                    .chain(query_params)
                    .fold(String::new(), |mut ep, segment| {
                        let ch = match segment {
                            UrlSegment::Text(txt) => txt.to_owned(),
                            UrlSegment::Variable(var) => {
                                format!("{{{{{}}}}}", var)
                            },
                        };
                        ep.push_str(ch.as_str());
                        ep
                    })
            },
            Url::Raw(_) => todo!(),
        }
    }
}
