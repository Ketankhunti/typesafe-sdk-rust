//! # TypeSafe AI Rust SDK
//!
//! Rust SDK for [TypeSafe AI](https://typesafe.ai) — typed judgments from
//! System One models like Jev.
//!
//! ## Quickstart
//!
//! Set `TYPESAFE_API_KEY` in your environment, then:
//!
//! ```no_run
//! use typesafeai_sdk::{TypeSafeClient, noul, choice, score, Question};
//! use std::collections::HashMap;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = TypeSafeClient::from_env()?;
//!
//!     let mut questions: HashMap<String, Question> = HashMap::new();
//!     questions.insert("billing".to_string(), noul("Is this about billing?").into());
//!
//!     let response = client.system_one("I was charged twice. Please help.", questions).await?;
//!
//!     if let Some(answer) = response.noul("billing") {
//!         println!("Billing probability: {}", answer.noul);
//!     }
//!     Ok(())
//! }
//! ```
//!
//! ## Primitives
//!
//! - [`noul`] — yes/no question → probability in `[0, 1]`
//! - [`choice`] — multiple-choice → label, probabilities, confidence
//! - [`score`] — rubric-based → numeric score, legend, probabilities, confidence

#![forbid(unsafe_code)]
#![warn(missing_docs, missing_debug_implementations)]

mod client;
mod error;
mod questions;
mod retry;
mod types;

#[cfg(feature = "blocking")]
pub mod blocking;

pub use client::{ClientConfig, SystemOneOpts, TypeSafeClient};
pub use error::{ErrorKind, Result, TypeSafeError};
pub use questions::{
    choice, noul, score, Choice, Description, Noul, NoulCriteria, Question, Score,
};
pub use retry::RetryPolicy;
pub use types::{
    Answer, ChoiceAnswer, ListModelsResponse, ModelCard, NoulAnswer, ScoreAnswer,
    SystemOneRequest, SystemOneResponse, Usage,
};

#[cfg(feature = "blocking")]
pub use blocking::BlockingClient;
