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
pub struct SystemOneRequest {
    pub state: Value,
    pub model: String,
    pub questions: HashMap<String, Question>,
}

// ---------------------------------------------------------------------------
// Response
// ---------------------------------------------------------------------------

/// Token usage for a request.
#[derive(Debug, Clone, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// A yes/no answer.
#[derive(Debug, Clone, Deserialize)]
pub struct NoulAnswer {
    /// Probability of a "yes" answer, from 0 to 1.
    pub noul: f64,
}

/// A choice answer.
#[derive(Debug, Clone, Deserialize)]
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
pub enum Answer {
    #[serde(rename = "noul")]
    Noul(NoulAnswer),
    #[serde(rename = "choice")]
    Choice(ChoiceAnswer),
    #[serde(rename = "score")]
    Score(ScoreAnswer),
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
pub struct SystemOneResponse {
    /// The model used to answer the request.
    pub model: String,
    /// All answers keyed by question name.
    pub answers: HashMap<String, Answer>,
    /// Token usage for the request.
    pub usage: Usage,
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
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

/// Metadata for an available model.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelCard {
    pub name: String,
    pub description: String,
    /// Release date of the model, if available.
    #[serde(default)]
    pub release_date: Option<String>,
}

/// The response from `GET /v1/models`.
#[derive(Debug, Clone, Deserialize)]
pub struct ListModelsResponse {
    pub models: Vec<ModelCard>,
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
}
