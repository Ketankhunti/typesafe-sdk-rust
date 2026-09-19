//! Request and response types for the TypeSafe SDK.
//!
//! Mirrors the Python SDK's `SystemOneRequest` / `SystemOneResponse` and the
//! JS SDK's `SystemOneRequest` / `SystemOneResult`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::questions::Question;

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// The request body for `POST /v1/systemone`.
///
/// Fields:
/// - `state` — text, a JSON object, or an array to evaluate.
/// - `model` — the model name or alias (e.g. `"jev-latest"`).
/// - `questions` — non-empty map of question names to question objects.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct SystemOneRequest {
    /// Text, a JSON object, or an array to evaluate.
    pub state: Value,
    /// The model name or alias (e.g. `"jev-latest"`).
    pub model: String,
    /// Non-empty map of question names to question objects.
    pub questions: HashMap<String, Question>,
}

// ---------------------------------------------------------------------------
// Response
// ---------------------------------------------------------------------------

/// Token usage for a request.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Usage {
    /// Number of input tokens consumed.
    pub input_tokens: u64,
    /// Number of output tokens generated.
    pub output_tokens: u64,
}

/// A yes/no answer.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct NoulAnswer {
    /// Probability of a "yes" answer, from 0 to 1.
    pub noul: f64,
}

/// A choice answer.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct ChoiceAnswer {
    /// The selected label.
    pub choice: String,
    /// Per-label probabilities.
    pub probabilities: HashMap<String, f64>,
    /// Confidence score in `[0, 1]`.
    pub confidence: f64,
}

/// A score answer.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct ScoreAnswer {
    /// The assigned score (0-indexed). Returned as a float by the API
    /// (e.g. `0.0`); cast to `u32` if you need an integer index.
    pub score: f64,
    /// Rubric descriptions keyed by score.
    pub legend: HashMap<String, Value>,
    /// Per-score probabilities.
    pub probabilities: HashMap<String, f64>,
    /// Confidence score in `[0, 1]`.
    pub confidence: f64,
}

/// An answer to a single question, identified by its `type` field.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum Answer {
    /// A yes/no answer.
    #[serde(rename = "noul")]
    Noul(NoulAnswer),
    /// A multiple-choice answer.
    #[serde(rename = "choice")]
    Choice(ChoiceAnswer),
    /// A rubric-based score answer.
    #[serde(rename = "score")]
    Score(ScoreAnswer),
    /// An unknown answer type returned by a newer API version.
    /// This prevents deserialization failures when the server adds new
    /// primitives that this SDK version doesn't yet know about.
    #[serde(other)]
    Unknown,
}

impl Answer {
    /// Returns `true` if this is a noul answer.
    pub fn is_noul(&self) -> bool {
        matches!(self, Answer::Noul(_))
    }

    /// Returns `true` if this is a choice answer.
    pub fn is_choice(&self) -> bool {
        matches!(self, Answer::Choice(_))
    }

    /// Returns `true` if this is a score answer.
    pub fn is_score(&self) -> bool {
        matches!(self, Answer::Score(_))
    }

    /// Returns the noul answer if this is one.
    pub fn as_noul(&self) -> Option<&NoulAnswer> {
        if let Answer::Noul(n) = self {
            Some(n)
        } else {
            None
        }
    }

    /// Returns the choice answer if this is one.
    pub fn as_choice(&self) -> Option<&ChoiceAnswer> {
        if let Answer::Choice(c) = self {
            Some(c)
        } else {
            None
        }
    }

    /// Returns the score answer if this is one.
    pub fn as_score(&self) -> Option<&ScoreAnswer> {
        if let Answer::Score(s) = self {
            Some(s)
        } else {
            None
        }
    }
}

/// The response from `POST /v1/systemone`.
///
/// Answers are grouped by question name in the `answers` field. You can also
/// access them by type using the [`SystemOneResponse::nouls`],
/// [`SystemOneResponse::choices`], and [`SystemOneResponse::scores`] helper
/// methods, mirroring the Python SDK's grouped accessors.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct SystemOneResponse {
    /// The model used to answer the request.
    pub model: String,
    /// All answers keyed by question name.
    pub answers: HashMap<String, Answer>,
    /// Token usage for the request.
    pub usage: Usage,
    /// The request ID returned by the server (from the
    /// `x-typesafe-request-id` response header), if present.
    #[serde(default)]
    pub request_id: Option<String>,
    /// The raw JSON body of the response, injected after deserialization.
    /// Useful for accessing fields the SDK doesn't yet model.
    #[serde(skip)]
    pub raw: Option<Value>,
}

impl SystemOneResponse {
    /// Returns all noul answers keyed by question name.
    pub fn nouls(&self) -> HashMap<&str, &NoulAnswer> {
        self.answers
            .iter()
            .filter_map(|(k, v)| v.as_noul().map(|n| (k.as_str(), n)))
            .collect()
    }

    /// Returns all choice answers keyed by question name.
    pub fn choices(&self) -> HashMap<&str, &ChoiceAnswer> {
        self.answers
            .iter()
            .filter_map(|(k, v)| v.as_choice().map(|c| (k.as_str(), c)))
            .collect()
    }

    /// Returns all score answers keyed by question name.
    pub fn scores(&self) -> HashMap<&str, &ScoreAnswer> {
        self.answers
            .iter()
            .filter_map(|(k, v)| v.as_score().map(|s| (k.as_str(), s)))
            .collect()
    }

    /// Returns the noul answer for the given question name, if it exists.
    pub fn noul(&self, name: &str) -> Option<&NoulAnswer> {
        self.answers.get(name).and_then(|a| a.as_noul())
    }

    /// Returns the choice answer for the given question name, if it exists.
    pub fn choice(&self, name: &str) -> Option<&ChoiceAnswer> {
        self.answers.get(name).and_then(|a| a.as_choice())
    }

    /// Returns the score answer for the given question name, if it exists.
    pub fn score(&self, name: &str) -> Option<&ScoreAnswer> {
        self.answers.get(name).and_then(|a| a.as_score())
    }

    /// Returns the request ID associated with this response, if the server
    /// provided one via the `x-typesafe-request-id` header.
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    /// Returns the raw JSON body of the response as a [`serde_json::Value`],
    /// if it was captured. This gives access to fields the SDK doesn't yet
    /// model.
    pub fn raw(&self) -> Option<&Value> {
        self.raw.as_ref()
    }
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

/// Metadata for an available model.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct ModelCard {
    /// The model name or alias.
    pub name: String,
    /// Human-readable description of the model.
    pub description: String,
    /// Release date of the model, if available.
    #[serde(default)]
    pub release_date: Option<String>,
}

/// The response from `GET /v1/models`.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct ListModelsResponse {
    /// List of available models.
    pub models: Vec<ModelCard>,
    /// The request ID returned by the server (from the
    /// `x-typesafe-request-id` response header), if present.
    #[serde(default)]
    pub request_id: Option<String>,
    /// The raw JSON body of the response, injected after deserialization.
    #[serde(skip)]
    pub raw: Option<Value>,
}

impl ListModelsResponse {
    /// Returns the request ID associated with this response, if the server
    /// provided one via the `x-typesafe-request-id` header.
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    /// Returns the raw JSON body of the response as a [`serde_json::Value`],
    /// if it was captured.
    pub fn raw(&self) -> Option<&Value> {
        self.raw.as_ref()
    }
}

// ---------------------------------------------------------------------------
// SetMetadata trait (internal)
// ---------------------------------------------------------------------------

/// Internal trait for response types that can carry a request ID (from the
/// `x-typesafe-request-id` header) and the raw JSON body.
///
/// Implemented by [`SystemOneResponse`] and [`ListModelsResponse`]. The
/// client calls [`set_metadata`](Self::set_metadata) after deserializing
/// the response body.
///
/// This trait is not exported: it is an implementation detail of the client's
/// generic `parse_response` method.
pub(crate) trait SetMetadata {
    /// Set the request ID and raw body on this response. `request_id` is
    /// `Some` only when the header was present, so a `None` value does not
    /// overwrite an ID that may have been in the body.
    fn set_metadata(&mut self, request_id: Option<String>, raw: Option<Value>);
}

impl SetMetadata for SystemOneResponse {
    fn set_metadata(&mut self, request_id: Option<String>, raw: Option<Value>) {
        if let Some(id) = request_id {
            self.request_id = Some(id);
        }
        self.raw = raw;
    }
}

impl SetMetadata for ListModelsResponse {
    fn set_metadata(&mut self, request_id: Option<String>, raw: Option<Value>) {
        if let Some(id) = request_id {
            self.request_id = Some(id);
        }
        self.raw = raw;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_noul_answer() {
        let json = r#"{"type":"noul","noul":0.95}"#;
        let answer: Answer = serde_json::from_str(json).unwrap();
        match answer {
            Answer::Noul(n) => assert!((n.noul - 0.95).abs() < 1e-9),
            _ => panic!("expected noul answer"),
        }
    }

    #[test]
    fn deserialize_choice_answer() {
        let json = r#"{"type":"choice","choice":"calm","probabilities":{"calm":0.9,"angry":0.1},"confidence":0.85}"#;
        let answer: Answer = serde_json::from_str(json).unwrap();
        match answer {
            Answer::Choice(c) => {
                assert_eq!(c.choice, "calm");
                assert!((c.confidence - 0.85).abs() < 1e-9);
                assert!((c.probabilities["calm"] - 0.9).abs() < 1e-9);
            }
            _ => panic!("expected choice answer"),
        }
    }

    #[test]
    fn deserialize_score_answer() {
        let json = r#"{"type":"score","score":1,"legend":{"0":"low","1":"high"},"probabilities":{"0":0.3,"1":0.7},"confidence":0.8}"#;
        let answer: Answer = serde_json::from_str(json).unwrap();
        match answer {
            Answer::Score(s) => {
                assert!((s.score - 1.0).abs() < 1e-9);
                assert!((s.confidence - 0.8).abs() < 1e-9);
            }
            _ => panic!("expected score answer"),
        }
    }

    #[test]
    fn deserialize_full_response() {
        let json = r#"{
            "model": "jev-latest",
            "answers": {
                "billing": {"type":"noul","noul":0.98},
                "tone": {"type":"choice","choice":"calm","probabilities":{"calm":0.9,"angry":0.1},"confidence":0.85}
            },
            "usage": {"input_tokens":120,"output_tokens":12}
        }"#;
        let resp: SystemOneResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.model, "jev-latest");
        assert_eq!(resp.usage.input_tokens, 120);
        assert_eq!(resp.nouls().len(), 1);
        assert_eq!(resp.choices().len(), 1);
        assert!((resp.nouls()["billing"].noul - 0.98).abs() < 1e-9);
        assert_eq!(resp.choices()["tone"].choice, "calm");
    }

    #[test]
    fn deserialize_unknown_answer_type() {
        let json = r#"{"type":"future_primitive","data":"something"}"#;
        let answer: Answer = serde_json::from_str(json).unwrap();
        assert!(matches!(answer, Answer::Unknown));
    }

    #[test]
    fn safe_accessors_return_none_for_missing() {
        let json = r#"{
            "model": "jev-latest",
            "answers": {
                "billing": {"type":"noul","noul":0.98}
            },
            "usage": {"input_tokens":10,"output_tokens":2}
        }"#;
        let resp: SystemOneResponse = serde_json::from_str(json).unwrap();
        assert!(resp.noul("billing").is_some());
        assert!(resp.noul("nonexistent").is_none());
        assert!(resp.choice("billing").is_none());
        assert!(resp.score("billing").is_none());
    }
}
