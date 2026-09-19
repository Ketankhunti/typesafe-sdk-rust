//! Blocking (synchronous) client for the TypeSafe AI API.
//!
//! Enabled with the `blocking` feature flag. Wraps the async
//! [`TypeSafeClient`](crate::TypeSafeClient) with a current-thread Tokio
//! runtime so you can call the API without `async`/`await`.
//!
//! ## Quickstart
//!
//! ```no_run
//! use typesafeai_sdk::blocking::BlockingClient;
//! use typesafeai_sdk::noul;
//! use std::collections::HashMap;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = BlockingClient::from_env()?;
//!
//!     let mut questions = HashMap::new();
//!     questions.insert("billing".to_string(), noul("Is this about billing?").into());
//!
//!     let response = client.system_one("I was charged twice.", questions)?;
//!
//!     if let Some(answer) = response.noul("billing") {
//!         println!("Billing probability: {}", answer.noul);
//!     }
//!     Ok(())
//! }
//! ```

use std::collections::HashMap;
use std::fmt;

use serde_json::Value;

use crate::client::{ClientConfig, SystemOneOpts, TypeSafeClient};
use crate::error::{ErrorKind, Result, TypeSafeError};
use crate::questions::Question;
use crate::types::{ListModelsResponse, SystemOneResponse};

/// A blocking (synchronous) client for the TypeSafe AI API.
///
/// Wraps an async [`TypeSafeClient`] with a dedicated current-thread Tokio
/// runtime. Each method call blocks the calling thread until the response
/// is received.
///
/// Construct with [`BlockingClient::new`], [`BlockingClient::from_env`], or
/// [`BlockingClient::from_config`].
///
/// # Panics
///
/// This client never panics. If the internal Tokio runtime cannot be created
/// (extremely unlikely, typically system resource exhaustion), an
/// [`ErrorKind::Runtime`] error is returned instead. If you attempt to
/// construct a `BlockingClient` from inside an async context, an
/// [`ErrorKind::Runtime`] error is returned rather than panicking.
#[non_exhaustive]
pub struct BlockingClient {
    inner: TypeSafeClient,
    runtime: tokio::runtime::Runtime,
}

impl fmt::Debug for BlockingClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlockingClient")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl BlockingClient {
    /// Create a new blocking client with the given API key and default settings.
    ///
    /// # Errors
    /// - [`TypeSafeError::Validation`] if the API key is empty.
    #[must_use = "the returned client should be used to make API calls"]
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        Self::from_config(ClientConfig {
            api_key: api_key.into(),
            ..ClientConfig::default()
        })
    }

    /// Create a new blocking client from a full [`ClientConfig`].
    ///
    /// # Errors
    /// - [`TypeSafeError::Validation`] if the API key is empty, the base URL
    ///   is invalid, or the default model name is empty.
    /// - [`TypeSafeError::Runtime`] if called from inside an async context or
    ///   if the Tokio runtime cannot be created.
    #[must_use = "the returned client should be used to make API calls"]
    pub fn from_config(config: ClientConfig) -> Result<Self> {
        // Guard: creating a new runtime inside an existing async context
        // would panic. Return a typed error instead.
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(TypeSafeError::new(ErrorKind::Runtime(
                "Cannot create a BlockingClient from inside an async context. \
                 Use TypeSafeClient instead, or construct the BlockingClient \
                 before entering the async runtime."
                    .to_string(),
            )));
        }

        let inner = TypeSafeClient::from_config(config)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| {
                TypeSafeError::new(ErrorKind::Runtime(format!(
                    "Failed to create Tokio runtime for blocking client: {e}"
                )))
            })?;
        Ok(Self { inner, runtime })
    }

    /// Create a new blocking client, reading the API key from the
    /// `TYPESAFE_API_KEY` environment variable. Also reads
    /// `TYPESAFE_BASE_URL` and `TYPESAFE_DEFAULT_MODEL` if set.
    ///
    /// # Errors
    /// - [`TypeSafeError::Validation`] if `TYPESAFE_API_KEY` is unset or empty.
    /// - [`TypeSafeError::Runtime`] if called from inside an async context or
    ///   if the Tokio runtime cannot be created.
    #[must_use = "the returned client should be used to make API calls"]
    pub fn from_env() -> Result<Self> {
        // Guard: creating a new runtime inside an existing async context
        // would panic. Return a typed error instead.
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(TypeSafeError::new(ErrorKind::Runtime(
                "Cannot create a BlockingClient from inside an async context. \
                 Use TypeSafeClient instead, or construct the BlockingClient \
                 before entering the async runtime."
                    .to_string(),
            )));
        }

        let inner = TypeSafeClient::from_env()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| {
                TypeSafeError::new(ErrorKind::Runtime(format!(
                    "Failed to create Tokio runtime for blocking client: {e}"
                )))
            })?;
        Ok(Self { inner, runtime })
    }

    /// Returns the base URL this client is configured to use.
    #[must_use]
    pub fn base_url(&self) -> &str {
        self.inner.base_url()
    }

    /// Returns the default model this client uses.
    #[must_use]
    pub fn default_model(&self) -> &str {
        self.inner.default_model()
    }

    /// Answer named questions about text or structured state.
    ///
    /// Blocks the calling thread until the response is received.
    ///
    /// # Errors
    /// See [`TypeSafeClient::system_one`] for the full list of error variants.
    pub fn system_one(
        &self,
        state: impl Into<Value>,
        questions: HashMap<String, Question>,
    ) -> Result<SystemOneResponse> {
        self.runtime
            .block_on(self.inner.system_one(state, questions))
    }

    /// Like [`system_one`](Self::system_one) but with an explicit model
    /// override (`None` uses the client default).
    ///
    /// # Errors
    /// See [`TypeSafeClient::system_one_with_model`] for the full list of
    /// error variants.
    pub fn system_one_with_model(
        &self,
        state: impl Into<Value>,
        questions: HashMap<String, Question>,
        model: Option<&str>,
    ) -> Result<SystemOneResponse> {
        self.runtime
            .block_on(self.inner.system_one_with_model(state, questions, model))
    }

    /// Like [`system_one`](Self::system_one) but with full per-call options
    /// via a [`SystemOneOpts`] builder.
    ///
    /// # Errors
    /// See [`TypeSafeClient::system_one_with_opts`] for the full list of
    /// error variants.
    pub fn system_one_with_opts(
        &self,
        questions: HashMap<String, Question>,
        opts: SystemOneOpts,
    ) -> Result<SystemOneResponse> {
        self.runtime
            .block_on(self.inner.system_one_with_opts(questions, opts))
    }

    /// List available models. Sends `GET /v1/models`.
    ///
    /// # Errors
    /// See [`TypeSafeClient::list_models`] for the full list of error variants.
    pub fn list_models(&self) -> Result<ListModelsResponse> {
        self.runtime.block_on(self.inner.list_models())
    }
}
