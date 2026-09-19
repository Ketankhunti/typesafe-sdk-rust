//! Error types for the TypeSafe SDK.
//!
//! Mirrors the error hierarchy in the Python and JavaScript SDKs:
//! a root [`TypeSafeError`] struct wrapping an [`ErrorKind`] enum, with an
//! optional request ID for correlating with server-side logs.

use std::time::Duration;

use thiserror::Error;

/// The specific kind of error that occurred.
///
/// This is the inner classification extracted from [`TypeSafeError`]. Match
/// on `err.kind()` to handle specific error categories.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ErrorKind {
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

    /// `500`, `502`, `503`, or `504` — retryable server-side error.
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
    #[error("request timed out (configured timeout: {0:?})")]
    Timeout(Duration),

    /// The response body did not match the expected schema.
    #[error("response validation error: {message}{}", field_path.as_ref().map(|p| format!(" (field: {p})")).unwrap_or_default())]
    ResponseValidation {
        /// The human-readable error message.
        message: String,
        /// The JSON path to the field that failed validation (e.g.
        /// `answers.billing.noul`), if known.
        field_path: Option<String>,
    },

    /// A local validation error before the request was sent
    /// (e.g. empty questions or a score with fewer than two criteria).
    #[error("{0}")]
    Validation(String),

    /// A JSON serialization or deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// An HTTP transport error from the underlying client (e.g. a body-read
    /// failure, redirect loop, or request-builder error). The underlying
    /// `reqwest::Error` is converted to a string so it does not leak into
    /// the public API.
    #[error("transport error: {0}")]
    Transport(String),

    /// A Tokio runtime error from the blocking client (e.g. attempting to
    /// create or use a blocking client from inside an async context).
    #[error("runtime error: {0}")]
    Runtime(String),
}

impl ErrorKind {
    /// Returns `true` if this error is retryable (rate limit, overload,
    /// server error, connection failure, or timeout).
    ///
    /// `Transport` errors (body read, builder, redirect, decode) are
    /// deterministic or occur after the server has already processed the
    /// request, and are **not** retryable.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ErrorKind::RateLimit(_)
                | ErrorKind::Overloaded(_)
                | ErrorKind::InternalServer(_)
                | ErrorKind::Connection(_)
                | ErrorKind::Timeout(_)
        )
    }
}

/// The root error type returned by all SDK operations.
///
/// Wraps an [`ErrorKind`] with an optional request ID (from the
/// `x-typesafe-request-id` response header) for correlating errors with
/// server-side logs.
#[derive(Debug)]
#[non_exhaustive]
pub struct TypeSafeError {
    kind: ErrorKind,
    request_id: Option<String>,
}

impl TypeSafeError {
    /// Create a new error from a kind, with no request ID.
    pub fn new(kind: ErrorKind) -> Self {
        Self {
            kind,
            request_id: None,
        }
    }

    /// Create a new error from a kind and an optional request ID.
    pub fn with_request_id(kind: ErrorKind, request_id: Option<String>) -> Self {
        Self { kind, request_id }
    }

    /// Returns the specific error kind.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// Returns the request ID associated with this error, if the server
    /// provided one via the `x-typesafe-request-id` header.
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    /// Returns `true` if this error is retryable. Delegates to
    /// [`ErrorKind::is_retryable`].
    pub fn is_retryable(&self) -> bool {
        self.kind.is_retryable()
    }
}

impl std::fmt::Display for TypeSafeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.kind)?;
        if let Some(ref id) = self.request_id {
            write!(f, " (request id: {id})")?;
        }
        Ok(())
    }
}

impl std::error::Error for TypeSafeError {}

impl From<serde_json::Error> for TypeSafeError {
    fn from(e: serde_json::Error) -> Self {
        Self::new(ErrorKind::Json(e))
    }
}

impl From<reqwest::Error> for TypeSafeError {
    fn from(e: reqwest::Error) -> Self {
        Self::new(ErrorKind::Transport(e.to_string()))
    }
}

impl From<ErrorKind> for TypeSafeError {
    fn from(kind: ErrorKind) -> Self {
        Self::new(kind)
    }
}

/// A convenience `Result` alias used throughout the SDK.
pub type Result<T> = std::result::Result<T, TypeSafeError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_validation_with_field_path() {
        let err = TypeSafeError::new(ErrorKind::ResponseValidation {
            message: "expected a number".to_string(),
            field_path: Some("answers.billing.noul".to_string()),
        });
        let display = format!("{err}");
        assert!(display.contains("expected a number"));
        assert!(display.contains("answers.billing.noul"));
    }

    #[test]
    fn response_validation_without_field_path() {
        let err = TypeSafeError::new(ErrorKind::ResponseValidation {
            message: "malformed JSON".to_string(),
            field_path: None,
        });
        let display = format!("{err}");
        assert!(display.contains("malformed JSON"));
        assert!(!display.contains("field:"));
    }

    #[test]
    fn response_validation_is_not_retryable() {
        let kind = ErrorKind::ResponseValidation {
            message: "bad".to_string(),
            field_path: None,
        };
        assert!(!kind.is_retryable());
    }

    #[test]
    fn request_id_displayed_when_present() {
        let err = TypeSafeError::with_request_id(
            ErrorKind::Authentication("bad key".to_string()),
            Some("req-123".to_string()),
        );
        let display = format!("{err}");
        assert!(display.contains("bad key"));
        assert!(display.contains("req-123"));
    }

    #[test]
    fn request_id_absent_when_not_provided() {
        let err = TypeSafeError::new(ErrorKind::Authentication("bad key".to_string()));
        let display = format!("{err}");
        assert!(display.contains("bad key"));
        assert!(!display.contains("request id"));
    }

    #[test]
    fn from_error_kind_creates_error() {
        let kind = ErrorKind::Validation("test".to_string());
        let err: TypeSafeError = kind.into();
        assert!(matches!(err.kind(), ErrorKind::Validation(_)));
    }
}
