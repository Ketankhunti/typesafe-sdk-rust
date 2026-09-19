//! Integration tests for the TypeSafe SDK client.
//!
//! These tests spin up a raw TCP mock server (no external HTTP framework
//! needed) and exercise the client's retry, error-mapping, and header
//! behaviour end-to-end.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use typesafe_sdk::{ClientConfig, RetryPolicy, TypeSafeClient, TypeSafeError};

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
        let handle = ServerHandle {
            addr,
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
    _shutdown: Arc<()>,
}

impl ServerHandle {
    fn url(&self) -> String {
        format!("http://{}", self.addr)
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
    let config = ClientConfig {
        api_key: "test_key".to_string(),
        base_url: url.to_string(),
        default_model: "jev-latest".to_string(),
        timeout: Duration::from_secs(5),
        retry: RetryPolicy::new(3)
            .with_base_delay(Duration::from_millis(50))
            .with_max_delay(Duration::from_secs(5)),
    };
    TypeSafeClient::from_config(config).expect("client")
}

/// Create a single-question map for system_one calls.
fn billing_question() -> std::collections::HashMap<String, typesafe_sdk::Question> {
    let mut q = std::collections::HashMap::new();
    q.insert(
        "billing".to_string(),
        typesafe_sdk::noul("Is this about billing?").into(),
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
    assert!(matches!(err, TypeSafeError::RateLimit(_)), "got {err:?}");
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
        matches!(err, TypeSafeError::Authentication(_)),
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

    assert!(matches!(err, TypeSafeError::BadRequest(_)), "got {err:?}");
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

    assert!(matches!(err, TypeSafeError::NotFound(_)), "got {err:?}");
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
        matches!(err, TypeSafeError::UnprocessableEntity(_)),
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
        matches!(err, TypeSafeError::InternalServer(_)),
        "got {err:?}"
    );
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

    assert!(matches!(err, TypeSafeError::Overloaded(_)), "got {err:?}");
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

    match err {
        TypeSafeError::Api { status, .. } => assert_eq!(status, 418),
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
    assert!(matches!(err, TypeSafeError::Connection(_)), "got {err:?}");
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
                std::thread::sleep(Duration::from_secs(30));
            }
        }
    });

    let config = ClientConfig {
        api_key: "test_key".to_string(),
        base_url: format!("http://{addr}"),
        default_model: "jev-latest".to_string(),
        timeout: Duration::from_millis(100),
        retry: RetryPolicy::new(0),
    };
    let client = TypeSafeClient::from_config(config).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap_err();

    assert!(matches!(err, TypeSafeError::Timeout(_)), "got {err:?}");
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
    match err {
        TypeSafeError::BadRequest(msg) => assert!(msg.contains("coût")),
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

    let config = ClientConfig {
        api_key: "my_secret_key".to_string(),
        base_url: format!("http://{addr}"),
        default_model: "jev-latest".to_string(),
        timeout: Duration::from_secs(5),
        retry: RetryPolicy::new(0),
    };
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

    let config = ClientConfig {
        api_key: "test_key".to_string(),
        base_url: format!("http://{addr}"),
        default_model: "jev-latest".to_string(),
        timeout: Duration::from_secs(5),
        retry: RetryPolicy::new(0),
    };
    let client = TypeSafeClient::from_config(config).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _ = rt
        .block_on(async { client.system_one("test", billing_question()).await })
        .unwrap();

    let request = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(
        request.contains("typesafe-sdk/"),
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

    let config = ClientConfig {
        api_key: "test_key".to_string(),
        base_url: format!("http://{addr}"),
        default_model: "jev-latest".to_string(),
        timeout: Duration::from_secs(5),
        retry: RetryPolicy::new(0),
    };
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
        matches!(err, TypeSafeError::Validation(ref m) if m.contains("Model name")),
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
        typesafe_sdk::noul("Is this about billing?").into(),
    );

    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(async { client.system_one("test", questions).await })
        .unwrap_err();

    assert!(
        matches!(err, TypeSafeError::Validation(ref m) if m.contains("Question names")),
        "got {err:?}"
    );
}
