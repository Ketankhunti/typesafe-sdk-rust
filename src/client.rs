//! The TypeSafe AI API client.
//!
//! Mirrors the Python `TypeSafeClient` and JavaScript `TypeSafeClient`:
//! a client struct with configurable API key, base URL, default model,
//! timeout, and retry policy. The primary method is [`system_one`],
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

/// Configuration for constructing a [`TypeSafeClient`].
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// API key. Falls back to `TYPESAFE_API_KEY` env var.
    pub api_key: String,
    /// API root. Defaults to `https://api.typesafe.ai`.
    pub base_url: String,
    /// Default model. Defaults to `jev-latest`.
    pub default_model: String,
    /// Per-attempt timeout. Defaults to 60 seconds.
    pub timeout: Duration,
    /// Retry policy. Defaults to 2 retries with exponential backoff.
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

/// Client for the TypeSafe AI API.
///
/// Construct with [`TypeSafeClient::new`] or [`TypeSafeClient::from_env`].
pub struct TypeSafeClient {
    config: ClientConfig,
    http: HttpClient,
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
    pub fn from_config(config: ClientConfig) -> Result<Self> {
        if config.api_key.is_empty() {
            return Err(TypeSafeError::Validation(
                "No API key was provided. Pass an API key to `TypeSafeClient::new` or set the `TYPESAFE_API_KEY` environment variable."
                    .to_string(),
            ));
        }

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

        self.post_with_retry(SYSTEM_ONE_PATH, body).await
    }

    // -----------------------------------------------------------------------
    // models
    // -----------------------------------------------------------------------

    /// List available models. Sends `GET /v1/models`.
    pub async fn list_models(&self) -> Result<ListModelsResponse> {
        self.get_with_retry(MODELS_PATH).await
    }

    // -----------------------------------------------------------------------
    // Internal HTTP + retry
    // -----------------------------------------------------------------------

    async fn post_with_retry<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: Value,
    ) -> Result<T> {
        let url = format!("{}{}", self.config.base_url, path);
        let max_retries = self.config.retry.max_retries;

        let mut attempt = 0;
        loop {
            let result = self.attempt_post(&url, &body).await;
            match result {
                Ok(resp) => return Ok(resp),
                Err(e) if e.is_retryable() && attempt < max_retries => {
                    let delay = self.config.retry.delay_for(attempt);
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn attempt_post<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        body: &Value,
    ) -> Result<T> {
        let response = self
            .http
            .post(url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("User-Agent", format!("typesafe-sdk/{}", SDK_VERSION))
            .header("X-TypeSafe-SDK", format!("typesafe-sdk/{}", SDK_VERSION))
            .json(body)
            .send()
            .await?;

        Self::parse_response(response).await
    }

    async fn get_with_retry<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}{}", self.config.base_url, path);
        let max_retries = self.config.retry.max_retries;

        let mut attempt = 0;
        loop {
            let result = self.attempt_get(&url).await;
            match result {
                Ok(resp) => return Ok(resp),
                Err(e) if e.is_retryable() && attempt < max_retries => {
                    let delay = self.config.retry.delay_for(attempt);
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn attempt_get<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        let response = self
            .http
            .get(url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Accept", "application/json")
            .header("User-Agent", format!("typesafe-sdk/{}", SDK_VERSION))
            .header("X-TypeSafe-SDK", format!("typesafe-sdk/{}", SDK_VERSION))
            .send()
            .await?;

        Self::parse_response(response).await
    }

    async fn parse_response<T: serde::de::DeserializeOwned>(
        response: reqwest::Response,
    ) -> Result<T> {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();

        if status.is_success() {
            return serde_json::from_str(&text).map_err(|e| {
                TypeSafeError::ResponseValidation(format!(
                    "Failed to parse response body: {e}\nBody: {text}"
                ))
            });
        }

        // Map HTTP status codes to typed errors, mirroring the Python/JS SDKs.
        let message = Self::extract_error_message(&text).unwrap_or(text);
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
}
