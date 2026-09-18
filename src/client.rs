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
//! println!("{}", response.nouls()["billing"].noul);
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::env;
use std::fmt;
use std::time::Duration;

use reqwest::{Client as HttpClient, StatusCode};
use serde_json::Value;

use crate::error::{Result, TypeSafeError};
use crate::questions::{validate_questions, Question};
use crate::retry::RetryPolicy;
use crate::types::{ListModelsResponse, SystemOneRequest, SystemOneResponse};

const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
const DEFAULT_MODEL: &str = "jev-latest";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");
const SYSTEM_ONE_PATH: &str = "/v1/systemone";
const MODELS_PATH: &str = "/v1/models";
const MAX_ERROR_BODY_LEN: usize = 512;

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
        // Trim whitespace — a stray newline from an env file would otherwise
        // become an opaque transport error.
        config.api_key = config.api_key.trim().to_string();

        if config.api_key.is_empty() {
            return Err(TypeSafeError::Validation(
                "No API key was provided. Pass an API key to `TypeSafeClient::new` or set the `TYPESAFE_API_KEY` environment variable."
                    .to_string(),
            ));
        }

        // Normalize the base URL: trim trailing slash and validate scheme.
        config.base_url = config.base_url.trim().trim_end_matches('/').to_string();
        validate_base_url(&config.base_url)?;

        let http = HttpClient::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|e| TypeSafeError::Connection(e.to_string()))?;

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
    /// - `model` — optional model override; `None` uses the client default.
    ///
    /// # Errors
    /// - [`TypeSafeError::Validation`] if questions are empty or a score
    ///   question has fewer than two criteria.
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
    /// override.
    pub async fn system_one_with_model(
        &self,
        state: impl Into<Value>,
        questions: HashMap<String, Question>,
        model: Option<&str>,
    ) -> Result<SystemOneResponse> {
        validate_questions(&questions)?;

        let request = SystemOneRequest {
            state: state.into(),
            model: model.unwrap_or(&self.config.default_model).to_string(),
            questions,
        };

        let body = serde_json::to_value(&request)?;

        self.request_with_retry("POST", SYSTEM_ONE_PATH, Some(body))
            .await
    }

    // -----------------------------------------------------------------------
    // models
    // -----------------------------------------------------------------------

    /// List available models. Sends `GET /v1/models`.
    pub async fn list_models(&self) -> Result<ListModelsResponse> {
        self.request_with_retry("GET", MODELS_PATH, None).await
    }

    // -----------------------------------------------------------------------
    // Internal HTTP + retry
    // -----------------------------------------------------------------------

    async fn request_with_retry<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<T> {
        let url = format!("{}{}", self.config.base_url, path);
        let max_retries = self.config.retry.max_retries;

        let mut attempt = 0;
        loop {
            let result = self.attempt_request(method, &url, body.as_ref()).await;
            match result {
                Ok(resp) => return Ok(resp),
                Err(e) if e.is_retryable() && attempt < max_retries => {
                    let delay = self
                        .config
                        .retry
                        .delay_for_with_jitter(attempt, attempt as u64);
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn attempt_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        url: &str,
        body: Option<&Value>,
    ) -> Result<T> {
        let auth_value = format!("Bearer {}", self.config.api_key);
        let auth_header = reqwest::header::HeaderValue::from_str(&auth_value)
            .map_err(|e| TypeSafeError::Validation(format!("Invalid API key header value: {e}")))?;

        let mut request = self
            .http
            .request(method.parse().unwrap_or(reqwest::Method::GET), url)
            .header(reqwest::header::AUTHORIZATION, auth_header)
            .header("Accept", "application/json")
            .header("User-Agent", format!("typesafe-sdk/{}", SDK_VERSION))
            .header("X-TypeSafe-SDK", format!("typesafe-sdk/{}", SDK_VERSION));

        if method == "POST" {
            request = request.header("Content-Type", "application/json");
            if let Some(b) = body {
                request = request.json(b);
            }
        }

        let response = request.send().await.map_err(map_send_error)?;

        Self::parse_response(response).await
    }

    async fn parse_response<T: serde::de::DeserializeOwned>(
        response: reqwest::Response,
    ) -> Result<T> {
        let status = response.status();

        let text = response
            .text()
            .await
            .map_err(|e| TypeSafeError::Connection(format!("Failed to read response body: {e}")))?;

        if status.is_success() {
            return serde_json::from_str(&text).map_err(|e| {
                TypeSafeError::ResponseValidation(format!(
                    "Failed to parse response body: {e}\nBody: {}",
                    truncate(&text, MAX_ERROR_BODY_LEN)
                ))
            });
        }

        // Map HTTP status codes to typed errors, mirroring the Python/JS SDKs.
        let message = Self::extract_error_message(&text)
            .unwrap_or_else(|| truncate(&text, MAX_ERROR_BODY_LEN));
        Err(match status {
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
        })
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

/// Map a `reqwest` send error to the appropriate `TypeSafeError` variant
/// so that timeouts and connection failures are retryable.
fn map_send_error(e: reqwest::Error) -> TypeSafeError {
    if e.is_timeout() {
        TypeSafeError::Timeout(10) // approximate; the real timeout is in the builder
    } else if e.is_connect() {
        TypeSafeError::Connection(e.to_string())
    } else {
        TypeSafeError::Transport(e)
    }
}

/// Parse a `Retry-After` header value (seconds or HTTP-date).
/// Returns `None` if the value can't be parsed.
#[allow(dead_code)] // used in tests; reserved for future Retry-After support
fn parse_retry_after(value: &str) -> Option<Duration> {
    // Try parsing as seconds (most common).
    if let Ok(secs) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    // HTTP-date format is rarely used and harder to parse without extra deps.
    None
}

/// Truncate a string to `max` characters, appending "..." if truncated.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

/// Validate that the base URL has an https scheme (or is localhost for dev).
fn validate_base_url(url: &str) -> Result<()> {
    if url.starts_with("https://") {
        return Ok(());
    }
    // Allow http:// only for localhost (development/testing).
    if url.starts_with("http://localhost") || url.starts_with("http://127.0.0.1") {
        return Ok(());
    }
    Err(TypeSafeError::Validation(format!(
        "Base URL must use https:// (or http://localhost for development). Got: {url}"
    )))
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
    fn trims_api_key_whitespace() {
        // A stray newline from an env file should be trimmed.
        let result = TypeSafeClient::new("  apikey_test  \n");
        assert!(result.is_ok());
        let client = result.unwrap();
        // The key is stored trimmed internally.
        assert_eq!(client.config.api_key, "apikey_test");
    }

    #[test]
    fn rejects_http_base_url() {
        let config = ClientConfig {
            api_key: "test".to_string(),
            base_url: "http://api.typesafe.ai".to_string(),
            ..ClientConfig::default()
        };
        let result = TypeSafeClient::from_config(config);
        assert!(result.is_err());
    }

    #[test]
    fn allows_localhost_http() {
        let config = ClientConfig {
            api_key: "test".to_string(),
            base_url: "http://localhost:8080".to_string(),
            ..ClientConfig::default()
        };
        let result = TypeSafeClient::from_config(config);
        assert!(result.is_ok());
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

    #[test]
    fn truncate_shortens_long_strings() {
        let long = "x".repeat(600);
        let truncated = truncate(&long, 512);
        assert_eq!(truncated.len(), 515); // 512 + "..."
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn parse_retry_after_seconds() {
        assert_eq!(parse_retry_after("5"), Some(Duration::from_secs(5)));
        assert_eq!(parse_retry_after("  10  "), Some(Duration::from_secs(10)));
        assert_eq!(parse_retry_after("not a number"), None);
    }
}
