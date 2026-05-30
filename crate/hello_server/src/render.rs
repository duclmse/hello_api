//! Response rendering — transforms a [`MatchResult`] into HTTP response bytes.

use std::time::Duration;

use crate::registry::MatchResult;

pub struct RenderedResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub delay: Option<Duration>,
}

pub async fn render(m: &MatchResult<'_>) -> RenderedResponse {
    let mock = m.response;

    // Merge path + query params for template substitution
    let mut params = m.path_params.clone();
    params.extend(m.query_params.clone());

    // Render body (template substitution)
    let rendered_body = mock.body.render(&params);

    // Determine Content-Type
    let mut headers = mock.headers.clone();
    let has_ct = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
    if !has_ct {
        if let Some(ct) = rendered_body.inferred_content_type() {
            headers.push(("Content-Type".to_string(), ct.to_string()));
        }
    }

    let body = rendered_body.to_bytes();
    let delay = if mock.delay_ms > 0 {
        Some(Duration::from_millis(mock.delay_ms))
    } else {
        None
    };

    RenderedResponse { status: mock.status, headers, body, delay }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::model::{MockResponse, ResponseBody};
    use crate::registry::MatchResult;

    fn make_match(body: ResponseBody, status: u16) -> MockResponse {
        MockResponse { status, headers: vec![], body, delay_ms: 0 }
    }

    #[tokio::test]
    async fn json_body_gets_content_type() {
        let mock = make_match(ResponseBody::Json(serde_json::json!({"ok": true})), 200);
        let mr = MatchResult {
            response: &mock,
            path_params: HashMap::new(),
            query_params: HashMap::new(),
        };
        let rr = render(&mr).await;
        assert!(rr.headers.iter().any(|(k, v)| k == "Content-Type" && v.contains("json")));
    }

    #[tokio::test]
    async fn template_substitution() {
        let mock = make_match(ResponseBody::Template("hello {{name}}".to_string()), 200);
        let mr = MatchResult {
            response: &mock,
            path_params: [("name".to_string(), "world".to_string())].into(),
            query_params: HashMap::new(),
        };
        let rr = render(&mr).await;
        assert_eq!(rr.body, b"hello world");
    }

    #[tokio::test]
    async fn delay_propagated() {
        let mut mock = make_match(ResponseBody::Empty, 200);
        mock.delay_ms = 10;
        let mr = MatchResult {
            response: &mock,
            path_params: HashMap::new(),
            query_params: HashMap::new(),
        };
        let rr = render(&mr).await;
        assert_eq!(rr.delay, Some(Duration::from_millis(10)));
    }
}
