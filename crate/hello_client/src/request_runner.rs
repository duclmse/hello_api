#[cfg(test)]
mod test_request_runner {
    use std::collections::HashMap;

    use reqwest::{Method, blocking::Client, header::HeaderMap};

    use hello_core::http_request::{HttpRequest, RequestLine, Url};

    #[test]
    fn main() {
        let request = HttpRequest {
            request_line: RequestLine {
                method: "GET",
                url: Url::Raw("http://www.google.com"),
                http_version: None,
            },
            headers: HashMap::new(),
            body: None,
        };

        let method = match Method::from_bytes("GET".as_bytes()) {
            Ok(m) => m,
            Err(e) => {
                println!("Error parsing method: {}", e);
                return;
            },
        };

        let mut headers = HeaderMap::new();
        headers.insert("User-Agent", "MyRustClient/1.0".parse().unwrap());

        let client = Client::new()
            .request(method, "http://www.google.com")
            .headers(headers);
        let resp = client.send().unwrap();
        let status = resp.status();
        let body = resp.text().unwrap();
        println!("Body: {}", body);
        println!("Status: {}", status);
    }
}
