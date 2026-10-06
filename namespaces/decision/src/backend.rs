//! Decision backends: one batch of labelled choice questions about one state
//! document, answered with one probability distribution per question.
//!
//! The elements build a provider-neutral [`Batch`]; a [`Backend`] turns it
//! into one HTTP request and its response into [`Distribution`]s. Every
//! decision the elements take is computed from those distributions alone
//! ([`argmax`]: highest probability, ties broken by the label's UTF-8 bytes),
//! so a backend contributes probabilities and nothing else. [`TypeSafe`] is
//! the only backend today; a CLM or NVIDIA backend implements the same trait.

use scxml_event_schema::canonical_json;
use serde_json::{Map, Value as Json};

use crate::{Part, Request};

/// One labelled alternative of a question.
#[derive(Clone, Debug, PartialEq)]
pub struct Criterion {
    /// The label the answer names.
    pub label: String,
    /// What the label means: a string, a JSON object or array, or `null`
    /// for an undescribed label.
    pub description: Json,
}

/// One choice question: pick one of the criteria.
#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    /// The question's name in the batch (unique).
    pub name: String,
    /// What to decide.
    pub instructions: String,
    /// The alternatives (labels unique), in the element's canonical order.
    pub criteria: Vec<Criterion>,
}

/// One batch of questions about one state document.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    /// The model the element names, or `None` for the backend's default.
    pub model: Option<String>,
    /// The state document: a string, a JSON object, or a JSON array.
    pub state: Json,
    /// The questions, in order.
    pub questions: Vec<Question>,
}

/// The probabilities a backend gave one question's criteria, in the
/// question's criteria order; a label the backend did not mention is `0`.
#[derive(Clone, Debug, PartialEq)]
pub struct Distribution {
    /// One probability in `[0, 1]` per criterion.
    pub probabilities: Vec<f64>,
}

/// A decision backend.
pub trait Backend {
    /// The one `POST` that asks `batch`: `base_url` is the `baseurl` slot's
    /// reference (no backend URL is compiled in) and `api_key` the `apikey`
    /// slot's.
    fn request(&self, batch: &Batch, api_key: u32, base_url: u32) -> Request;

    /// The distributions of a successful response body, one per question of
    /// `batch`, in order.
    ///
    /// # Errors
    ///
    /// A static detail when the body does not answer exactly the batch's
    /// questions with valid distributions over their labels.
    fn answers(&self, batch: &Batch, body: &[u8]) -> Result<Vec<Distribution>, &'static str>;
}

/// The index of the most probable criterion: highest probability, ties
/// broken by the smallest label (UTF-8 byte order); `None` for no criteria.
#[must_use]
pub fn argmax(criteria: &[Criterion], distribution: &Distribution) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (index, criterion) in criteria.iter().enumerate() {
        let probability = distribution
            .probabilities
            .get(index)
            .copied()
            .unwrap_or(0.0);
        best = match best {
            None => Some(index),
            Some(current) => {
                let current_probability = distribution
                    .probabilities
                    .get(current)
                    .copied()
                    .unwrap_or(0.0);
                let current_label = criteria
                    .get(current)
                    .map_or("", |criterion| criterion.label.as_str());
                if probability > current_probability
                    || (probability.total_cmp(&current_probability).is_eq()
                        && criterion.label.as_str() < current_label)
                {
                    Some(index)
                } else {
                    Some(current)
                }
            }
        };
    }
    best
}

/// `TypeSafe`'s `system_one` API, as `typesafe-sdk` 0.6 speaks it:
/// `POST {base}/v1/systemone` with `Authorization: Bearer <key>` and the
/// canonical JSON body `{"model":M,"questions":{N:{"criteria":{L:D},"instructions":I,"type":"choice"}},"state":S}`;
/// the answer is `{"model":…,"answers":{N:{"type":"choice","choice":L,"confidence":…,"probabilities":{L:P}}},"usage":…}`.
#[derive(Clone, Copy, Debug, Default)]
pub struct TypeSafe;

impl TypeSafe {
    /// The path appended to the base URL (the SDK's `SYSTEM_ONE_PATH`).
    pub const SYSTEM_ONE_PATH: &'static str = "/v1/systemone";
    /// The model without a `model` attribute (the SDK's `DEFAULT_MODEL`).
    pub const DEFAULT_MODEL: &'static str = "jev-latest";
}

impl Backend for TypeSafe {
    fn request(&self, batch: &Batch, api_key: u32, base_url: u32) -> Request {
        let questions: Map<String, Json> = batch
            .questions
            .iter()
            .map(|question| {
                let criteria: Map<String, Json> = question
                    .criteria
                    .iter()
                    .map(|criterion| (criterion.label.clone(), criterion.description.clone()))
                    .collect();
                (
                    question.name.clone(),
                    serde_json::json!({
                        "type": "choice",
                        "instructions": question.instructions,
                        "criteria": criteria,
                    }),
                )
            })
            .collect();
        let body = serde_json::json!({
            "model": batch.model.as_deref().unwrap_or(Self::DEFAULT_MODEL),
            "questions": questions,
            "state": batch.state,
        });
        Request {
            url: vec![
                Part::Reference(base_url),
                Part::Literal(Self::SYSTEM_ONE_PATH.into()),
            ],
            headers: vec![
                (
                    "authorization".into(),
                    vec![Part::Literal("Bearer ".into()), Part::Reference(api_key)],
                ),
                (
                    "content-type".into(),
                    vec![Part::Literal("application/json".into())],
                ),
                (
                    "accept".into(),
                    vec![Part::Literal("application/json".into())],
                ),
            ],
            body: canonical_json(&body).into_bytes(),
        }
    }

    fn answers(&self, batch: &Batch, body: &[u8]) -> Result<Vec<Distribution>, &'static str> {
        const MALFORMED: &str = "the decision response is not a valid system_one answer";
        let value: Json = serde_json::from_slice(body).map_err(|_| MALFORMED)?;
        let answers = value
            .get("answers")
            .and_then(Json::as_object)
            .ok_or(MALFORMED)?;
        if answers.len() != batch.questions.len() {
            return Err("the decision response does not answer exactly the questions asked");
        }
        batch
            .questions
            .iter()
            .map(|question| {
                let answer = answers
                    .get(&question.name)
                    .and_then(Json::as_object)
                    .ok_or("the decision response does not answer exactly the questions asked")?;
                if answer.get("type").and_then(Json::as_str) != Some("choice") {
                    return Err("the decision response answers a question with another type than choice");
                }
                let known = |label: &str| {
                    question
                        .criteria
                        .iter()
                        .position(|criterion| criterion.label == label)
                };
                answer
                    .get("choice")
                    .and_then(Json::as_str)
                    .and_then(known)
                    .ok_or("the decision response chose a label the question does not offer")?;
                let probabilities = answer
                    .get("probabilities")
                    .and_then(Json::as_object)
                    .ok_or(MALFORMED)?;
                let mut distribution = vec![0.0; question.criteria.len()];
                for (label, probability) in probabilities {
                    let index = known(label).ok_or(
                        "the decision response gives a probability to a label the question does not offer",
                    )?;
                    let probability = probability
                        .as_f64()
                        .filter(|probability| (0.0..=1.0).contains(probability))
                        .ok_or("the decision response holds a probability outside [0, 1]")?;
                    if let Some(slot) = distribution.get_mut(index) {
                        *slot = probability;
                    }
                }
                Ok(Distribution {
                    probabilities: distribution,
                })
            })
            .collect()
    }
}
