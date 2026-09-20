//! Integration tests for the TypeSafe SDK client.
//!
//! These tests spin up a raw TCP mock server (no external HTTP framework
//! needed) and exercise the client's retry, error-mapping, and header
//! behaviour end-to-end.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use typesafeai_sdk::{ClientConfig, ErrorKind, RetryPolicy, TypeSafeClient};

// ---------------------------------------------------------------------------
// Mock server helpers
// ---------------------------------------------------------------------------

/// A tiny mock HTTP server that responds with a scripted sequence of raw
/// HTTP responses, one per connection.
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

    /// Accept `responses.len()` connections, sending each scripted response.
    /// Each response is sent on its own connection (the client opens a new
    /// connection per attempt).
    fn serve(self, responses: Vec<String>) -> ServerHandle {
        let listener = self.listener;
        let addr = self.addr;
        let request_count = Arc::new(std::sync::Mutex::new(0));
        let request_count_clone = request_count.clone();
        let handle = ServerHandle {
            addr,
            request_count,
            _shutdown: Arc::new(()),
        };

        std::thread::spawn(move || {
            for resp in responses {
                // Accept one connection per response.
                let (mut stream, _) = match listener.accept() {
                    Ok(s) => s,
                    Err(_) => break,
                };
                // Read and discard the request (we still need to consume it).
                let _ = read_request(&mut stream);
                *request_count_clone.lock().unwrap() += 1;
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
                // Give the client a moment to read before we close.
                std::thread::sleep(Duration::from_millis(50));
                drop(stream);
            }
        });

        handle
    }
}

/// Read the HTTP request line + headers from a stream, discarding the body.
/// For POST requests with a body, also reads Content-Length bytes.
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
    // If there's a Content-Length header, read the body too.
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
    request_count: Arc<std::sync::Mutex<usize>>,
    _shutdown: Arc<()>,
}

impl ServerHandle {
    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn request_count(&self) -> usize {
        *self.request_count.lock().unwrap()
    }
}

/// Build a minimal HTTP/1.1 response with a reason phrase.
fn http_response(status: u16, reason: &str, headers: &str, body: &str) -> String {
    let content_len = body.len();
    format!(
        "HTTP/1.1 {status} {reason}\r\n{headers}Content-Length: {content_len}\r\nConnection: close\r\n\r\n{body}"
    )
}

/// Build a valid `systemone` success response body.
fn systemone_body() -> String {
    r#"{"model":"jev-latest","answers":{"billing":{"type":"noul","noul":0.95}},"usage":{"input_tokens":10,"output_tokens":5}}"#
        .to_string()
}

/// Build a valid `models` list response body.
fn models_body() -> String {
    r#"{"models":[{"name":"jev-latest","description":"Latest Jev model"}]}"#.to_string()
}

/// Create a client pointed at the mock server with fast retries.
fn test_client(url: &str) -> TypeSafeClient {
    let config = ClientConfig::new("test_key")
        .with_base_url(url.to_string())
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(
            RetryPolicy::new(3)
                .with_base_delay(Duration::from_millis(50))
                .with_max_delay(Duration::from_secs(5)),
        );
    TypeSafeClient::from_config(config).expect("client")
}

/// Create a single-question map for system_one calls.
fn billing_question() -> std::collections::HashMap<String, typesafeai_sdk::Question> {
    let mut q = std::collections::HashMap::new();
    q.insert(
        "billing".to_string(),
        typesafeai_sdk::noul("Is this about billing?").into(),
    );
    q
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn succeeds_on_first_try() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: application/json\r\n",
        &systemone_body(),
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap();

    assert_eq!(result.model, "jev-latest");
    assert!(result.noul("billing").is_some());
}

#[test]
fn retries_on_429_then_succeeds() {
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

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap();

    assert_eq!(result.model, "jev-latest");
}

#[test]
fn honors_retry_after_header() {
    let server = MockServer::new();
    let handle = server.serve(vec![
        http_response(
            429,
            "Too Many Requests",
            "Content-Type: application/json\r\nRetry-After: 1\r\n",
            r#"{"error":{"message":"slow down"}}"#,
        ),
        http_response(
            200,
            "OK",
            "Content-Type: application/json\r\n",
            &systemone_body(),
        ),
    ]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let start = Instant::now();
    let result = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap();

    // The Retry-After: 1 header should cause at least ~1s of delay.
    assert!(
        start.elapsed() >= Duration::from_millis(900),
        "expected >= 900ms delay, got {:?}",
        start.elapsed()
    );
    assert_eq!(result.model, "jev-latest");
}

#[test]
fn gives_up_when_retry_after_exceeds_max_delay() {
    let server = MockServer::new();
    let handle = server.serve(vec![
        http_response(
            429,
            "Too Many Requests",
            "Content-Type: application/json\r\nRetry-After: 600\r\n",
            r#"{"error":{"message":"slow down"}}"#,
        ),
        // This response should never be reached.
        http_response(
            200,
            "OK",
            "Content-Type: application/json\r\n",
            &systemone_body(),
        ),
    ]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let start = Instant::now();
    let result = rt.block_on(async { client.system_one("test", billing_question()).await });

    let elapsed = start.elapsed();
    assert!(result.is_err(), "should have failed");
    assert!(
        elapsed < Duration::from_secs(5),
        "should not wait 600s, got {elapsed:?}"
    );
    let err = result.unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::RateLimit(_)), "got {err:?}");
}

#[test]
fn maps_401_to_authentication_error() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        401,
        "Unauthorized",
        "Content-Type: application/json\r\n",
        r#"{"error":{"message":"Invalid API key"}}"#,
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap_err();

    assert!(
        matches!(err.kind(), ErrorKind::Authentication(_)),
        "got {err:?}"
    );
}

#[test]
fn maps_400_to_bad_request_error() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        400,
        "Bad Request",
        "Content-Type: application/json\r\n",
        r#"{"error":{"message":"Bad request"}}"#,
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap_err();

    assert!(
        matches!(err.kind(), ErrorKind::BadRequest(_)),
        "got {err:?}"
    );
}

#[test]
fn maps_404_to_not_found_error() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        404,
        "Not Found",
        "Content-Type: application/json\r\n",
        r#"{"error":{"message":"Not found"}}"#,
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap_err();

    assert!(matches!(err.kind(), ErrorKind::NotFound(_)), "got {err:?}");
}

#[test]
fn maps_422_to_unprocessable_entity_error() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        422,
        "Unprocessable Entity",
        "Content-Type: application/json\r\n",
        r#"{"error":{"message":"Validation failed"}}"#,
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap_err();

    assert!(
        matches!(err.kind(), ErrorKind::UnprocessableEntity(_)),
        "got {err:?}"
    );
}

#[test]
fn maps_500_to_internal_server_error_and_retries() {
    let server = MockServer::new();
    let handle = server.serve(vec![
        http_response(
            500,
            "Internal Server Error",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"oops"}}"#,
        ),
        http_response(
            500,
            "Internal Server Error",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"oops"}}"#,
        ),
        http_response(
            500,
            "Internal Server Error",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"oops"}}"#,
        ),
        http_response(
            500,
            "Internal Server Error",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"oops"}}"#,
        ),
    ]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap_err();

    assert!(
        matches!(err.kind(), ErrorKind::InternalServer(_)),
        "got {err:?}"
    );
}

/// HTTP 408 Request Timeout should be classified as a retryable
/// `RequestTimeout` error, not a generic `Api` error.
#[test]
fn maps_408_to_request_timeout_and_retries() {
    let server = MockServer::new();
    let handle = server.serve(vec![
        http_response(
            408,
            "Request Timeout",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"timeout"}}"#,
        ),
        http_response(
            408,
            "Request Timeout",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"timeout"}}"#,
        ),
        http_response(
            408,
            "Request Timeout",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"timeout"}}"#,
        ),
        http_response(
            408,
            "Request Timeout",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"timeout"}}"#,
        ),
    ]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap_err();

    assert!(
        matches!(err.kind(), ErrorKind::RequestTimeout(_)),
        "got {err:?}"
    );
    assert!(err.is_retryable());
}

#[test]
fn maps_529_to_overloaded_error() {
    let server = MockServer::new();
    let handle = server.serve(vec![
        http_response(
            529,
            "Overloaded",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"overloaded"}}"#,
        ),
        http_response(
            529,
            "Overloaded",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"overloaded"}}"#,
        ),
        http_response(
            529,
            "Overloaded",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"overloaded"}}"#,
        ),
        http_response(
            529,
            "Overloaded",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"overloaded"}}"#,
        ),
    ]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap_err();

    assert!(
        matches!(err.kind(), ErrorKind::Overloaded(_)),
        "got {err:?}"
    );
}

#[test]
fn maps_unknown_status_to_api_error() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        418,
        "I'm a teapot",
        "Content-Type: application/json\r\n",
        r#"{"error":{"message":"I'm a teapot"}}"#,
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap_err();

    match err.kind() {
        ErrorKind::Api { status, .. } => assert_eq!(*status, 418),
        other => panic!("expected Api error, got {other:?}"),
    }
}

#[test]
fn connection_error_when_server_unreachable() {
    // Bind and immediately drop to get a port that refuses connections.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);

    let client = test_client(&format!("http://{addr}"));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap_err();

    // Connection refused should map to Connection (retryable, but exhausted).
    assert!(
        matches!(err.kind(), ErrorKind::Connection(_)),
        "got {err:?}"
    );
}

#[test]
fn timeout_maps_to_timeout_error() {
    // Start a server that accepts connections but never responds.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    std::thread::spawn(move || {
        for _ in 0..5 {
            if let Ok((mut stream, _)) = listener.accept() {
                // Read the request but never respond.
                let _ = read_request(&mut stream);
                // Hold the connection open briefly; the client times out
                // after 100ms and moves on. The stream drops when the loop
                // iteration ends, closing the connection.
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    });

    let config = ClientConfig::new("test_key")
        .with_base_url(format!("http://{addr}"))
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_millis(100))
        .with_retry(RetryPolicy::new(0));
    let client = TypeSafeClient::from_config(config).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap_err();

    assert!(matches!(err.kind(), ErrorKind::Timeout(_)), "got {err:?}");
}

#[test]
fn non_ascii_error_body_does_not_panic() {
    let server = MockServer::new();
    // Error body with multi-byte UTF-8 characters.
    let body = r#"{"error":{"message":"Erreur: coût élevé — 日本語 🙂"}}"#;
    let handle = server.serve(vec![http_response(
        400,
        "Bad Request",
        "Content-Type: application/json; charset=utf-8\r\n",
        body,
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap_err();

    // Should not panic and should extract the message.
    match err.kind() {
        ErrorKind::BadRequest(msg) => assert!(msg.contains("coût")),
        other => panic!("expected BadRequest with non-ASCII message, got {other:?}"),
    }
}

#[test]
fn list_models_succeeds() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: application/json\r\n",
        &models_body(),
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async { client.list_models().await }).unwrap();

    assert_eq!(result.models.len(), 1);
    assert_eq!(result.models[0].name, "jev-latest");
}

/// Helper that captures the raw HTTP request sent to a single-connection
/// mock server, then replies with the given response. Takes ownership of
/// the listener so the caller doesn't need to bind twice.
fn capture_request_and_reply(
    listener: TcpListener,
    response: String,
) -> (String, Arc<std::sync::Mutex<Vec<u8>>>) {
    let addr = listener.local_addr().unwrap().to_string();
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_clone = captured.clone();

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_request(&mut stream);
        *captured_clone.lock().unwrap() = request;
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
        std::thread::sleep(Duration::from_millis(50));
        drop(stream);
    });

    (addr, captured)
}

#[test]
fn sends_authorization_header() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (addr, captured) = capture_request_and_reply(
        listener,
        http_response(
            200,
            "OK",
            "Content-Type: application/json\r\n",
            &systemone_body(),
        ),
    );

    let config = ClientConfig::new("my_secret_key")
        .with_base_url(format!("http://{addr}"))
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(RetryPolicy::new(0));
    let client = TypeSafeClient::from_config(config).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _ = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap();

    let request = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    let lower = request.to_ascii_lowercase();
    assert!(
        lower.contains("authorization: bearer my_secret_key"),
        "missing auth header in request:\n{request}"
    );
}

#[test]
fn sends_user_agent_header() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (addr, captured) = capture_request_and_reply(
        listener,
        http_response(
            200,
            "OK",
            "Content-Type: application/json\r\n",
            &systemone_body(),
        ),
    );

    let config = ClientConfig::new("test_key")
        .with_base_url(format!("http://{addr}"))
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(RetryPolicy::new(0));
    let client = TypeSafeClient::from_config(config).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _ = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap();

    let request = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(
        request.contains("typesafeai-sdk/"),
        "missing user-agent in request:\n{request}"
    );
}

#[test]
fn sends_accept_json_header() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (addr, captured) = capture_request_and_reply(
        listener,
        http_response(
            200,
            "OK",
            "Content-Type: application/json\r\n",
            &systemone_body(),
        ),
    );

    let config = ClientConfig::new("test_key")
        .with_base_url(format!("http://{addr}"))
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(RetryPolicy::new(0));
    let client = TypeSafeClient::from_config(config).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _ = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap();

    let request = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    let lower = request.to_ascii_lowercase();
    assert!(
        lower.contains("accept: application/json"),
        "missing Accept header in request:\n{request}"
    );
}

#[test]
fn rejects_empty_model_name() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: application/json\r\n",
        &systemone_body(),
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async {
            client
                .system_one_with_model("test", billing_question(), Some("  "))
                .await
        })
        .unwrap_err();

    assert!(
        matches!(err.kind(), ErrorKind::Validation(ref m) if m.contains("Model name")),
        "got {err:?}"
    );
}

#[test]
fn rejects_empty_question_name() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: application/json\r\n",
        &systemone_body(),
    )]);

    let client = test_client(&handle.url());
    let mut questions = std::collections::HashMap::new();
    questions.insert(
        "".to_string(),
        typesafeai_sdk::noul("Is this about billing?").into(),
    );

    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", questions).await })
        .unwrap_err();

    assert!(
        matches!(err.kind(), ErrorKind::Validation(ref m) if m.contains("Question names")),
        "got {err:?}"
    );
}

/// A truncated response body (Content-Length claims more bytes than sent)
/// must map to a non-retryable `Transport` error, not `Connection`, because
/// the server has already processed the POST by the time the body is read.
#[test]
fn truncated_body_is_not_retryable() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    std::thread::spawn(move || {
        for _ in 0..3 {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = read_request(&mut stream);
                // Claim 9999 bytes but send only a few, then close.
                let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 9999\r\nConnection: close\r\n\r\n{\"model\":\"jev";
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
                std::thread::sleep(Duration::from_millis(50));
                drop(stream);
            }
        }
    });

    let config = ClientConfig::new("test_key")
        .with_base_url(format!("http://{addr}"))
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(RetryPolicy::new(2));
    let client = TypeSafeClient::from_config(config).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap_err();

    // Body-read failures must be Transport (non-retryable), not Connection.
    assert!(matches!(err.kind(), ErrorKind::Transport(_)), "got {err:?}");
    assert!(!err.is_retryable());
}

/// A timeout while reading the response body must be non-retryable, because
/// the server has already processed the POST by that point.
#[test]
fn response_body_timeout_is_not_retryable() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    std::thread::spawn(move || {
        for _ in 0..3 {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = read_request(&mut stream);
                // Send headers claiming a large body, then stall forever.
                let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 9999\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
                // Hold the connection open but never send the body.
                // The client times out after 200ms; 500ms is enough to
                // keep the connection alive past the timeout without
                // leaving a long-lived thread.
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    });

    let config = ClientConfig::new("test_key")
        .with_base_url(format!("http://{addr}"))
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_millis(200))
        .with_retry(RetryPolicy::new(2));
    let client = TypeSafeClient::from_config(config).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap_err();

    // Body-read timeout must be Transport (non-retryable), not Timeout.
    assert!(matches!(err.kind(), ErrorKind::Transport(_)), "got {err:?}");
    assert!(!err.is_retryable());
}

/// An oversized 503 response body must still be classified as a retryable
/// `InternalServer` error, not a non-retryable `Transport` error from the
/// body-size cap. The HTTP status code is authoritative for retry decisions.
#[test]
fn oversized_503_body_is_still_retryable() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    std::thread::spawn(move || {
        for _ in 0..3 {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = read_request(&mut stream);
                // Send a 503 with a body larger than MAX_RESPONSE_BODY_BYTES.
                let big_body = "x".repeat(2 * 1024 * 1024); // 2 MiB
                let resp = format!(
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    big_body.len(),
                    big_body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
                std::thread::sleep(Duration::from_millis(50));
                drop(stream);
            }
        }
    });

    let config = ClientConfig::new("test_key")
        .with_base_url(format!("http://{addr}"))
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(RetryPolicy::new(0));
    let client = TypeSafeClient::from_config(config).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap_err();

    // Must be InternalServer (retryable), not Transport (non-retryable).
    assert!(
        matches!(err.kind(), ErrorKind::InternalServer(_)),
        "got {err:?}"
    );
    assert!(err.is_retryable());
}

#[test]
fn captures_request_id_from_response_header() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: application/json\r\nx-typesafe-request-id: req-abc-123\r\n",
        &systemone_body(),
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap();

    assert_eq!(result.request_id(), Some("req-abc-123"));
}

#[test]
fn request_id_absent_when_header_missing() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: application/json\r\n",
        &systemone_body(),
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap();

    assert_eq!(result.request_id(), None);
}

#[test]
fn captures_request_id_on_error() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        429,
        "Too Many Requests",
        "Content-Type: application/json\r\nx-typesafe-request-id: req-err-456\r\n",
        r#"{"error":{"message":"slow down"}}"#,
    )]);

    // Use no retries so the first 429 is returned immediately.
    let config = ClientConfig::new("test_key")
        .with_base_url(handle.url())
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(RetryPolicy::none());
    let client = TypeSafeClient::from_config(config).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap_err();

    assert_eq!(err.request_id(), Some("req-err-456"));
    assert!(matches!(err.kind(), ErrorKind::RateLimit(_)));
}

#[test]
fn honors_retry_after_ms_header() {
    let server = MockServer::new();
    let handle = server.serve(vec![
        http_response(
            429,
            "Too Many Requests",
            "Content-Type: application/json\r\nretry-after-ms: 500\r\n",
            r#"{"error":{"message":"slow down"}}"#,
        ),
        http_response(
            200,
            "OK",
            "Content-Type: application/json\r\n",
            &systemone_body(),
        ),
    ]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let start = Instant::now();
    let result = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap();

    // The retry-after-ms: 500 header should cause at least ~500ms of delay.
    assert!(
        start.elapsed() >= Duration::from_millis(400),
        "expected >= 400ms delay, got {:?}",
        start.elapsed()
    );
    assert_eq!(result.model, "jev-latest");
}

#[test]
fn budget_stops_retry_before_exceeding_limit() {
    let server = MockServer::new();
    // Server always returns 429, so retries would normally continue.
    let handle = server.serve(vec![
        http_response(
            429,
            "Too Many Requests",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"slow down"}}"#,
        ),
        http_response(
            429,
            "Too Many Requests",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"slow down"}}"#,
        ),
        http_response(
            429,
            "Too Many Requests",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"slow down"}}"#,
        ),
        http_response(
            429,
            "Too Many Requests",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"slow down"}}"#,
        ),
    ]);

    // Set a very small budget (50ms) with large delays so the budget is
    // exceeded on the first retry.
    let config = ClientConfig::new("test_key")
        .with_base_url(handle.url())
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(
            RetryPolicy::new(5)
                .with_base_delay(Duration::from_secs(10))
                .with_max_delay(Duration::from_secs(60))
                .with_budget(Some(Duration::from_millis(50))),
        );
    let client = TypeSafeClient::from_config(config).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let start = Instant::now();
    let err = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap_err();

    // Should stop quickly because the 10s delay exceeds the 50ms budget.
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "budget not enforced, took {:?}",
        start.elapsed()
    );
    assert!(matches!(err.kind(), ErrorKind::RateLimit(_)), "got {err:?}");
}

/// The first attempt must use the full configured timeout, even when a
/// tight budget is set.  We simulate a slow server that takes longer
/// than the budget but shorter than the per-call timeout — the first
/// attempt should still succeed (not be cut short by the budget).
#[test]
fn first_attempt_uses_full_timeout_not_budget() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    // Spawn a thread that accepts the connection, sleeps 300ms, then
    // replies with a success response.
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let response = http_response(
            200,
            "OK",
            "Content-Type: application/json\r\n",
            &systemone_body(),
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
        // Keep the stream open briefly so the client can read.
        std::thread::sleep(Duration::from_millis(100));
    });

    // Budget is 100ms — smaller than the 300ms the server takes — but the
    // first attempt must use the full 5s timeout, so the request should
    // succeed.
    let config = ClientConfig::new("test_key")
        .with_base_url(format!("http://{addr}"))
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(RetryPolicy::new(0).with_budget(Some(Duration::from_millis(100))));
    let client = TypeSafeClient::from_config(config).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap();

    assert_eq!(result.model, "jev-latest");
}

/// When the budget is exhausted, the SDK should return the *last real
/// error* from the server, not a synthetic `Timeout`.
#[test]
fn budget_exhausted_returns_last_error_not_synthetic_timeout() {
    let server = MockServer::new();
    // Server always returns 500 Internal Server Error.
    let handle = server.serve(vec![
        http_response(
            500,
            "Internal Server Error",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"server crash"}}"#,
        ),
        http_response(
            500,
            "Internal Server Error",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"server crash"}}"#,
        ),
        http_response(
            500,
            "Internal Server Error",
            "Content-Type: application/json\r\n",
            r#"{"error":{"message":"server crash"}}"#,
        ),
    ]);

    // Very small budget (50ms) with large delays so the budget is
    // exhausted before the first retry can happen.
    let config = ClientConfig::new("test_key")
        .with_base_url(handle.url())
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(
            RetryPolicy::new(5)
                .with_base_delay(Duration::from_secs(10))
                .with_max_delay(Duration::from_secs(60))
                .with_budget(Some(Duration::from_millis(50))),
        );
    let client = TypeSafeClient::from_config(config).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap_err();

    // Must be the real 500 error, not a synthetic Timeout.
    assert!(
        matches!(err.kind(), ErrorKind::InternalServer(_)),
        "expected InternalServer (last real error), got {err:?}"
    );
}

#[test]
fn extra_body_fields_are_sent_in_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (addr, captured) = capture_request_and_reply(
        listener,
        http_response(
            200,
            "OK",
            "Content-Type: application/json\r\n",
            &systemone_body(),
        ),
    );

    let config = ClientConfig::new("test_key")
        .with_base_url(format!("http://{addr}"))
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(RetryPolicy::new(0));
    let client = TypeSafeClient::from_config(config).unwrap();

    let opts = typesafeai_sdk::SystemOneOpts::new()
        .with_extra_body(serde_json::json!({"user_id": "u123", "priority": "high"}));

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _ = rt
        .block_on(async { client.system_one_with_opts(billing_question(), opts).await })
        .unwrap();

    let request = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    // Extract the JSON body from the request
    let body_start = request.find("\r\n\r\n").unwrap() + 4;
    let body: serde_json::Value = serde_json::from_str(&request[body_start..]).unwrap();
    assert_eq!(body["user_id"], "u123");
    assert_eq!(body["priority"], "high");
    // Known fields should still be present
    assert!(body.get("state").is_some());
    assert!(body["questions"].is_object());
    assert_eq!(body["model"], "jev-latest");
}

#[test]
fn extra_body_rejects_reserved_key_model() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (addr, _captured) = capture_request_and_reply(
        listener,
        http_response(
            200,
            "OK",
            "Content-Type: application/json\r\n",
            &systemone_body(),
        ),
    );

    let config = ClientConfig::new("test_key")
        .with_base_url(format!("http://{addr}"))
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(RetryPolicy::new(0));
    let client = TypeSafeClient::from_config(config).unwrap();

    // "model" is a reserved key — must be rejected, not last-write-wins.
    let opts = typesafeai_sdk::SystemOneOpts::new()
        .with_extra_body(serde_json::json!({"model": "custom-model"}));

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async { client.system_one_with_opts(billing_question(), opts).await });

    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("reserved") && msg.contains("model"),
        "expected reserved key rejection for model, got: {msg}"
    );
}

#[test]
fn system_one_with_opts_overrides_model() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (addr, captured) = capture_request_and_reply(
        listener,
        http_response(
            200,
            "OK",
            "Content-Type: application/json\r\n",
            &systemone_body(),
        ),
    );

    let config = ClientConfig::new("test_key")
        .with_base_url(format!("http://{addr}"))
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(RetryPolicy::new(0));
    let client = TypeSafeClient::from_config(config).unwrap();

    let opts = typesafeai_sdk::SystemOneOpts::new().with_model(Some("custom-model".to_string()));

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _ = rt
        .block_on(async { client.system_one_with_opts(billing_question(), opts).await })
        .unwrap();

    let request = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    let body_start = request.find("\r\n\r\n").unwrap() + 4;
    let body: serde_json::Value = serde_json::from_str(&request[body_start..]).unwrap();
    assert_eq!(body["model"], "custom-model");
}

#[test]
fn raw_body_is_captured_on_system_one_response() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: application/json\r\n",
        &systemone_body(),
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap();

    let raw = result.raw().expect("raw body should be captured");
    assert_eq!(raw["model"], "jev-latest");
    assert_eq!(raw["answers"]["billing"]["noul"], 0.95);
}

#[test]
fn raw_body_is_captured_on_list_models_response() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: application/json\r\n",
        &models_body(),
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async { client.list_models().await }).unwrap();

    let raw = result.raw().expect("raw body should be captured");
    assert_eq!(raw["models"][0]["name"], "jev-latest");
}

// ===========================================================================
// Per-call extra headers
// ===========================================================================

#[test]
fn extra_headers_are_sent_in_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (addr, captured) = capture_request_and_reply(
        listener,
        http_response(
            200,
            "OK",
            "Content-Type: application/json\r\n",
            &systemone_body(),
        ),
    );

    let config = ClientConfig::new("test_key")
        .with_base_url(format!("http://{addr}"))
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(RetryPolicy::new(0));
    let client = TypeSafeClient::from_config(config).unwrap();

    let opts = typesafeai_sdk::SystemOneOpts::new()
        .with_extra_header("X-Trace-Id", "abc-123")
        .with_extra_header("X-Request-Source", "test-suite");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _ = rt
        .block_on(async { client.system_one_with_opts(billing_question(), opts).await })
        .unwrap();

    let request = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    let lower = request.to_ascii_lowercase();
    assert!(
        lower.contains("x-trace-id: abc-123"),
        "missing custom header in request:\n{request}"
    );
    assert!(
        lower.contains("x-request-source: test-suite"),
        "missing second custom header in request:\n{request}"
    );
}

#[test]
fn extra_headers_protected_headers_are_rejected() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (addr, _captured) = capture_request_and_reply(
        listener,
        http_response(
            200,
            "OK",
            "Content-Type: application/json\r\n",
            &systemone_body(),
        ),
    );

    let config = ClientConfig::new("test_key")
        .with_base_url(format!("http://{addr}"))
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(RetryPolicy::new(0));
    let client = TypeSafeClient::from_config(config).unwrap();

    // Try to override protected headers — they should be rejected with an
    // error, not silently ignored.
    let opts = typesafeai_sdk::SystemOneOpts::new()
        .with_extra_header("Authorization", "Bearer evil-token")
        .with_extra_header("Accept", "text/html")
        .with_extra_header("x-typesafe-sdk", "fake-sdk/0.0.0");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async { client.system_one_with_opts(billing_question(), opts).await });

    assert!(
        result.is_err(),
        "expected protected headers to be rejected, but the call succeeded"
    );
    let err = result.unwrap_err();
    assert!(
        matches!(err.kind(), ErrorKind::Validation(msg) if msg.contains("protected")),
        "expected Validation error about protected header, got: {err:?}"
    );
}

// ===========================================================================
// Per-call timeout
// ===========================================================================

#[test]
fn per_call_timeout_overrides_client_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (addr, captured) = capture_request_and_reply(
        listener,
        http_response(
            200,
            "OK",
            "Content-Type: application/json\r\n",
            &systemone_body(),
        ),
    );

    // Client has a 30s timeout, but per-call opts set 5s.
    let config = ClientConfig::new("test_key")
        .with_base_url(format!("http://{addr}"))
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(30))
        .with_retry(RetryPolicy::new(0));
    let client = TypeSafeClient::from_config(config).unwrap();

    let opts = typesafeai_sdk::SystemOneOpts::new().with_timeout(Duration::from_secs(5));

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _ = rt
        .block_on(async { client.system_one_with_opts(billing_question(), opts).await })
        .unwrap();

    // The request should succeed — we just verify the per-call timeout
    // doesn't break the flow.
    let request = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(request.contains("POST /v1/systemone"));
}

// ===========================================================================
// Per-call retry policy
// ===========================================================================

#[test]
fn per_call_retry_overrides_client_retry() {
    let server = MockServer::new();
    // First attempt: 429 with Retry-After; second attempt: 200.
    let handle = server.serve(vec![
        http_response(
            429,
            "Too Many Requests",
            "Content-Type: application/json\r\nRetry-After: 0\r\n",
            r#"{"error":{"message":"rate limited"}}"#,
        ),
        http_response(
            200,
            "OK",
            "Content-Type: application/json\r\n",
            &systemone_body(),
        ),
    ]);

    // Client has 0 retries (no retry), but per-call opts set 3 retries.
    let config = ClientConfig::new("test_key")
        .with_base_url(handle.url())
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(RetryPolicy::new(0));
    let client = TypeSafeClient::from_config(config).unwrap();

    let opts = typesafeai_sdk::SystemOneOpts::new()
        .with_retry(RetryPolicy::new(3).with_base_delay(Duration::from_millis(1)));

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt
        .block_on(async { client.system_one_with_opts(billing_question(), opts).await })
        .unwrap();

    // Should have retried and succeeded on the second attempt.
    assert_eq!(result.model, "jev-latest");
}

#[test]
fn per_call_retry_zero_overrides_client_retries() {
    let server = MockServer::new();
    // Return 429 on every attempt.
    let handle = server.serve(vec![
        http_response(
            429,
            "Too Many Requests",
            "Content-Type: application/json\r\nRetry-After: 0\r\n",
            r#"{"error":{"message":"rate limited"}}"#,
        ),
        http_response(
            429,
            "Too Many Requests",
            "Content-Type: application/json\r\nRetry-After: 0\r\n",
            r#"{"error":{"message":"rate limited"}}"#,
        ),
        http_response(
            429,
            "Too Many Requests",
            "Content-Type: application/json\r\nRetry-After: 0\r\n",
            r#"{"error":{"message":"rate limited"}}"#,
        ),
    ]);

    // Client has 3 retries, but per-call opts set 0 retries.
    let config = ClientConfig::new("test_key")
        .with_base_url(handle.url())
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(RetryPolicy::new(3).with_base_delay(Duration::from_millis(1)));
    let client = TypeSafeClient::from_config(config).unwrap();

    let opts = typesafeai_sdk::SystemOneOpts::new().with_retry(RetryPolicy::new(0));

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async { client.system_one_with_opts(billing_question(), opts).await });

    // Should NOT retry — the per-call 0-retry policy overrides the client's 3.
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(
        err.kind(),
        typesafeai_sdk::ErrorKind::RateLimit(_)
    ));
    // Only one request should have been made (no retries).
    assert_eq!(handle.request_count(), 1);
}

// ===========================================================================
// Custom reqwest::Client injection
// ===========================================================================

#[test]
fn custom_http_client_is_used() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (addr, captured) = capture_request_and_reply(
        listener,
        http_response(
            200,
            "OK",
            "Content-Type: application/json\r\n",
            &systemone_body(),
        ),
    );

    // Build a custom reqwest client with a distinctive User-Agent.
    let custom_http = reqwest::Client::builder()
        .user_agent("my-custom-agent/1.0")
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let config = ClientConfig::new("test_key")
        .with_base_url(format!("http://{addr}"))
        .with_default_model("jev-latest")
        .with_http_client(custom_http);
    let client = TypeSafeClient::from_config(config).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _ = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap();

    let request = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    let lower = request.to_ascii_lowercase();
    // The custom User-Agent should appear in the request.
    assert!(
        lower.contains("user-agent: my-custom-agent/1.0"),
        "custom user-agent not found in request:\n{request}"
    );
    // Bug #1 regression: auth header must be present even with a custom client.
    assert!(
        lower.contains("authorization: bearer test_key"),
        "auth header missing from custom-client request:\n{request}"
    );
    // Accept header must also be present.
    assert!(
        lower.contains("accept: application/json"),
        "accept header missing from custom-client request:\n{request}"
    );
}

// ===========================================================================
// extra_body last-write-wins (additional test)
// ===========================================================================

#[test]
fn extra_body_rejects_reserved_key_state_with_existing_state() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (addr, _captured) = capture_request_and_reply(
        listener,
        http_response(
            200,
            "OK",
            "Content-Type: application/json\r\n",
            &systemone_body(),
        ),
    );

    let config = ClientConfig::new("test_key")
        .with_base_url(format!("http://{addr}"))
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(RetryPolicy::new(0));
    let client = TypeSafeClient::from_config(config).unwrap();

    // Set state via opts, then try to override it via extra_body — must be
    // rejected because "state" is a reserved key.
    let opts = typesafeai_sdk::SystemOneOpts::new()
        .with_state("original state")
        .with_extra_body(serde_json::json!({"state": "overridden state"}));

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async { client.system_one_with_opts(billing_question(), opts).await });

    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("reserved") && msg.contains("state"),
        "expected reserved key rejection for state, got: {msg}"
    );
}

// ===========================================================================
// Bug #2: extra_body cannot bypass validation
// ===========================================================================

#[test]
fn extra_body_cannot_empty_questions() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: application/json\r\n",
        &systemone_body(),
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Override questions with an empty object via extra_body — should be
    // rejected because `questions` is a reserved key.
    let opts =
        typesafeai_sdk::SystemOneOpts::new().with_extra_body(serde_json::json!({"questions": {}}));

    let result = rt.block_on(async { client.system_one_with_opts(billing_question(), opts).await });

    let err = result.unwrap_err();
    assert!(
        matches!(err.kind(), ErrorKind::Validation(msg) if msg.contains("reserved") && msg.contains("questions")),
        "expected Validation error about reserved key `questions`, got: {err:?}"
    );
}

#[test]
fn extra_body_cannot_empty_model() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: application/json\r\n",
        &systemone_body(),
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Override model with an empty string via extra_body — should be
    // rejected because `model` is a reserved key.
    let opts =
        typesafeai_sdk::SystemOneOpts::new().with_extra_body(serde_json::json!({"model": ""}));

    let result = rt.block_on(async { client.system_one_with_opts(billing_question(), opts).await });

    let err = result.unwrap_err();
    assert!(
        matches!(err.kind(), ErrorKind::Validation(msg) if msg.contains("reserved") && msg.contains("model")),
        "expected Validation error about reserved key `model`, got: {err:?}"
    );
}

#[test]
fn extra_body_rejects_non_object() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: application/json\r\n",
        &systemone_body(),
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();

    // A non-object extra_body (array) should be rejected.
    let opts = typesafeai_sdk::SystemOneOpts::new().with_extra_body(serde_json::json!([1, 2, 3]));

    let result = rt.block_on(async { client.system_one_with_opts(billing_question(), opts).await });

    let err = result.unwrap_err();
    assert!(
        matches!(err.kind(), ErrorKind::Validation(msg) if msg.contains("extra_body")),
        "expected Validation error about extra_body, got: {err:?}"
    );
}

#[test]
fn extra_body_rejects_reserved_key_state() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: application/json\r\n",
        &systemone_body(),
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();

    let opts =
        typesafeai_sdk::SystemOneOpts::new().with_extra_body(serde_json::json!({"state": "oops"}));

    let result = rt.block_on(async { client.system_one_with_opts(billing_question(), opts).await });

    let err = result.unwrap_err();
    assert!(
        matches!(err.kind(), ErrorKind::Validation(msg) if msg.contains("reserved") && msg.contains("state")),
        "expected Validation error about reserved key `state`, got: {err:?}"
    );
}

#[test]
fn extra_body_rejects_non_string_model() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: application/json\r\n",
        &systemone_body(),
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();

    // `model` is a reserved key — any value (not just empty string) is rejected.
    let opts =
        typesafeai_sdk::SystemOneOpts::new().with_extra_body(serde_json::json!({"model": 42}));

    let result = rt.block_on(async { client.system_one_with_opts(billing_question(), opts).await });

    let err = result.unwrap_err();
    assert!(
        matches!(err.kind(), ErrorKind::Validation(msg) if msg.contains("reserved")),
        "expected Validation error about reserved key, got: {err:?}"
    );
}

#[test]
fn extra_body_rejects_wrong_type_questions() {
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: application/json\r\n",
        &systemone_body(),
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();

    // `questions` is a reserved key — any type (string, array, etc.) is rejected.
    let opts = typesafeai_sdk::SystemOneOpts::new()
        .with_extra_body(serde_json::json!({"questions": "oops"}));

    let result = rt.block_on(async { client.system_one_with_opts(billing_question(), opts).await });

    let err = result.unwrap_err();
    assert!(
        matches!(err.kind(), ErrorKind::Validation(msg) if msg.contains("reserved")),
        "expected Validation error about reserved key, got: {err:?}"
    );
}

// ===========================================================================
// Bug #5: response body size cap
// ===========================================================================

#[test]
fn response_body_exceeding_cap_returns_error() {
    // Build a response body larger than 1 MiB.
    let large_body = "x".repeat(1024 * 1024 + 100);
    let server = MockServer::new();
    let handle = server.serve(vec![http_response(
        200,
        "OK",
        "Content-Type: text/plain\r\n",
        &large_body,
    )]);

    let client = test_client(&handle.url());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async { client.list_models().await });

    let err = result.unwrap_err();
    // Should be a transport error (body too large), not a successful parse.
    assert!(
        matches!(err.kind(), ErrorKind::Transport(msg) if msg.contains("maximum size")),
        "expected Transport error about body size, got: {err:?}"
    );
}

// ===========================================================================
// Bug #6: no redirects
// ===========================================================================

#[test]
fn does_not_follow_redirects() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    let redirect_target = format!("http://{addr}/evil");
    let redirect_response = format!(
        "HTTP/1.1 307 Temporary Redirect\r\n\
         Location: {redirect_target}\r\n\
         Content-Length: 0\r\n\
         \r\n"
    );

    let (addr, _captured) = capture_request_and_reply(listener, redirect_response);

    let config = ClientConfig::new("test_key")
        .with_base_url(format!("http://{addr}"))
        .with_default_model("jev-latest")
        .with_timeout(Duration::from_secs(5))
        .with_retry(RetryPolicy::new(0));
    let client = TypeSafeClient::from_config(config).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async { client.list_models().await });

    // The client should NOT follow the redirect. A 307 with no body
    // should result in an error (either a parse error or an API error
    // for the unexpected status), but NOT a successful response from
    // the redirect target.
    assert!(
        result.is_err(),
        "expected error for 307 redirect (should not be followed), but got success"
    );
}
