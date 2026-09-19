//! Integration tests for the blocking client (requires `blocking` feature).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use typesafeai_sdk::blocking::BlockingClient;
use typesafeai_sdk::{ClientConfig, ErrorKind, RetryPolicy, SystemOneOpts};

// ---------------------------------------------------------------------------
// Mock server helpers (duplicated from integration.rs to keep this file
// self-contained under its own feature flag).
// ---------------------------------------------------------------------------

struct MockServer {
    listener: TcpListener,
    addr: String,
}

impl MockServer {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local_addr").to_string();
        Self { listener, addr }
    }

    fn serve(self, responses: Vec<String>) -> ServerHandle {
        let listener = self.listener;
        let addr = self.addr;
        let handle = ServerHandle {
            addr,
            _shutdown: Arc::new(()),
        };

        std::thread::spawn(move || {
            for resp in responses {
                let (mut stream, _) = match listener.accept() {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let _ = read_request(&mut stream);
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
                std::thread::sleep(Duration::from_millis(50));
                drop(stream);
            }
        });

        handle
    }
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while stream.read(&mut byte).unwrap_or(0) > 0 {
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let headers = String::from_utf8_lossy(&buf);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("content-length:") {
                lower
                    .trim_start_matches("content-length:")
                    .trim()
                    .parse::<usize>()
                    .ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    for _ in 0..content_length {
        if stream.read(&mut byte).unwrap_or(0) > 0 {
            buf.push(byte[0]);
        } else {
            break;
        }
    }
    buf
}

struct ServerHandle {
    addr: String,
    _shutdown: Arc<()>,
}

impl ServerHandle {
    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

fn http_response(status: u16, reason: &str, headers: &str, body: &str) -> String {
    let content_len = body.len();
    format!(
        "HTTP/1.1 {status} {reason}\r\n{headers}Content-Length: {content_len}\r\nConnection: close\r\n\r\n{body}"
    )
}

fn systemone_body() -> String {
    r#"{"model":"jev-latest","answers":{"billing":{"type":"noul","noul":0.95}},"usage":{"input_tokens":10,"output_tokens":5}}"#
        .to_string()
}

fn models_body() -> String {
    r#"{"models":[{"name":"jev-latest","description":"Latest Jev model"}]}"#.to_string()
}

fn billing_question() -> std::collections::HashMap<String, typesafeai_sdk::Question> {
    let mut q = std::collections::HashMap::new();
    q.insert(
        "billing".to_string(),
        typesafeai_sdk::noul("Is this about billing?").into(),
    );
    q
}

fn blocking_client(url: &str) -> BlockingClient {
    let config = ClientConfig::new("test_key")
        .with_base_url(url.to_string())
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(
            RetryPolicy::new(3)
                .with_base_delay(Duration::from_millis(50))
                .with_max_delay(Duration::from_secs(5)),
        );
    BlockingClient::from_config(config).expect("blocking client")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn blocking_system_one_succeeds() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: application/json\r\n",
        &systemone_body(),
    )]);

    let client = blocking_client(&handle.url());
    let result = client.system_one("test", billing_question()).unwrap();

    assert_eq!(result.model, "jev-latest");
    assert!(result.noul("billing").is_some());
}

#[test]
fn blocking_list_models_succeeds() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: application/json\r\n",
        &models_body(),
    )]);

    let client = blocking_client(&handle.url());
    let result = client.list_models().unwrap();

    assert_eq!(result.models.len(), 1);
    assert_eq!(result.models[0].name, "jev-latest");
}

#[test]
fn blocking_system_one_with_model_override() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: application/json\r\n",
        &systemone_body(),
    )]);

    let client = blocking_client(&handle.url());
    let result = client
        .system_one_with_model("test", billing_question(), Some("custom-model"))
        .unwrap();

    assert_eq!(result.model, "jev-latest");
}

#[test]
fn blocking_system_one_with_opts() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: application/json\r\n",
        &systemone_body(),
    )]);

    let client = blocking_client(&handle.url());
    let opts = SystemOneOpts::new()
        .with_model(Some("custom-model".to_string()))
        .with_extra_body(serde_json::json!({"user_id": "u123"}));

    let result = client
        .system_one_with_opts(billing_question(), opts)
        .unwrap();

    assert_eq!(result.model, "jev-latest");
}

#[test]
fn blocking_returns_error_on_401() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        401,
        "Unauthorized",
        "Content-Type: application/json\r\n",
        r#"{"error":{"message":"Invalid API key"}}"#,
    )]);

    let config = ClientConfig::new("test_key")
        .with_base_url(handle.url())
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(RetryPolicy::new(0));
    let client = BlockingClient::from_config(config).unwrap();

    let err = client.system_one("test", billing_question()).unwrap_err();

    assert!(
        matches!(err.kind(), ErrorKind::Authentication(_)),
        "got {err:?}"
    );
}

#[test]
fn blocking_retries_on_429() {
    let server = MockServer::new();
    let handle = server.serve(vec![
        http_response(
            429,
            "Too Many Requests",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"slow down"}}"#,
        ),
        http_response(
            200,
            "OK",
            "Content-Type: application/json\r\n",
            &systemone_body(),
        ),
    ]);

    let client = blocking_client(&handle.url());
    let result = client.system_one("test", billing_question()).unwrap();

    assert_eq!(result.model, "jev-latest");
}

#[test]
fn blocking_new_requires_api_key() {
    let result = BlockingClient::new("");
    assert!(result.is_err());
}

#[test]
fn blocking_base_url_and_default_model_accessors() {
    let client = blocking_client("http://localhost:9999");
    assert_eq!(client.base_url(), "http://localhost:9999");
    assert_eq!(client.default_model(), "jev-latest");
}
