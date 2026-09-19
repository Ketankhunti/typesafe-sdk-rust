//! The TypeSafe AI API client.
//!
//! Mirrors the Python `TypeSafeClient` and JavaScript `TypeSafeClient`:
//! a client struct with configurable API key, base URL, default model,
//! timeout, and retry policy. The primary method is [`TypeSafeClient::system_one`],
//! which sends a `POST /v1/systemone` request and returns a typed
//! [`SystemOneResponse`].
//!
//! # Example
//! ```no_run
//! use typesafe_sdk::{TypeSafeClient, noul, choice, score};
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let client = TypeSafeClient::from_env()?;
//!
//! let response = client.system_one(
//!     "I was charged twice. Please help.",
//!     [
//!         ("billing".to_string(), noul("Is this about billing?").into()),
//!         ("tone".to_string(), choice("What is the tone?", [
//!             ("calm".to_string(), None),
//!             ("angry".to_string(), None),
//!         ].into()).into()),
//!     ].into(),
//! ).await?;
//!
//! if let Some(answer) = response.noul("billing") {
//!     println!("{}", answer.noul);
//! }
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::env;
use std::fmt;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, RETRY_AFTER};
use reqwest::{Client as HttpClient, Method, StatusCode};
use serde_json::Value;

use crate::error::{Result, TypeSafeError};
use crate::questions::{validate_questions, Question};
use crate::retry::{jitter_seed, RetryPolicy};
use crate::types::{ListModelsResponse, SystemOneRequest, SystemOneResponse};

const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
const DEFAULT_MODEL: &str = "jev-latest";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const SDK_USER_AGENT: &str = concat!("typesafe-sdk/", env!("CARGO_PKG_VERSION"));
const SYSTEM_ONE_PATH: &str = "/v1/systemone";
const MODELS_PATH: &str = "/v1/models";
/// Maximum number of characters of a response body echoed into an error.
const MAX_ERROR_BODY_CHARS: usize = 512;

/// Configuration for constructing a [`TypeSafeClient`].
///
/// All fields are public for construction, but prefer [`ClientConfig::default`]
/// or the builder methods on [`RetryPolicy`] for common cases.
#[derive(Clone)]
pub struct ClientConfig {
    /// API key for authentication.
    ///
    /// Set this directly or use [`TypeSafeClient::from_env`] to read it
    /// from the `TYPESAFE_API_KEY` environment variable.
    pub api_key: String,
    /// API root. Defaults to `https://api.typesafe.ai`.
    ///
    /// A base path is supported and preserved: e.g. `https://example.com/api`
    /// produces endpoints like `https://example.com/api/v1/systemone`.
    /// Credentials, query parameters, and fragments are rejected.
    pub base_url: String,
    /// Default model. Defaults to `jev-latest`.
    pub default_model: String,
    /// Per-attempt timeout. Defaults to 10 seconds.
    pub timeout: Duration,
    /// Retry policy. Defaults to 2 retries with exponential backoff + jitter.
    pub retry: RetryPolicy,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
            default_model: DEFAULT_MODEL.to_string(),
            timeout: DEFAULT_TIMEOUT,
            retry: RetryPolicy::default(),
        }
    }
}

impl fmt::Debug for ClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientConfig")
            .field("api_key", &"<redacted>")
            .field("base_url", &self.base_url)
            .field("default_model", &self.default_model)
            .field("timeout", &self.timeout)
            .field("retry", &self.retry)
            .finish()
    }
}

/// Client for the TypeSafe AI API.
///
/// Construct with [`TypeSafeClient::new`] or [`TypeSafeClient::from_env`].
/// Cloning is cheap and shares the underlying connection pool.
#[derive(Clone)]
pub struct TypeSafeClient {
    config: ClientConfig,
    http: HttpClient,
}

impl fmt::Debug for TypeSafeClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypeSafeClient")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// A failed attempt, carrying the server's `Retry-After` hint (if any) so the
/// retry loop can honour it. Internal: the public API only exposes
/// [`TypeSafeError`].
struct Failure {
    error: TypeSafeError,
    retry_after: Option<Duration>,
}

impl From<TypeSafeError> for Failure {
    fn from(error: TypeSafeError) -> Self {
        Self {
            error,
            retry_after: None,
        }
    }
}

impl TypeSafeClient {
    /// Create a new client with the given API key and default settings.
    #[must_use = "the returned client should be used to make API calls"]
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        let config = ClientConfig {
            api_key: api_key.into(),
            ..ClientConfig::default()
        };
        Self::from_config(config)
    }

    /// Create a new client from a full [`ClientConfig`].
    #[must_use = "the returned client should be used to make API calls"]
    pub fn from_config(mut config: ClientConfig) -> Result<Self> {
        // Trim whitespace: a stray newline from an env file would otherwise
        // become an opaque transport error.
        config.api_key = config.api_key.trim().to_string();

        if config.api_key.is_empty() {
            return Err(TypeSafeError::Validation(
                "No API key was provided. Pass an API key to `TypeSafeClient::new` or set the `TYPESAFE_API_KEY` environment variable."
                    .to_string(),
            ));
        }

        // Normalize the base URL: trim trailing slash and validate scheme/host.
        config.base_url = config.base_url.trim().trim_end_matches('/').to_string();
        validate_base_url(&config.base_url)?;

        // Validate the default model name.
        if config.default_model.trim().is_empty() {
            return Err(TypeSafeError::Validation(
                "Default model name cannot be empty.".to_string(),
            ));
        }

        // Build the auth header once. Marking it sensitive keeps the key out
        // of reqwest's debug output.
        let mut auth =
            HeaderValue::from_str(&format!("Bearer {}", config.api_key)).map_err(|_| {
                TypeSafeError::Validation(
                    "The API key contains characters that are not valid in an HTTP header."
                        .to_string(),
                )
            })?;
        auth.set_sensitive(true);

        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, auth);
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            HeaderName::from_static("x-typesafe-sdk"),
            HeaderValue::from_static(SDK_USER_AGENT),
        );

        let http = HttpClient::builder()
            .timeout(config.timeout)
            .user_agent(SDK_USER_AGENT)
            .default_headers(headers)
            .build()
            .map_err(TypeSafeError::Transport)?;

        Ok(Self { config, http })
    }

    /// Create a new client, reading the API key from the `TYPESAFE_API_KEY`
    /// environment variable. Also reads `TYPESAFE_BASE_URL` and
    /// `TYPESAFE_DEFAULT_MODEL` if set.
    #[must_use = "the returned client should be used to make API calls"]
    pub fn from_env() -> Result<Self> {
        let api_key = env::var("TYPESAFE_API_KEY").unwrap_or_default();
        let mut config = ClientConfig {
            api_key,
            ..ClientConfig::default()
        };

        if let Ok(base) = env::var("TYPESAFE_BASE_URL") {
            if !base.trim().is_empty() {
                config.base_url = base;
            }
        }

        if let Ok(model) = env::var("TYPESAFE_DEFAULT_MODEL") {
            if !model.trim().is_empty() {
                config.default_model = model;
            }
        }

        Self::from_config(config)
    }

    /// Returns the base URL this client is configured to use.
    pub fn base_url(&self) -> &str {
        &self.config.base_url
    }

    /// Returns the default model this client uses.
    pub fn default_model(&self) -> &str {
        &self.config.default_model
    }

    // -----------------------------------------------------------------------
    // system_one
    // -----------------------------------------------------------------------

    /// Answer named questions about text or structured state.
    ///
    /// Sends a `POST /v1/systemone` request and returns a typed
    /// [`SystemOneResponse`].
    ///
    /// # Arguments
    /// - `state` — text, a JSON object, or an array to evaluate. Accepts
    ///   `&str`, `String`, or any `serde_json::Value`.
    /// - `questions` — a non-empty map of question names to [`Question`]s.
    ///
    /// Use [`system_one_with_model`](Self::system_one_with_model) to override
    /// the client's default model.
    ///
    /// # Retry safety
    ///
    /// This is a `POST` request. The SDK automatically retries on transient
    /// failures (429, 5xx, connection errors, timeouts), but a timeout does
    /// not guarantee the server did not process the request. If the endpoint
    /// has side effects (credit consumption, usage recording, downstream
    /// triggers), a retry may result in duplicate processing. Future versions
    /// will support an `Idempotency-Key` header to make retries safe.
    ///
    /// To avoid replaying a POST whose body the server already processed,
    /// errors that occur while *reading the response body* — including
    /// timeouts and truncated bodies — are classified as non-retryable
    /// [`TypeSafeError::Transport`] rather than [`TypeSafeError::Connection`]
    /// or [`TypeSafeError::Timeout`].
    ///
    /// # Errors
    /// - [`TypeSafeError::Validation`] if questions are empty or a choice or
    ///   score question has fewer than two criteria.
    /// - [`TypeSafeError::Authentication`] if the API key is invalid.
    /// - [`TypeSafeError::RateLimit`] if rate-limited (after retries).
    /// - [`TypeSafeError::Connection`] / [`TypeSafeError::Timeout`] on
    ///   network failures (after retries).
    pub async fn system_one(
        &self,
        state: impl Into<Value>,
        questions: HashMap<String, Question>,
    ) -> Result<SystemOneResponse> {
        self.system_one_with_model(state, questions, None).await
    }

    /// Like [`system_one`](Self::system_one) but with an explicit model
    /// override (`None` uses the client default).
    pub async fn system_one_with_model(
        &self,
        state: impl Into<Value>,
        questions: HashMap<String, Question>,
        model: Option<&str>,
    ) -> Result<SystemOneResponse> {
        validate_questions(&questions)?;

        let model_name = model.unwrap_or(&self.config.default_model);
        if model_name.trim().is_empty() {
            return Err(TypeSafeError::Validation(
                "Model name cannot be empty.".to_string(),
            ));
        }

        let request = SystemOneRequest {
            state: state.into(),
            model: model_name.to_string(),
            questions,
        };

        let body = serde_json::to_value(&request)?;

        self.request_with_retry(Method::POST, SYSTEM_ONE_PATH, Some(body))
            .await
    }

    // -----------------------------------------------------------------------
    // models
    // -----------------------------------------------------------------------

    /// List available models. Sends `GET /v1/models`.
    pub async fn list_models(&self) -> Result<ListModelsResponse> {
        self.request_with_retry(Method::GET, MODELS_PATH, None)
            .await
    }

    // -----------------------------------------------------------------------
    // Internal HTTP + retry
    // -----------------------------------------------------------------------

    async fn request_with_retry<T: serde::de::DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T> {
        let url = format!("{}{}", self.config.base_url, path);
        let retry = &self.config.retry;

        let mut attempt = 0;
        loop {
            match self.attempt_request(&method, &url, body.as_ref()).await {
                Ok(resp) => return Ok(resp),
                Err(failure) => {
                    if !failure.error.is_retryable() || attempt >= retry.max_retries {
                        return Err(failure.error);
                    }

                    let delay = match failure.retry_after {
                        // Honour the server's hint, unless it asks us to wait
                        // longer than `max_delay`: then give up instead of
                        // stalling the caller.
                        Some(hint) => match retry.delay_for_retry_after(attempt, hint) {
                            Some(delay) => delay,
                            None => return Err(failure.error),
                        },
                        None => retry.delay_for_with_jitter(attempt, jitter_seed()),
                    };

                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
            }
        }
    }

    async fn attempt_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &Method,
        url: &str,
        body: Option<&Value>,
    ) -> std::result::Result<T, Failure> {
        // Auth, Accept, User-Agent and X-TypeSafe-SDK are default headers set
        // once in `from_config`; `.json()` sets Content-Type.
        let mut request = self.http.request(method.clone(), url);
        if let Some(b) = body {
            request = request.json(b);
        }

        let response = request
            .send()
            .await
            .map_err(|e| map_reqwest_error(e, self.config.timeout))?;

        self.parse_response(response).await
    }

    async fn parse_response<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> std::result::Result<T, Failure> {
        let status = response.status();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_retry_after);

        let text = response.text().await.map_err(map_body_error)?;

        if status.is_success() {
            return serde_json::from_str(&text).map_err(|e| {
                Failure::from(TypeSafeError::ResponseValidation(format!(
                    "Failed to parse response body: {e}\nBody: {}",
                    truncate(&text, MAX_ERROR_BODY_CHARS)
                )))
            });
        }

        // Map HTTP status codes to typed errors, mirroring the Python/JS SDKs.
        let message = Self::extract_error_message(&text)
            .unwrap_or_else(|| truncate(&text, MAX_ERROR_BODY_CHARS));
        let error = match status {
            StatusCode::UNAUTHORIZED => TypeSafeError::Authentication(message),
            StatusCode::BAD_REQUEST => TypeSafeError::BadRequest(message),
            StatusCode::NOT_FOUND => TypeSafeError::NotFound(message),
            StatusCode::UNPROCESSABLE_ENTITY => TypeSafeError::UnprocessableEntity(message),
            StatusCode::TOO_MANY_REQUESTS => TypeSafeError::RateLimit(message),
            StatusCode::INTERNAL_SERVER_ERROR
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT => TypeSafeError::InternalServer(message),
            s if s.as_u16() == 529 => TypeSafeError::Overloaded(message),
            _ => TypeSafeError::Api {
                status: status.as_u16(),
                message,
            },
        };
        Err(Failure { error, retry_after })
    }

    /// Try to extract a human-readable error message from the response body.
    /// The API typically returns `{"error": {"message": "..."}}` or
    /// `{"detail": "..."}`.
    fn extract_error_message(text: &str) -> Option<String> {
        let value: Value = serde_json::from_str(text).ok()?;
        let obj = value.as_object()?;
        if let Some(error) = obj.get("error") {
            if let Some(msg) = error.get("message").and_then(|m| m.as_str()) {
                return Some(msg.to_string());
            }
            if let Some(msg) = error.as_str() {
                return Some(msg.to_string());
            }
        }
        if let Some(detail) = obj.get("detail").and_then(|d| d.as_str()) {
            return Some(detail.to_string());
        }
        if let Some(message) = obj.get("message").and_then(|m| m.as_str()) {
            return Some(message.to_string());
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map a `reqwest` error to the appropriate `TypeSafeError` variant.
///
/// - timeouts -> [`TypeSafeError::Timeout`], reporting the *configured* timeout
/// - connect failures (refused, reset, DNS) -> [`TypeSafeError::Connection`]
/// - everything else (body read, builder, redirect, decode, request
///   construction) is deterministic or occurs after the server has already
///   processed the request, so it stays a non-retryable
///   [`TypeSafeError::Transport`]
///
/// `is_request()` and `is_body()` are intentionally **not** classified as
/// `Connection`. `is_request()` covers the entire request lifecycle, including
/// non-retryable errors like builder failures, redirect loops, and decode
/// errors. `is_body()` indicates a failure while *reading the response body*,
/// which happens **after** the server has already received and processed the
/// request — retrying a POST in that state risks duplicate side effects.
/// Only `is_connect()` (pre-request network failure) is retryable.
fn map_reqwest_error(e: reqwest::Error, timeout: Duration) -> TypeSafeError {
    if e.is_timeout() {
        TypeSafeError::Timeout(timeout)
    } else if e.is_connect() {
        TypeSafeError::Connection(e.to_string())
    } else {
        TypeSafeError::Transport(e)
    }
}

/// Map an error that occurred while *reading the response body*.
///
/// By the time we're reading the body, the server has already received and
/// processed the request. Any failure here — timeout, truncated body, or
/// connection reset — must be non-retryable to avoid replaying a POST whose
/// side effects the server may have already applied.
fn map_body_error(e: reqwest::Error) -> TypeSafeError {
    TypeSafeError::Transport(e)
}

/// Parse a `Retry-After` header value given in seconds.
///
/// The HTTP-date form is not supported (it needs a date parser); such values
/// return `None`, and the SDK falls back to its own backoff.
fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Truncate a string to at most `max_chars` *characters*, appending "..." if
/// anything was cut. Slicing at a char boundary means non-ASCII bodies can't
/// cause a panic.
fn truncate(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => format!("{}...", &s[..idx]),
        None => s.to_string(),
    }
}

/// Validate the base URL: it must be `https`, or `http` only for a loopback
/// host (development/testing). The URL is parsed rather than prefix-matched so
/// look-alikes such as `http://localhost.evil.example` or
/// `http://localhost@evil.example` are rejected.
fn validate_base_url(raw: &str) -> Result<()> {
    let url = reqwest::Url::parse(raw)
        .map_err(|e| TypeSafeError::Validation(format!("Base URL is not a valid URL: {e}")))?;

    // Reject credentials, query parameters, and fragments — a base URL
    // should be a bare origin + optional path.
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(TypeSafeError::Validation(
            "Base URL must not contain credentials, query parameters, or fragments.".to_string(),
        ));
    }

    match (url.scheme(), url.host_str()) {
        ("https", Some(_)) => Ok(()),
        ("http", Some("localhost" | "127.0.0.1" | "::1" | "[::1]")) => Ok(()),
        (scheme, _) => Err(TypeSafeError::Validation(format!(
            "Base URL must use https:// (or http:// for localhost/127.0.0.1/::1 during development); got scheme `{scheme}` and an unsupported host."
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_requires_api_key() {
        let result = TypeSafeClient::new("");
        assert!(result.is_err());
    }

    #[test]
    fn from_config_requires_api_key() {
        let config = ClientConfig::default();
        let result = TypeSafeClient::from_config(config);
        assert!(result.is_err());
    }

    #[test]
    fn extract_error_message_from_nested_object() {
        let text = r#"{"error":{"message":"Invalid API key"}}"#;
        let msg = TypeSafeClient::extract_error_message(text).unwrap();
        assert_eq!(msg, "Invalid API key");
    }

    #[test]
    fn extract_error_message_from_detail() {
        let text = r#"{"detail":"Not found"}"#;
        let msg = TypeSafeClient::extract_error_message(text).unwrap();
        assert_eq!(msg, "Not found");
    }

    #[test]
    fn extract_error_message_returns_none_for_plain_text() {
        let text = "plain text error";
        assert!(TypeSafeClient::extract_error_message(text).is_none());
    }

    #[test]
    fn debug_redacts_api_key() {
        let config = ClientConfig {
            api_key: "secret_key_123".to_string(),
            ..ClientConfig::default()
        };
        let debug_str = format!("{config:?}");
        assert!(!debug_str.contains("secret_key_123"));
        assert!(debug_str.contains("<redacted>"));
    }

    #[test]
    fn client_debug_does_not_leak_api_key() {
        let client = TypeSafeClient::new("secret_key_123").unwrap();
        let debug_str = format!("{client:?}");
        assert!(!debug_str.contains("secret_key_123"));
    }

    #[test]
    fn trims_api_key_whitespace() {
        // A stray newline from an env file should be trimmed.
        let client = TypeSafeClient::new("  apikey_test  \n").unwrap();
        assert_eq!(client.config.api_key, "apikey_test");
    }

    #[test]
    fn rejects_api_key_that_is_not_a_valid_header_value() {
        let err = TypeSafeClient::new("bad\nkey").unwrap_err();
        assert!(matches!(err, TypeSafeError::Validation(_)), "got {err:?}");
    }

    #[test]
    fn rejects_http_base_url() {
        let config = ClientConfig {
            api_key: "test".to_string(),
            base_url: "http://api.typesafe.ai".to_string(),
            ..ClientConfig::default()
        };
        assert!(TypeSafeClient::from_config(config).is_err());
    }

    #[test]
    fn allows_localhost_http() {
        let config = ClientConfig {
            api_key: "test".to_string(),
            base_url: "http://localhost:8080".to_string(),
            ..ClientConfig::default()
        };
        assert!(TypeSafeClient::from_config(config).is_ok());
    }

    #[test]
    fn trims_trailing_slash_from_base_url() {
        let config = ClientConfig {
            api_key: "test".to_string(),
            base_url: "https://api.typesafe.ai/".to_string(),
            ..ClientConfig::default()
        };
        let client = TypeSafeClient::from_config(config).unwrap();
        assert_eq!(client.base_url(), "https://api.typesafe.ai");
    }

    /// Regression: a `starts_with("http://localhost")` check accepted these
    /// look-alikes, sending the API key in plaintext to an attacker's host.
    #[test]
    fn base_url_rejects_localhost_lookalikes() {
        for bad in [
            "http://localhost.evil.example",
            "http://127.0.0.1.evil.example/v1",
            "http://localhost@evil.example",
            "http://localhostevil.example",
            "ftp://localhost",
            "not a url",
            "",
        ] {
            assert!(
                validate_base_url(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn base_url_accepts_valid_urls() {
        for good in [
            "https://api.typesafe.ai",
            "HTTPS://API.TYPESAFE.AI",
            "https://api.typesafe.ai/prefix",
            "http://localhost",
            "http://localhost:8080",
            "http://LOCALHOST:8080",
            "http://127.0.0.1:9000",
            "http://[::1]:9000",
        ] {
            assert!(
                validate_base_url(good).is_ok(),
                "{good:?} should be accepted"
            );
        }
    }

    #[test]
    fn truncate_shortens_long_strings() {
        let long = "x".repeat(600);
        let truncated = truncate(&long, 512);
        assert_eq!(truncated.len(), 515); // 512 + "..."
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn truncate_leaves_short_strings_alone() {
        assert_eq!(truncate("hello", 512), "hello");
        assert_eq!(truncate("exactly", 7), "exactly");
    }

    /// Regression: byte-slicing at `max` panicked when it fell inside a
    /// multi-byte character (512 % 3 != 0 for "€").
    #[test]
    fn truncate_handles_multibyte_characters() {
        let long = "€".repeat(600);
        let truncated = truncate(&long, 512);
        assert_eq!(truncated.chars().count(), 515); // 512 chars + "..."
        assert!(truncated.ends_with("..."));

        // Emoji (4 bytes) and mixed scripts, at many cut points.
        let mixed = "héllo wörld 日本語 🙂".repeat(20);
        for max in 0..mixed.chars().count() + 2 {
            let _ = truncate(&mixed, max); // must not panic
        }
    }

    #[test]
    fn parse_retry_after_seconds() {
        assert_eq!(parse_retry_after("5"), Some(Duration::from_secs(5)));
        assert_eq!(parse_retry_after("  10  "), Some(Duration::from_secs(10)));
        assert_eq!(parse_retry_after("not a number"), None);
        // HTTP-date form is intentionally unsupported.
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
    }

    /// Builder errors are deterministic, so they must map to the
    /// non-retryable `Transport` variant, not `Connection`.
    #[test]
    fn builder_errors_are_not_retryable() {
        let err = reqwest::Client::new().get("not a url").build().unwrap_err();
        let mapped = map_reqwest_error(err, Duration::from_secs(1));
        assert!(
            matches!(mapped, TypeSafeError::Transport(_)),
            "got {mapped:?}"
        );
        assert!(!mapped.is_retryable());
    }

    /// IPv6 loopback `::1` should be accepted (reqwest's `host_str()` returns
    /// `::1` without brackets).
    #[test]
    fn base_url_accepts_ipv6_loopback() {
        assert!(validate_base_url("http://[::1]:8080").is_ok());
        assert!(validate_base_url("http://[::1]").is_ok());
    }

    #[test]
    fn from_config_rejects_empty_default_model() {
        let config = ClientConfig {
            api_key: "test".to_string(),
            default_model: "  ".to_string(),
            ..ClientConfig::default()
        };
        let err = TypeSafeClient::from_config(config).unwrap_err();
        assert!(
            matches!(err, TypeSafeError::Validation(ref m) if m.contains("model")),
            "got {err:?}"
        );
    }

    #[test]
    fn timeout_error_preserves_subsecond_duration() {
        let err = TypeSafeError::Timeout(Duration::from_millis(150));
        let msg = err.to_string();
        assert!(
            msg.contains("150ms"),
            "expected 150ms in message, got: {msg}"
        );
    }

    #[test]
    fn base_url_rejects_credentials() {
        assert!(validate_base_url("https://user:password@api.typesafe.ai").is_err());
        assert!(validate_base_url("https://user@api.typesafe.ai").is_err());
    }

    #[test]
    fn base_url_rejects_query_and_fragment() {
        assert!(validate_base_url("https://api.typesafe.ai?foo=bar").is_err());
        assert!(validate_base_url("https://api.typesafe.ai#fragment").is_err());
    }

    #[test]
    fn base_url_accepts_path() {
        assert!(validate_base_url("https://api.typesafe.ai/api").is_ok());
        assert!(validate_base_url("https://api.typesafe.ai/v1/").is_ok());
    }
}
