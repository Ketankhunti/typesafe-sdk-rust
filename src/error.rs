//! Error types for the TypeSafe SDK.
//!
//! Mirrors the error hierarchy in the Python and JavaScript SDKs:
//! a root [`TypeSafeError`] enum with variants for authentication, rate
//! limits, bad requests, connection failures, timeouts, and response
//! validation.

use thiserror::Error;

/// The root error type returned by all SDK operations.
#[derive(Debug, Error)]
pub enum TypeSafeError {
    /// `401 Unauthorized` — missing or invalid API key.
    #[error("authentication failed: {0}")]
    Authentication(String),

    /// `400 Bad Request` — the server rejected the request as malformed.
    #[error("bad request: {0}")]
    BadRequest(String),

    /// `404 Not Found` — the requested resource does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// `422 Unprocessable Entity` — the request body failed validation.
    #[error("unprocessable entity: {0}")]
    UnprocessableEntity(String),

    /// `429 Too Many Requests` — rate limit exceeded.
    #[error("rate limit exceeded: {0}")]
    RateLimit(String),

    /// `5xx` — server-side error.
    #[error("internal server error: {0}")]
    InternalServer(String),

    /// `529 Overloaded` — the service is temporarily overloaded.
    #[error("service overloaded: {0}")]
    Overloaded(String),

    /// A generic API error for unexpected HTTP status codes.
    #[error("API error (status {status}): {message}")]
    Api {
        /// The HTTP status code.
        status: u16,
        /// The error message from the server.
        message: String,
    },

    /// The request could not connect or timed out after all retries.
    #[error("connection error: {0}")]
    Connection(String),

    /// The request timed out.
    #[error("request timed out after {0}s")]
    Timeout(u64),

    /// The response body did not match the expected schema.
    #[error("response validation error: {0}")]
    ResponseValidation(String),

    /// A local validation error before the request was sent
    /// (e.g. empty questions or a score with fewer than two criteria).
    #[error("{0}")]
    Validation(String),

    /// A JSON serialization or deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// An HTTP transport error from the underlying client.
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),
}

impl TypeSafeError {
    /// Returns `true` if this error is retryable (rate limit, overload,
    /// server error, connection failure, or timeout).
    ///
    /// `Transport` errors (builder, redirect, decode) are deterministic and
    /// are **not** retryable.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            TypeSafeError::RateLimit(_)
                | TypeSafeError::Overloaded(_)
                | TypeSafeError::InternalServer(_)
                | TypeSafeError::Connection(_)
                | TypeSafeError::Timeout(_)
        )
    }
}

/// A convenience `Result` alias used throughout the SDK.
pub type Result<T> = std::result::Result<T, TypeSafeError>;
