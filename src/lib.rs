//! # TypeSafe AI Rust SDK
//!
//! Rust SDK for [TypeSafe AI](https://typesafe.ai) — typed judgments from
//! System One models like Jev.
//!
//! See the [Quickstart](#quickstart) below or the full
//! [primitives documentation](#primitives) in the README.
//!
//! ## Primitives
//!
//! - [`noul`] — yes/no question → probability in `[0, 1]`
//! - [`choice`] — multiple-choice → label, probabilities, confidence
//! - [`score`] — rubric-based → numeric score, legend, probabilities, confidence

#![forbid(unsafe_code)]
#![warn(missing_docs, missing_debug_implementations)]
// Include README.md as a doctest so code samples in the README are verified
// by `cargo test --doc`.
#![doc = include_str!("../README.md")]

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
    SystemOneResponse, Usage,
};

#[cfg(feature = "blocking")]
pub use blocking::BlockingClient;
