//! Question types for the TypeSafe SDK.
//!
//! Three primitives, matching the Python and JavaScript SDKs:
//!
//! - [`Noul`] — a yes/no question returning a probability in `[0, 1]`.
//! - [`Choice`] — a multiple-choice question returning a label, probabilities,
//!   and a confidence score.
//! - [`Score`] — a rubric-based question returning a numeric score, a legend,
//!   probabilities, and a confidence score.
//!
//! Each type serializes to the wire format expected by `POST /v1/systemone`:
//! a JSON object with a `"type"` discriminator and `"instructions"` /
//! `"criteria"` fields.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Description type
// ---------------------------------------------------------------------------

/// A criterion description. Can be a string, a JSON object/array, or `null`
/// (meaning "undescribed"). Mirrors the JS SDK's `EntryType` / `Description`.
pub type Description = Option<Value>;

// ---------------------------------------------------------------------------
// Noul
// ---------------------------------------------------------------------------

/// Optional descriptions for the `true` and `false` outcomes of a [`Noul`]
/// question.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NoulCriteria {
    /// Description of the "yes" outcome.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#true: Description,
    /// Description of the "no" outcome.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#false: Description,
}

/// A yes/no question. Returns a probability in `[0, 1]`.
///
/// Wire format: `{"type":"noul","instructions":"...","criteria":{"true":...,"false":...}}`
///
/// The `"type"` tag is added by the [`Question`] enum's `#[serde(tag = "type")]`
/// attribute; this struct only carries `instructions` and `criteria`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Noul {
    /// The question as text, a JSON object, or an array. May be `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Description,
    /// Optional descriptions of the yes and no outcomes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub criteria: Option<NoulCriteria>,
}

impl Noul {
    /// Create a new noul question with the given instructions.
    ///
    /// `instructions` can be a string, `null`, or any JSON value.
    /// Pass [`serde_json::Value::Null`] to explicitly send `null` as the
    /// instructions (the API default).
    #[must_use = "the returned Noul should be used"]
    pub fn new(instructions: impl Into<Value>) -> Self {
        Self {
            instructions: Some(instructions.into()),
            criteria: None,
        }
    }

    /// Attach criteria describing the yes/no outcomes.
    #[must_use = "the returned Noul should be used"]
    pub fn with_criteria(mut self, criteria: NoulCriteria) -> Self {
        self.criteria = Some(criteria);
        self
    }
}

// ---------------------------------------------------------------------------
// Choice
// ---------------------------------------------------------------------------

/// A multiple-choice question. Returns a selected label, per-label
/// probabilities, and a confidence score.
///
/// Wire format:
/// `{"type":"choice","instructions":"...","criteria":{"label":"desc",...}}`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Choice {
    /// The question as text, a JSON object, or an array. May be `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Description,
    /// Labels mapped to descriptions. `null` means "undescribed".
    pub criteria: HashMap<String, Description>,
}

impl Choice {
    /// Create a new choice question with the given instructions and criteria.
    ///
    /// `criteria` is a map of label → description (or `None` for undescribed).
    #[must_use = "the returned Choice should be used"]
    pub fn new(instructions: impl Into<Value>, criteria: HashMap<String, Description>) -> Self {
        Self {
            instructions: Some(instructions.into()),
            criteria,
        }
    }
}

// ---------------------------------------------------------------------------
// Score
// ---------------------------------------------------------------------------

/// A rubric-based question. Returns a numeric score, a legend mapping scores
/// to descriptions, per-score probabilities, and a confidence score.
///
/// Wire format:
/// `{"type":"score","instructions":"...","criteria":["desc0","desc1",...]}`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Score {
    /// The question as text, a JSON object, or an array. May be `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Description,
    /// Ordered list of descriptions indexed by score from zero. At least two
    /// entries are required. Entries may be `null` (undescribed).
    pub criteria: Vec<Description>,
}

impl Score {
    /// Create a new score question with the given instructions and criteria.
    ///
    /// `criteria` must have at least two entries.
    #[must_use = "the returned Score should be used"]
    pub fn new(instructions: impl Into<Value>, criteria: Vec<Description>) -> Self {
        Self {
            instructions: Some(instructions.into()),
            criteria,
        }
    }
}

// ---------------------------------------------------------------------------
// Question enum
// ---------------------------------------------------------------------------

/// A question of any type, identified by its `type` field.
///
/// This enum is used as the value type in the `questions` map sent to the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum Question {
    /// A yes/no question.
    #[serde(rename = "noul")]
    Noul(Noul),
    /// A multiple-choice question.
    #[serde(rename = "choice")]
    Choice(Choice),
    /// A rubric-based score question.
    #[serde(rename = "score")]
    Score(Score),
}

impl From<Noul> for Question {
    fn from(q: Noul) -> Self {
        Question::Noul(q)
    }
}

impl From<Choice> for Question {
    fn from(q: Choice) -> Self {
        Question::Choice(q)
    }
}

impl From<Score> for Question {
    fn from(q: Score) -> Self {
        Question::Score(q)
    }
}

impl Question {
    /// Returns `true` if this is a noul question.
    pub fn is_noul(&self) -> bool {
        matches!(self, Question::Noul(_))
    }

    /// Returns `true` if this is a choice question.
    pub fn is_choice(&self) -> bool {
        matches!(self, Question::Choice(_))
    }

    /// Returns `true` if this is a score question.
    pub fn is_score(&self) -> bool {
        matches!(self, Question::Score(_))
    }

    /// Returns the noul question if this is one.
    pub fn as_noul(&self) -> Option<&Noul> {
        if let Question::Noul(n) = self {
            Some(n)
        } else {
            None
        }
    }

    /// Returns the choice question if this is one.
    pub fn as_choice(&self) -> Option<&Choice> {
        if let Question::Choice(c) = self {
            Some(c)
        } else {
            None
        }
    }

    /// Returns the score question if this is one.
    pub fn as_score(&self) -> Option<&Score> {
        if let Question::Score(s) = self {
            Some(s)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Builder functions (mirrors the JS SDK's `noul()`, `choice()`, `score()`)
// ---------------------------------------------------------------------------

/// Build a yes/no question.
///
/// `instructions` is required — pass a string, `serde_json::Value::Null`,
/// or any JSON value. To omit instructions on the wire, pass
/// [`serde_json::Value::Null`].
///
/// # Examples
/// ```
/// use typesafeai_sdk::noul;
///
/// let q = noul("Is this about billing?");
/// let q = noul(serde_json::Value::Null); // null instructions
/// ```
#[must_use = "the returned Noul should be used"]
pub fn noul(instructions: impl Into<Value>) -> Noul {
    Noul::new(instructions)
}

/// Build a multiple-choice question.
///
/// # Examples
/// ```
/// use typesafeai_sdk::choice;
/// use std::collections::HashMap;
///
/// let mut criteria = HashMap::new();
/// criteria.insert("calm".to_string(), None);
/// criteria.insert("angry".to_string(), None);
/// let q = choice("What is the tone?", criteria);
/// ```
#[must_use = "the returned Choice should be used"]
pub fn choice(instructions: impl Into<Value>, criteria: HashMap<String, Description>) -> Choice {
    Choice::new(instructions, criteria)
}

/// Build a rubric-based score question. `criteria` must have at least two
/// entries.
///
/// # Examples
/// ```
/// use typesafeai_sdk::score;
///
/// let q = score("How urgent?", vec![None, Some("high".into())]);
/// ```
#[must_use = "the returned Score should be used"]
pub fn score(instructions: impl Into<Value>, criteria: Vec<Description>) -> Score {
    Score::new(instructions, criteria)
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate a set of questions before sending.
///
/// Mirrors the JS SDK's `validateQuestions`:
/// - Questions must not be empty.
/// - Choice criteria must have at least two entries.
/// - Score criteria must be an array with at least two entries.
pub(crate) fn validate_questions(
    questions: &HashMap<String, Question>,
) -> crate::error::Result<()> {
    if questions.is_empty() {
        return Err(crate::error::TypeSafeError::new(
            crate::error::ErrorKind::Validation("At least one question is required.".to_string()),
        ));
    }

    for (name, question) in questions {
        if name.trim().is_empty() {
            return Err(crate::error::TypeSafeError::new(
                crate::error::ErrorKind::Validation(
                    "Question names cannot be empty or whitespace.".to_string(),
                ),
            ));
        }
        match question {
            Question::Choice(choice) => {
                if choice.criteria.len() < 2 {
                    return Err(crate::error::TypeSafeError::new(
                        crate::error::ErrorKind::Validation(format!(
                            "Choice question \"{name}\" has {} criteria; at least two choices are required.",
                            choice.criteria.len()
                        )),
                    ));
                }
            }
            Question::Score(score) => {
                if score.criteria.len() < 2 {
                    return Err(crate::error::TypeSafeError::new(
                        crate::error::ErrorKind::Validation(format!(
                            "Score question \"{name}\" has {} criteria; at least two scores are required.",
                            score.criteria.len()
                        )),
                    ));
                }
            }
            Question::Noul(_) => {}
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noul_serializes_correctly() {
        let q: Question = noul("Is this about billing?").into();
        let json = serde_json::to_value(&q).unwrap();
        assert_eq!(json["type"], "noul");
        assert_eq!(json["instructions"], "Is this about billing?");
        assert!(json.get("criteria").is_none());
    }

    #[test]
    fn noul_with_criteria_serializes_correctly() {
        let q = noul("?").with_criteria(NoulCriteria {
            r#true: Some("yes means this".into()),
            r#false: None,
        });
        let json = serde_json::to_value(&q).unwrap();
        assert_eq!(json["criteria"]["true"], "yes means this");
        assert!(json["criteria"].get("false").is_none());
    }

    #[test]
    fn choice_serializes_correctly() {
        let mut criteria = HashMap::new();
        criteria.insert("calm".to_string(), None);
        criteria.insert("angry".to_string(), Some("very upset".into()));
        let q: Question = choice("Tone?", criteria).into();
        let json = serde_json::to_value(&q).unwrap();
        assert_eq!(json["type"], "choice");
        assert_eq!(json["instructions"], "Tone?");
        assert_eq!(json["criteria"]["calm"], serde_json::Value::Null);
        assert_eq!(json["criteria"]["angry"], "very upset");
    }

    #[test]
    fn score_serializes_correctly() {
        let q: Question = score("Urgency?", vec![Some("low".into()), Some("high".into())]).into();
        let json = serde_json::to_value(&q).unwrap();
        assert_eq!(json["type"], "score");
        assert_eq!(json["instructions"], "Urgency?");
        assert_eq!(json["criteria"][0], "low");
        assert_eq!(json["criteria"][1], "high");
    }

    #[test]
    fn question_enum_serializes_with_type_tag() {
        let q: Question = noul("?").into();
        let json = serde_json::to_value(&q).unwrap();
        assert_eq!(json["type"], "noul");
    }

    #[test]
    fn validate_rejects_empty_questions() {
        let questions = HashMap::new();
        let result = validate_questions(&questions);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("At least one question is required"));
    }

    #[test]
    fn validate_rejects_empty_question_name() {
        let mut questions = HashMap::new();
        questions.insert("".to_string(), Question::Noul(noul("?")));
        let result = validate_questions(&questions);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Question names cannot be empty"));
    }

    #[test]
    fn validate_rejects_whitespace_question_name() {
        let mut questions = HashMap::new();
        questions.insert("   ".to_string(), Question::Noul(noul("?")));
        let result = validate_questions(&questions);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Question names cannot be empty"));
    }

    #[test]
    fn validate_rejects_score_with_fewer_than_two_criteria() {
        let mut questions = HashMap::new();
        questions.insert(
            "q".to_string(),
            Question::Score(score("?", vec![Some("only".into())])),
        );
        let result = validate_questions(&questions);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("at least two"));
    }

    #[test]
    fn validate_rejects_choice_with_fewer_than_two_criteria() {
        let mut questions = HashMap::new();
        let mut criteria = HashMap::new();
        criteria.insert("only".to_string(), None);
        questions.insert("q".to_string(), Question::Choice(choice("?", criteria)));
        let result = validate_questions(&questions);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("at least two"));
    }

    #[test]
    fn validate_accepts_valid_questions() {
        let mut questions = HashMap::new();
        questions.insert("q".to_string(), Question::Noul(noul("?")));
        questions.insert(
            "s".to_string(),
            Question::Score(score("?", vec![Some("low".into()), Some("high".into())])),
        );
        let mut choice_criteria = HashMap::new();
        choice_criteria.insert("a".to_string(), None);
        choice_criteria.insert("b".to_string(), None);
        questions.insert(
            "c".to_string(),
            Question::Choice(choice("?", choice_criteria)),
        );
        assert!(validate_questions(&questions).is_ok());
    }
}
