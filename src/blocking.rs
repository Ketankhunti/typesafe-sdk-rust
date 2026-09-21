//! Blocking (synchronous) client for the TypeSafe AI API.
//!
//! Enabled with the `blocking` feature flag. Wraps the async
//! [`TypeSafeClient`] with a dedicated multi-threaded Tokio runtime
//! (one worker thread) so you can call the API without `async`/`await`.
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
/// Wraps an async [`TypeSafeClient`] with a dedicated multi-threaded Tokio
/// runtime (one worker thread). Each method call blocks the calling thread
/// until the response is received — up to the configured timeout multiplied
/// by (retries + 1), with retries bounded by the budget (potentially 30 s or
/// more).
///
/// Construct with [`BlockingClient::new`], [`BlockingClient::from_env`], or
/// [`BlockingClient::from_config`].
///
/// # Async-context safety
///
/// The constructors and API methods will **not panic** when called from
/// inside a Tokio async context (including `spawn_blocking` threads).
/// When a Tokio runtime is detected on the current thread, `block_on` is
/// executed on a dedicated helper thread to avoid the "cannot start a
/// runtime from within a runtime" panic.
///
/// However, each call still **blocks the calling executor thread** for the
/// entire duration of the request (up to timeout × retries). On a
/// current-thread runtime this stalls all other tasks. For async code,
/// prefer [`TypeSafeClient`] directly, or wrap blocking calls in
/// `tokio::task::spawn_blocking`.
///
/// Dropping a `BlockingClient` inside an async context is safe because the
/// runtime is shut down in the background via `shutdown_background()`.
#[non_exhaustive]
pub struct BlockingClient {
    inner: TypeSafeClient,
    runtime: Option<tokio::runtime::Runtime>,
}

impl fmt::Debug for BlockingClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlockingClient")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl BlockingClient {
    /// Run a future to completion, blocking the calling thread.
    ///
    /// If the current thread is inside a Tokio runtime, `block_on` is
    /// executed on a dedicated helper thread to avoid the "cannot start a
    /// runtime from within a runtime" panic. This makes the blocking client
    /// usable from `spawn_blocking` and other async-adjacent contexts.
    fn block_on<F>(&self, future: F) -> F::Output
    where
        F: std::future::Future + Send,
        F::Output: Send,
    {
        let runtime = self.runtime.as_ref().expect("runtime missing");
        if tokio::runtime::Handle::try_current().is_ok() {
            // We're inside a Tokio runtime. Running `block_on` directly
            // would panic, so spawn a helper thread, run the future there,
            // and wait for the result.
            let handle = runtime.handle().clone();
            let (tx, rx) = std::sync::mpsc::channel();
            // The scope closure returns the result of `join()`, which is
            // `Result<F::Output, Box<dyn Any + Send>>`.  If the helper
            // thread panicked, `join()` gives us the panic payload so we
            // can `resume_unwind` it — preserving the original error for
            // `catch_unwind` callers instead of a synthetic message.
            let join_result = std::thread::scope(|scope| {
                let join_handle = scope.spawn(move || {
                    let result = handle.block_on(future);
                    let _ = tx.send(result);
                });
                // Wait for the channel result first. If `recv()` fails,
                // the helper thread panicked, so we `join()` to recover
                // the panic payload.
                match rx.recv() {
                    Ok(result) => Ok(result),
                    Err(_) => Err(join_handle.join()),
                }
            });
            match join_result {
                Ok(result) => result,
                Err(join_err) => match join_err {
                    Err(payload) => std::panic::resume_unwind(payload),
                    Ok(_) => panic!("helper thread exited without sending a result"),
                },
            }
        } else {
            runtime.block_on(future)
        }
    }

    /// Build a multi-threaded Tokio runtime, returning a `Runtime` error on
    /// failure. A multi-threaded runtime is used so that `Handle::block_on`
    /// can drive the event loop from a helper thread when the blocking
    /// client is used inside an async context.
    fn build_runtime() -> Result<tokio::runtime::Runtime> {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| {
                TypeSafeError::new(ErrorKind::Runtime(format!(
                    "Failed to create Tokio runtime for blocking client: {e}"
                )))
            })
    }

    /// Create a new blocking client with the given API key and default settings.
    ///
    /// # Errors
    /// - [`ErrorKind::Validation`] if the API key is empty.
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
    /// - [`ErrorKind::Validation`] if the API key is empty, the base URL
    ///   is invalid, or the default model name is empty.
    /// - [`ErrorKind::Runtime`] if the Tokio runtime cannot be created.
    #[must_use = "the returned client should be used to make API calls"]
    pub fn from_config(config: ClientConfig) -> Result<Self> {
        let inner = TypeSafeClient::from_config(config)?;
        let runtime = Self::build_runtime()?;
        Ok(Self {
            inner,
            runtime: Some(runtime),
        })
    }

    /// Create a new blocking client, reading the API key from the
    /// `TYPESAFE_API_KEY` environment variable. Also reads
    /// `TYPESAFE_BASE_URL` and `TYPESAFE_DEFAULT_MODEL` if set.
    ///
    /// # Errors
    /// - [`ErrorKind::Validation`] if `TYPESAFE_API_KEY` is unset or empty.
    /// - [`ErrorKind::Runtime`] if the Tokio runtime cannot be created.
    #[must_use = "the returned client should be used to make API calls"]
    pub fn from_env() -> Result<Self> {
        let inner = TypeSafeClient::from_env()?;
        let runtime = Self::build_runtime()?;
        Ok(Self {
            inner,
            runtime: Some(runtime),
        })
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
    /// The `state` parameter requires [`Send`] because it is moved to a
    /// helper thread where the Tokio runtime executes the future.
    ///
    /// # Errors
    /// See [`TypeSafeClient::system_one`] for the full list of error variants.
    pub fn system_one(
        &self,
        state: impl Into<Value> + Send,
        questions: HashMap<String, Question>,
    ) -> Result<SystemOneResponse> {
        self.block_on(self.inner.system_one(state, questions))
    }

    /// Like [`system_one`](Self::system_one) but with an explicit model
    /// override (`None` uses the client default).
    ///
    /// The `state` parameter requires [`Send`] because it is moved to a
    /// helper thread where the Tokio runtime executes the future.
    ///
    /// # Errors
    /// See [`TypeSafeClient::system_one_with_model`] for the full list of
    /// error variants.
    pub fn system_one_with_model(
        &self,
        state: impl Into<Value> + Send,
        questions: HashMap<String, Question>,
        model: Option<&str>,
    ) -> Result<SystemOneResponse> {
        self.block_on(self.inner.system_one_with_model(state, questions, model))
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
        self.block_on(self.inner.system_one_with_opts(questions, opts))
    }

    /// List available models. Sends `GET /v1/models`.
    ///
    /// # Errors
    /// See [`TypeSafeClient::list_models`] for the full list of error variants.
    pub fn list_models(&self) -> Result<ListModelsResponse> {
        self.block_on(self.inner.list_models())
    }
}

impl Drop for BlockingClient {
    fn drop(&mut self) {
        // Shut down the runtime in the background so that dropping a
        // `BlockingClient` inside an async context does not panic.
        // `Runtime::drop` calls `block_on` internally to wait for tasks to
        // finish, which panics if called from within a runtime. Using
        // `shutdown_background()` avoids this by not blocking.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

// Static assertion that the blocking client is `Send + Sync`. The blocking
// client's soundness story depends on being usable from any thread (it owns
// a dedicated runtime). The `where` clause is verified at definition time, so
// if a future change breaks this bound, compilation will fail.
const _: () = {
    #[allow(dead_code)]
    fn _assert_blocking_client_send_sync()
    where
        BlockingClient: Send + Sync,
    {
    }
};
