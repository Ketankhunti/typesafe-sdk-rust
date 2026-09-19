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
//! use typesafeai_sdk::{TypeSafeClient, noul, choice, score};
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
use reqwest::{redirect, Client as HttpClient, Method, StatusCode};
use serde_json::Value;

use crate::error::{ErrorKind, Result, TypeSafeError};
use crate::questions::{validate_questions, Question};
use crate::retry::{jitter_seed, RetryPolicy};
use crate::types::{ListModelsResponse, SetMetadata, SystemOneRequest, SystemOneResponse};

/// Maximum response body size to read into memory (1 MiB). Protects against
/// a malicious or buggy server returning an enormous body that would exhaust
/// memory.
const MAX_RESPONSE_BODY_BYTES: usize = 1024 * 1024;

/// Header names that callers may not set via `with_extra_header`. These are
/// either set by the SDK itself (auth, accept, SDK ID) or are hop-by-hop /
/// content headers that must not be tampered with.
const PROTECTED_HEADER_NAMES: &[&str] = &[
    "authorization",
    "accept",
    "x-typesafe-sdk",
    "content-type",
    "content-length",
    "transfer-encoding",
    "host",
    "connection",
    "cookie",
    "proxy-authorization",
    "proxy-authenticate",
    "te",
    "trailer",
    "upgrade",
];

const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
const DEFAULT_MODEL: &str = "jev-latest";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const SDK_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
const SYSTEM_ONE_PATH: &str = "/v1/systemone";
const MODELS_PATH: &str = "/v1/models";
const RETRY_AFTER_MS_HEADER: &str = "retry-after-ms";
const REQUEST_ID_HEADER: &str = "x-typesafe-request-id";
/// Maximum number of characters of a response body echoed into an error.
const MAX_ERROR_BODY_CHARS: usize = 512;

/// Configuration for constructing a [`TypeSafeClient`].
///
/// All fields are public for construction, but prefer [`ClientConfig::default`]
/// or the builder methods on [`RetryPolicy`] for common cases.
#[derive(Clone)]
#[non_exhaustive]
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
    /// Custom `reqwest::Client` to use for HTTP requests. If `None`, a new
    /// client is built from the other config fields. This allows sharing a
    /// connection pool or using a custom TLS configuration.
    pub http_client: Option<HttpClient>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
            default_model: DEFAULT_MODEL.to_string(),
            timeout: DEFAULT_TIMEOUT,
            retry: RetryPolicy::default(),
            http_client: None,
        }
    }
}

impl ClientConfig {
    /// Create a new `ClientConfig` with the given API key and all other
    /// fields set to their defaults.
    ///
    /// Use the `.with_*()` methods to override individual fields.
    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            ..Self::default()
        }
    }

    /// Set the API key.
    #[must_use]
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = api_key.into();
        self
    }

    /// Set the base URL.
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Set the default model.
    #[must_use]
    pub fn with_default_model(mut self, default_model: impl Into<String>) -> Self {
        self.default_model = default_model.into();
        self
    }

    /// Set the per-attempt timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the retry policy.
    #[must_use]
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Provide a custom `reqwest::Client` for HTTP requests. If not set, the
    /// client builds one from the other config fields (timeout, default
    /// headers). Use this to share a connection pool or apply custom TLS
    /// settings.
    ///
    /// The client is used as-is, so the caller controls redirect behavior.
    /// The SDK's default client disables redirects to prevent POST replay
    /// via 307/308; a custom client that follows redirects does not have
    /// this protection.
    #[must_use]
    pub fn with_http_client(mut self, client: HttpClient) -> Self {
        self.http_client = Some(client);
        self
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
            .field("http_client", &self.http_client.is_some())
            .finish()
    }
}

/// Client for the TypeSafe AI API.
///
/// Construct with [`TypeSafeClient::new`] or [`TypeSafeClient::from_env`].
/// Cloning is cheap and shares the underlying connection pool.
#[derive(Clone)]
#[non_exhaustive]
pub struct TypeSafeClient {
    config: ClientConfig,
    http: HttpClient,
    /// The pre-built `Authorization: Bearer <key>` header value, stored so it
    /// can be attached per-request even when the caller supplies a custom
    /// `reqwest::Client` (whose default headers we cannot modify).
    auth_header: HeaderValue,
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

/// Internal per-call options passed through the retry loop.
/// `None` fields mean "use the client default".
struct CallOpts {
    extra_headers: Vec<(String, String)>,
    timeout: Option<Duration>,
    retry: Option<RetryPolicy>,
}

impl CallOpts {
    /// Create an empty options set (use all client defaults).
    fn empty() -> Self {
        Self {
            extra_headers: Vec::new(),
            timeout: None,
            retry: None,
        }
    }
}

/// Per-call options for [`TypeSafeClient::system_one_with_opts`].
///
/// Built with a builder pattern. Start with [`SystemOneOpts::new`] and chain
/// `.with_*()` methods. Fields not set use the client's defaults.
///
/// # Example
/// ```no_run
/// use typesafeai_sdk::SystemOneOpts;
/// use std::time::Duration;
///
/// let opts = SystemOneOpts::new()
///     .with_model(Some("jev-latest".to_string()))
///     .with_state("I was charged twice.")
///     .with_extra_body(serde_json::json!({"priority": "high"}))
///     .with_timeout(Duration::from_secs(30))
///     .with_extra_header("X-Trace-Id", "abc123");
/// ```
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct SystemOneOpts {
    /// Override the client's default model for this call.
    pub(crate) model: Option<String>,
    /// The state to evaluate. If not set, defaults to `null`.
    pub(crate) state: Option<Value>,
    /// Extra fields to merge into the request body for forward compatibility.
    /// Reserved keys (`state`, `model`, `questions`) are rejected; use the
    /// dedicated methods instead. Object values are replaced, not deep-merged.
    pub(crate) extra_body: Option<Value>,
    /// Additional headers to send with this call only.
    pub(crate) extra_headers: Vec<(String, String)>,
    /// Per-call timeout override. `None` uses the client default.
    pub(crate) timeout: Option<Duration>,
    /// Per-call retry policy override. `None` uses the client default.
    pub(crate) retry: Option<RetryPolicy>,
}

impl fmt::Debug for SystemOneOpts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemOneOpts")
            .field("model", &self.model)
            // Redact state — it may contain sensitive user data.
            .field("state", &self.state.as_ref().map(|_| "<redacted>"))
            // Redact extra_body — it may contain sensitive user data.
            .field(
                "extra_body",
                &self.extra_body.as_ref().map(|_| "<redacted>"),
            )
            // Redact header values — they may contain tokens.
            .field(
                "extra_headers",
                &self
                    .extra_headers
                    .iter()
                    .map(|(k, _)| (k.as_str(), "<redacted>"))
                    .collect::<Vec<_>>(),
            )
            .field("timeout", &self.timeout)
            .field("retry", &self.retry)
            .finish()
    }
}

impl SystemOneOpts {
    /// Create a new empty options set (all fields use client defaults).
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the model for this call. `None` uses the client default.
    #[must_use = "the returned SystemOneOpts should be used"]
    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }

    /// Set the state to evaluate. Accepts a string, JSON object, or array.
    #[must_use = "the returned SystemOneOpts should be used"]
    pub fn with_state(mut self, state: impl Into<Value>) -> Self {
        self.state = Some(state.into());
        self
    }

    /// Set extra fields to merge into the request body for forward
    /// compatibility. Reserved keys (`state`, `model`, `questions`) are
    /// rejected with a validation error — use the dedicated methods
    /// (`with_state`, `with_model`, or the `questions` argument) instead.
    /// Object values are replaced rather than deep-merged.
    #[must_use = "the returned SystemOneOpts should be used"]
    pub fn with_extra_body(mut self, extra: Value) -> Self {
        self.extra_body = Some(extra);
        self
    }

    /// Add a custom header to send with this call only. Headers set here
    /// are applied on top of the client's default headers. Authentication,
    /// `Accept`, and SDK identification headers remain protected and cannot
    /// be overridden.
    ///
    /// Can be called multiple times to add multiple headers.
    #[must_use = "the returned SystemOneOpts should be used"]
    pub fn with_extra_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_headers.push((name.into(), value.into()));
        self
    }

    /// Override the timeout for this call only. `None` uses the client default.
    #[must_use = "the returned SystemOneOpts should be used"]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Override the retry policy for this call only. `None` uses the client
    /// default.
    #[must_use = "the returned SystemOneOpts should be used"]
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = Some(retry);
        self
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
            return Err(TypeSafeError::new(ErrorKind::Validation(
                "No API key was provided. Pass an API key to `TypeSafeClient::new` or set the `TYPESAFE_API_KEY` environment variable."
                    .to_string(),
            )));
        }

        // Normalize the base URL: trim trailing slash and validate scheme/host.
        config.base_url = config.base_url.trim().trim_end_matches('/').to_string();
        validate_base_url(&config.base_url)?;

        // Validate the default model name.
        if config.default_model.trim().is_empty() {
            return Err(TypeSafeError::new(ErrorKind::Validation(
                "Default model name cannot be empty.".to_string(),
            )));
        }

        // Build the auth header once. Marking it sensitive keeps the key out
        // of reqwest's debug output.
        let mut auth =
            HeaderValue::from_str(&format!("Bearer {}", config.api_key)).map_err(|_| {
                TypeSafeError::new(ErrorKind::Validation(
                    "The API key contains characters that are not valid in an HTTP header."
                        .to_string(),
                ))
            })?;
        auth.set_sensitive(true);

        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, auth.clone());
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            HeaderName::from_static("x-typesafe-sdk"),
            HeaderValue::from_static(SDK_USER_AGENT),
        );

        // Use the caller-provided HTTP client if given; otherwise build one
        // from the config. A provided client is used as-is so the caller
        // controls TLS, proxies, connection pools, etc. Auth and SDK headers
        // are attached per-request in `attempt_request` so they work with both
        // paths.
        let http = match config.http_client.take() {
            Some(client) => client,
            None => HttpClient::builder()
                .timeout(config.timeout)
                .user_agent(SDK_USER_AGENT)
                .default_headers(headers)
                // Never follow redirects: a 307/308 would replay the POST body
                // (and the Authorization header) to a potentially different host.
                .redirect(redirect::Policy::none())
                .build()
                .map_err(|e| TypeSafeError::new(ErrorKind::Transport(e.to_string())))?,
        };

        Ok(Self {
            config,
            http,
            auth_header: auth,
        })
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
    /// failures (429, 500, 502, 503, 504, 529, connection errors, timeouts),
    /// but a timeout does
    /// not guarantee the server did not process the request. If the endpoint
    /// has side effects (credit consumption, usage recording, downstream
    /// triggers), a retry may result in duplicate processing. Future versions
    /// will support an `Idempotency-Key` header to make retries safe.
    ///
    /// To avoid replaying a POST whose body the server already processed,
    /// errors that occur while *reading the response body* — including
    /// timeouts and truncated bodies — are classified as non-retryable
    /// [`ErrorKind::Transport`] rather than [`ErrorKind::Connection`]
    /// or [`ErrorKind::Timeout`].
    ///
    /// # Errors
    /// - [`ErrorKind::Validation`] if questions are empty or a choice or
    ///   score question has fewer than two criteria.
    /// - [`ErrorKind::Authentication`] if the API key is invalid.
    /// - [`ErrorKind::RateLimit`] if rate-limited (after retries).
    /// - [`ErrorKind::Connection`] / [`ErrorKind::Timeout`] on
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
        let opts = SystemOneOpts::new()
            .with_model(model.map(|m| m.to_string()))
            .with_state(state);
        self.system_one_with_opts(questions, opts).await
    }

    /// Like [`system_one`](Self::system_one) but with full per-call options
    /// via a [`SystemOneOpts`] builder.
    ///
    /// This allows overriding the model, adding extra body fields for
    /// forward compatibility, and attaching custom headers — all per-call.
    ///
    /// # Example
    /// ```no_run
    /// use typesafeai_sdk::{TypeSafeClient, noul, SystemOneOpts};
    /// use std::collections::HashMap;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = TypeSafeClient::from_env()?;
    /// let mut questions = HashMap::new();
    /// questions.insert("billing".to_string(), noul("Is this about billing?").into());
    ///
    /// let opts = SystemOneOpts::new()
    ///     .with_model(Some("jev-latest".to_string()))
    ///     .with_extra_body(serde_json::json!({"user_id": "u123"}));
    ///
    /// let response = client.system_one_with_opts(questions, opts).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn system_one_with_opts(
        &self,
        questions: HashMap<String, Question>,
        opts: SystemOneOpts,
    ) -> Result<SystemOneResponse> {
        validate_questions(&questions)?;

        let model_name = opts
            .model
            .unwrap_or_else(|| self.config.default_model.clone());
        if model_name.trim().is_empty() {
            return Err(TypeSafeError::new(ErrorKind::Validation(
                "Model name cannot be empty.".to_string(),
            )));
        }

        let request = SystemOneRequest {
            state: opts.state.unwrap_or(Value::Null),
            model: model_name,
            questions,
        };

        let mut body = serde_json::to_value(&request)?;

        // Merge extra_body fields at the top level for forward compatibility.
        // Reserved keys (state, model, questions) are rejected outright so
        // that extra_body cannot override or bypass validation of the known
        // fields.
        if let Some(extra) = &opts.extra_body {
            let extra_obj = extra.as_object().ok_or_else(|| {
                TypeSafeError::new(ErrorKind::Validation(
                    "extra_body must be a JSON object.".to_string(),
                ))
            })?;
            const RESERVED_KEYS: &[&str] = &["state", "model", "questions"];
            for key in extra_obj.keys() {
                if RESERVED_KEYS.contains(&key.as_str()) {
                    return Err(TypeSafeError::new(ErrorKind::Validation(format!(
                        "extra_body cannot override reserved key `{key}`. \
                         Use the dedicated method (with_state, with_model, or the \
                         questions argument) instead."
                    ))));
                }
            }
            let body_obj = body.as_object_mut().ok_or_else(|| {
                TypeSafeError::new(ErrorKind::Validation(
                    "Internal error: request body is not a JSON object.".to_string(),
                ))
            })?;
            for (k, v) in extra_obj {
                body_obj.insert(k.clone(), v.clone());
            }
        }

        let call_opts = CallOpts {
            extra_headers: opts.extra_headers,
            timeout: opts.timeout,
            retry: opts.retry,
        };

        self.request_with_retry_opts(Method::POST, SYSTEM_ONE_PATH, Some(body), &call_opts)
            .await
    }

    // -----------------------------------------------------------------------
    // models
    // -----------------------------------------------------------------------

    /// List available models. Sends `GET /v1/models`.
    pub async fn list_models(&self) -> Result<ListModelsResponse> {
        self.request_with_retry_opts(Method::GET, MODELS_PATH, None, &CallOpts::empty())
            .await
    }

    // -----------------------------------------------------------------------
    // Internal HTTP + retry
    // -----------------------------------------------------------------------

    async fn request_with_retry_opts<T: serde::de::DeserializeOwned + SetMetadata>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        opts: &CallOpts,
    ) -> Result<T> {
        let url = format!("{}{}", self.config.base_url, path);
        let retry = opts.retry.as_ref().unwrap_or(&self.config.retry);
        let timeout = opts.timeout.unwrap_or(self.config.timeout);

        let started = std::time::Instant::now();
        let mut attempt = 0;
        loop {
            // Cap this attempt's timeout at the remaining budget so a single
            // attempt cannot push the total elapsed time past the budget.
            let attempt_timeout = match retry.budget {
                Some(budget) => {
                    let remaining = budget.saturating_sub(started.elapsed());
                    if remaining.is_zero() {
                        // Nothing left in the budget — don't even start.
                        // (Only reached on retry iterations; the first attempt
                        // always has the full budget.)
                        return Err(failure_on_budget_exhausted(retry));
                    }
                    std::cmp::min(timeout, remaining)
                }
                None => timeout,
            };

            match self
                .attempt_request(&method, &url, body.as_ref(), opts, attempt_timeout)
                .await
            {
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

                    // Stop before a delay that would exceed the budget.
                    if retry.budget_exceeded(started.elapsed(), delay) {
                        return Err(failure.error);
                    }

                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
            }
        }
    }

    async fn attempt_request<T: serde::de::DeserializeOwned + SetMetadata>(
        &self,
        method: &Method,
        url: &str,
        body: Option<&Value>,
        opts: &CallOpts,
        timeout: Duration,
    ) -> std::result::Result<T, Failure> {
        // Attach auth, Accept, and SDK headers per-request so they are present
        // even when the caller supplies a custom `reqwest::Client` whose
        // default headers we cannot modify.
        let mut request = self
            .http
            .request(method.clone(), url)
            .timeout(timeout)
            .header(AUTHORIZATION, &self.auth_header)
            .header(ACCEPT, "application/json")
            .header("x-typesafe-sdk", SDK_USER_AGENT);

        if let Some(b) = body {
            request = request.json(b);
        }

        // Apply per-call extra headers. Protected headers are rejected with an
        // error to prevent accidental credential leakage, SDK identification
        // removal, or content/header corruption.
        for (name, value) in &opts.extra_headers {
            let lower = name.to_ascii_lowercase();
            if PROTECTED_HEADER_NAMES.contains(&lower.as_str()) {
                return Err(Failure::from(TypeSafeError::new(ErrorKind::Validation(
                    format!(
                        "Header `{name}` is protected and cannot be set via with_extra_header."
                    ),
                ))));
            }
            let hn = HeaderName::try_from(name.as_str()).map_err(|_| {
                Failure::from(TypeSafeError::new(ErrorKind::Validation(format!(
                    "Invalid header name: {name}"
                ))))
            })?;
            let hv = HeaderValue::try_from(value.as_str()).map_err(|_| {
                Failure::from(TypeSafeError::new(ErrorKind::Validation(format!(
                    "Invalid header value for `{name}`: contains invalid characters."
                ))))
            })?;
            request = request.header(hn, hv);
        }

        let response = request
            .send()
            .await
            .map_err(|e| map_reqwest_error(e, timeout))?;

        self.parse_response(response).await
    }

    async fn parse_response<T: serde::de::DeserializeOwned + SetMetadata>(
        &self,
        response: reqwest::Response,
    ) -> std::result::Result<T, Failure> {
        let status = response.status();
        let headers = response.headers();
        // Capture the request ID for both success and error paths.
        let request_id = headers
            .get(REQUEST_ID_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        // Prefer retry-after-ms (milliseconds, more precise) over the
        // standard Retry-After (seconds). Either may be absent.
        let retry_after = headers
            .get(RETRY_AFTER_MS_HEADER)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_retry_after_ms)
            .or_else(|| {
                headers
                    .get(RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(parse_retry_after)
            });

        // Read the body with a size cap to protect against unbounded memory
        // consumption from a malicious or buggy server.
        let bytes = read_body_capped(response, MAX_RESPONSE_BODY_BYTES).await?;
        let text = String::from_utf8_lossy(&bytes);

        if status.is_success() {
            // Parse once into a `Value`, then deserialize `T` from it so we
            // can reuse the same `Value` for the `raw` field without a second
            // parse.
            let raw: Value = serde_json::from_str(&text).map_err(|e| {
                Failure::from(TypeSafeError::new(ErrorKind::ResponseValidation {
                    message: format!(
                        "Failed to parse response body: {e}\nBody: {}",
                        truncate(&text, MAX_ERROR_BODY_CHARS)
                    ),
                    field_path: None,
                }))
            })?;
            let mut result: T = T::deserialize(&raw).map_err(|e| {
                Failure::from(TypeSafeError::new(ErrorKind::ResponseValidation {
                    message: format!(
                        "Failed to parse response body: {e}\nBody: {}",
                        truncate(&text, MAX_ERROR_BODY_CHARS)
                    ),
                    field_path: None,
                }))
            })?;
            // Inject the request ID (only if the header was present) and raw
            // body into the response type via the trait.
            result.set_metadata(request_id, Some(raw));
            return Ok(result);
        }

        // Map HTTP status codes to typed errors, mirroring the Python/JS SDKs.
        // Truncate the extracted message to prevent unbounded error strings.
        let message = Self::extract_error_message(&text)
            .map(|m| truncate(&m, MAX_ERROR_BODY_CHARS))
            .unwrap_or_else(|| truncate(&text, MAX_ERROR_BODY_CHARS));
        let kind = match status {
            StatusCode::UNAUTHORIZED => ErrorKind::Authentication(message),
            StatusCode::BAD_REQUEST => ErrorKind::BadRequest(message),
            StatusCode::NOT_FOUND => ErrorKind::NotFound(message),
            StatusCode::UNPROCESSABLE_ENTITY => ErrorKind::UnprocessableEntity(message),
            StatusCode::TOO_MANY_REQUESTS => ErrorKind::RateLimit(message),
            StatusCode::INTERNAL_SERVER_ERROR
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT => ErrorKind::InternalServer(message),
            s if s.as_u16() == 529 => ErrorKind::Overloaded(message),
            _ => ErrorKind::Api {
                status: status.as_u16(),
                message,
            },
        };
        Err(Failure {
            error: TypeSafeError::with_request_id(kind, request_id),
            retry_after,
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

/// Read the response body into a `Vec<u8>` with a maximum size cap.
/// Returns `Err` if the body exceeds `max_bytes`, using a transport error.
async fn read_body_capped(
    response: reqwest::Response,
    max_bytes: usize,
) -> std::result::Result<Vec<u8>, TypeSafeError> {
    let mut bytes = Vec::new();
    let mut resp = response;
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                if bytes.len() + chunk.len() > max_bytes {
                    return Err(TypeSafeError::new(ErrorKind::Transport(format!(
                        "response body exceeds maximum size of {max_bytes} bytes"
                    ))));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(e) => return Err(TypeSafeError::new(ErrorKind::Transport(e.to_string()))),
        }
    }
    Ok(bytes)
}

/// Build a `Timeout` error when the retry budget is exhausted before an
/// attempt can even start. Uses the configured per-attempt timeout in the
/// message so the caller sees what *would* have been used.
fn failure_on_budget_exhausted(retry: &RetryPolicy) -> TypeSafeError {
    TypeSafeError::new(ErrorKind::Timeout(retry.budget.unwrap_or(Duration::ZERO)))
}

/// Map a `reqwest` error to the appropriate `TypeSafeError` variant.
///
/// - timeouts -> [`ErrorKind::Timeout`], reporting the *configured* timeout
/// - connect failures (refused, reset, DNS) -> [`ErrorKind::Connection`]
/// - everything else (body read, builder, redirect, decode, request
///   construction) is deterministic or occurs after the server has already
///   processed the request, so it stays a non-retryable
///   [`ErrorKind::Transport`]
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
        TypeSafeError::new(ErrorKind::Timeout(timeout))
    } else if e.is_connect() {
        TypeSafeError::new(ErrorKind::Connection(e.to_string()))
    } else {
        TypeSafeError::new(ErrorKind::Transport(e.to_string()))
    }
}

/// Parse a `Retry-After` header value given in seconds.
///
/// The HTTP-date form is not supported (it needs a date parser); such values
/// return `None`, and the SDK falls back to its own backoff.
fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Parse a `retry-after-ms` header value given in milliseconds.
///
/// This is a non-standard header used by some API gateways to express
/// sub-second retry delays with more precision than the standard
/// `Retry-After` (seconds) header.
fn parse_retry_after_ms(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_millis)
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
    let url = reqwest::Url::parse(raw).map_err(|e| {
        TypeSafeError::new(ErrorKind::Validation(format!(
            "Base URL is not a valid URL: {e}"
        )))
    })?;

    // Reject credentials, query parameters, and fragments — a base URL
    // should be a bare origin + optional path.
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(TypeSafeError::new(ErrorKind::Validation(
            "Base URL must not contain credentials, query parameters, or fragments.".to_string(),
        )));
    }

    match (url.scheme(), url.host_str()) {
        ("https", Some(_)) => Ok(()),
        ("http", Some("localhost" | "127.0.0.1" | "::1" | "[::1]")) => Ok(()),
        (scheme, _) => Err(TypeSafeError::new(ErrorKind::Validation(format!(
            "Base URL must use https:// (or http:// for localhost/127.0.0.1/::1 during development); got scheme `{scheme}` and an unsupported host."
        )))),
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
        assert!(
            matches!(err.kind(), ErrorKind::Validation(_)),
            "got {err:?}"
        );
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

    /// A base URL with a trailing slash must not produce a double slash
    /// when joined with a path like `/v1/systemone`.
    #[test]
    fn trailing_slash_does_not_produce_double_slash() {
        let config = ClientConfig {
            api_key: "test".to_string(),
            base_url: "https://api.typesafe.ai/v1/".to_string(),
            ..ClientConfig::default()
        };
        let client = TypeSafeClient::from_config(config).unwrap();
        // Trailing slash is stripped, so joining with "/v1/systemone"
        // produces a single slash, not "//".
        let url = format!("{}{}", client.base_url(), "/v1/systemone");
        assert_eq!(url, "https://api.typesafe.ai/v1/v1/systemone");
        assert!(!url.contains("//v1"), "double slash in URL: {url}");
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

    #[test]
    fn parse_retry_after_ms_parses_milliseconds() {
        assert_eq!(
            parse_retry_after_ms("500"),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            parse_retry_after_ms("  1500  "),
            Some(Duration::from_millis(1500))
        );
        assert_eq!(parse_retry_after_ms("not a number"), None);
    }

    /// Builder errors are deterministic, so they must map to the
    /// non-retryable `Transport` variant, not `Connection`.
    #[test]
    fn builder_errors_are_not_retryable() {
        let err = reqwest::Client::new().get("not a url").build().unwrap_err();
        let mapped = map_reqwest_error(err, Duration::from_secs(1));
        assert!(
            matches!(mapped.kind(), ErrorKind::Transport(_)),
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
            matches!(err.kind(), ErrorKind::Validation(ref m) if m.contains("model")),
            "got {err:?}"
        );
    }

    #[test]
    fn timeout_error_preserves_subsecond_duration() {
        let err = TypeSafeError::new(ErrorKind::Timeout(Duration::from_millis(150)));
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
