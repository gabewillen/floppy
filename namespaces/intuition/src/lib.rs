//! The `intuition` extension namespace: `<intuition:observe>` and
//! `<intuition:escalate>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! A [`ConfidenceTuner`] is a self-tuning confidence baseline on the 0–100
//! scale that decides when the fast stage escalates a turn to reasoning.
//! Each turn observes the turn's confidence first and then asks whether it
//! escalates against the updated threshold (D114). Both are synchronous
//! and quick, so they are executable content here, named after the tuner's
//! own methods (`should_` dropped, Q6). The tuner state lives in the data
//! model (a `confidence_tuner` datum) and crosses turns by `<param>` and
//! `<donedata>`. Documents bind [`NAMESPACE_URI`] (conventionally with the
//! prefix `intuition`):
//!
//! ```xml
//! <intuition:observe location="confidence_tuner" confidence="confidence"/>
//! <intuition:escalate location="escalate" confidence="confidence" tuner="confidence_tuner"/>
//! ```
//!
//! # The tuner state
//!
//! `{mean, var, n}`: numbers, `n` a non-negative integer, with the
//! tuner's defaults (floor [`CONFIDENCE_FLOOR`], EMA alpha
//! [`TUNER_ALPHA`], `k_sigma` [`TUNER_K_SIGMA`], warmup [`TUNER_WARMUP`]
//! observations, variance floor [`TUNER_VAR_FLOOR`]). Any other value is
//! read as the initial state `{mean:` [`TUNER_MEAN`]`, var:`
//! [`TUNER_VAR_FLOOR`]`, n: 0}`, a fresh [`ConfidenceTuner::default`].
//!
//! # `<intuition:observe location="L" confidence="X"/>`
//!
//! [`ConfidenceTuner::observed`]. `location` (required) holds
//! the tuner state; `confidence` (required) is an expression of the turn's
//! reported confidence. A number (clamped to 0–100) is observed: the first
//! sets the mean, later ones move the mean and variance by the EMA
//! (variance never under the floor). `null` (also `undefined`) is not
//! observed. `location` receives the resulting state, canonical JSON.
//!
//! # `<intuition:escalate location="L" confidence="X" tuner="T"/>`
//!
//! [`ConfidenceTuner::should_escalate`]. `location` (required)
//! receives `true` when `confidence` (required, an expression) is a number
//! below the threshold of the tuner state `tuner` (required, an
//! expression), else `false`; a `null` (also `undefined`) confidence never
//! escalates ("no score: cannot self-tune; do not force escalate"). The
//! threshold is the floor during the warmup, else `max(floor / 2, mean -
//! k_sigma * sqrt(max(var, variance floor)))`, on the 0–100 scale.
//!
//! # Failures
//!
//! A malformed element, a confidence that is neither a number nor `null`,
//! an expression that fails, or a failed assignment is an `execution` error
//! with a static detail (SCXML `error.execution`); nothing is assigned then.
//!
//! # Bounds
//!
//! The state is three numbers; the data model bounds the values it hands
//! this component.
//!
//! # Telemetry
//!
//! Each entry point (`initialize`, `execute`, `shutdown`) records one
//! observation through scxml.rs's `telemetry` WIT facade: its closed
//! operation, its closed outcome (`ok`, `rejected`, `failed`), and one
//! closed signal event. No attribute carries a confidence, a state, or a
//! location, so the cardinality is the product of the three closed enums.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

use scxml_event_schema::canonical_json;
use scxml_types::DataValue as Data;
use serde_json::Value as Json;

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "intuition";

/// The exact namespace URI documents bind: tagless, so it names the
/// repository's latest release (decision D19); the manifest and schema name
/// it too.
pub const NAMESPACE_URI: &str = "https://github.com/stateforward/floppy/namespaces/intuition";

/// The tuner's absolute confidence floor.
pub const CONFIDENCE_FLOOR: f64 = 35.0;

/// The tuner's EMA weight.
pub const TUNER_ALPHA: f64 = 0.08;

/// The tuner's standard deviations under the mean.
pub const TUNER_K_SIGMA: f64 = 1.0;

/// Observations before the tuner leaves its floor.
pub const TUNER_WARMUP: u64 = 8;

/// The tuner's variance floor and initial variance.
pub const TUNER_VAR_FLOOR: f64 = 100.0;

/// The tuner's initial mean.
pub const TUNER_MEAN: f64 = 70.0;

pub use floppy_guest::element::{Attribute, Content, Executed, Failure, Host};
use floppy_guest::element::{assign_to, attribute_values, evaluate, no_content};

/// Executes one element with the given local name, attributes, and
/// flattened content.
///
/// # Errors
///
/// [`Failure::Execution`] for every malformed element, value, or
/// assignment; [`Failure::Host`] for a non-recoverable host failure.
pub fn execute<H: Host>(
    local_name: &str,
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    match local_name {
        "observe" => observe(attributes, content, host),
        "escalate" => escalate(attributes, content, host),
        _ => Ok(Executed::NotHandled),
    }
}

/// The tuner state `expression` holds: a [`ConfidenceTuner`] when it is
/// one, else the initial state.
fn read_tuner<H: Host>(
    host: &mut H,
    expression: &str,
    failed: &'static str,
) -> Result<ConfidenceTuner, Failure<H::Error>> {
    Ok(match evaluate(host, expression, failed)? {
        Data::JsonObject(text) => serde_json::from_str(&text)
            .ok()
            .as_ref()
            .and_then(ConfidenceTuner::from_json),
        _ => None,
    }
    .unwrap_or_default())
}

/// A reported confidence: `Some` number, or `None` for `null` (and
/// `undefined`); anything else is `malformed`.
fn read_confidence<H: Host>(
    host: &mut H,
    expression: &str,
    failed: &'static str,
    malformed: &'static str,
) -> Result<Option<f64>, Failure<H::Error>> {
    match evaluate(host, expression, failed)? {
        Data::Number(value) => Ok(Some(value.get())),
        Data::Null | Data::Undefined => Ok(None),
        _ => Err(Failure::Execution(malformed)),
    }
}

/// Executes `<intuition:observe>`.
fn observe<H: Host>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, confidence] = attribute_values(
        attributes,
        ["location", "confidence"],
        [
            "intuition:observe takes only the location and confidence attributes",
            "intuition:observe repeats an attribute",
            "intuition:observe has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution("intuition:observe requires a location"))?;
    let confidence = confidence.ok_or(Failure::Execution(
        "intuition:observe requires a confidence",
    ))?;
    no_content(content, "intuition:observe holds no content")?;
    let tuner = read_tuner(
        host,
        location,
        "intuition:observe location did not evaluate",
    )?;
    let confidence = read_confidence(
        host,
        confidence,
        "intuition:observe confidence did not evaluate",
        "intuition:observe confidence is not a number or null",
    )?;
    let tuner = confidence.map_or(tuner, |confidence| tuner.observed(confidence));
    assign_to(
        host,
        location,
        &Data::JsonObject(canonical_json(&tuner.to_json())),
        "intuition:observe could not assign the tuner state",
    )?;
    Ok(Executed::Handled)
}

/// Executes `<intuition:escalate>`.
fn escalate<H: Host>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, confidence, tuner] = attribute_values(
        attributes,
        ["location", "confidence", "tuner"],
        [
            "intuition:escalate takes only the location, confidence and tuner attributes",
            "intuition:escalate repeats an attribute",
            "intuition:escalate has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution("intuition:escalate requires a location"))?;
    let confidence = confidence.ok_or(Failure::Execution(
        "intuition:escalate requires a confidence",
    ))?;
    let tuner = tuner.ok_or(Failure::Execution("intuition:escalate requires a tuner"))?;
    no_content(content, "intuition:escalate holds no content")?;
    let confidence = read_confidence(
        host,
        confidence,
        "intuition:escalate confidence did not evaluate",
        "intuition:escalate confidence is not a number or null",
    )?;
    let tuner = read_tuner(host, tuner, "intuition:escalate tuner did not evaluate")?;
    assign_to(
        host,
        location,
        &Data::Bool(tuner.should_escalate(confidence)),
        "intuition:escalate could not assign the decision",
    )?;
    Ok(Executed::Handled)
}

/// The confidence tuner's state on the 0–100 scale: see the crate
/// documentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConfidenceTuner {
    /// The running mean of the observed confidences.
    pub mean: f64,
    /// Their running variance.
    pub var: f64,
    /// How many were observed.
    pub n: u64,
}

impl Default for ConfidenceTuner {
    fn default() -> Self {
        Self {
            mean: TUNER_MEAN,
            var: TUNER_VAR_FLOOR,
            n: 0,
        }
    }
}

impl ConfidenceTuner {
    /// The state `{mean, var, n}` holds, when it is one: numbers, `n` a
    /// non-negative integer.
    #[must_use]
    pub fn from_json(value: &Json) -> Option<Self> {
        let n = value.get("n")?;
        let n = n.as_u64().or_else(|| {
            n.as_f64()
                .filter(|n| n.fract() == 0.0 && *n >= 0.0 && *n <= 9_007_199_254_740_991.0)
                .map(|n| {
                    #[allow(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "a non-negative integer within the safe-integer range"
                    )]
                    let n = n as u64;
                    n
                })
        })?;
        Some(Self {
            mean: value.get("mean")?.as_f64()?,
            var: value.get("var")?.as_f64()?,
            n,
        })
    }

    /// The state after observing `confidence`
    /// (clamped to 0–100).
    #[must_use]
    pub fn observed(self, confidence: f64) -> Self {
        let conf = confidence.clamp(0.0, 100.0);
        if self.n == 0 {
            return Self {
                mean: conf,
                var: TUNER_VAR_FLOOR,
                n: 1,
            };
        }
        let delta = conf - self.mean;
        Self {
            mean: self.mean + TUNER_ALPHA * delta,
            var: TUNER_VAR_FLOOR.max((1.0 - TUNER_ALPHA) * self.var + TUNER_ALPHA * delta * delta),
            n: self.n.saturating_add(1),
        }
    }

    /// The escalation threshold on the 0–100 scale.
    #[must_use]
    pub fn threshold(self) -> f64 {
        if self.n < TUNER_WARMUP {
            return CONFIDENCE_FLOOR;
        }
        let std = self.var.max(TUNER_VAR_FLOOR).sqrt();
        (0.5 * CONFIDENCE_FLOOR).max(self.mean - TUNER_K_SIGMA * std)
    }

    /// Whether `confidence` is below the threshold; none never escalates.
    #[must_use]
    pub fn should_escalate(self, confidence: Option<f64>) -> bool {
        confidence.is_some_and(|confidence| confidence < self.threshold())
    }

    /// The state as `{mean, n, var}`; an integral mean or variance is
    /// written as an integer.
    #[must_use]
    pub fn to_json(self) -> Json {
        let number = |value: f64| {
            if value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0 {
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "an integral value within the safe-integer range"
                )]
                let value = value as i64;
                Json::from(value)
            } else {
                serde_json::Number::from_f64(value).map_or(Json::Null, Json::Number)
            }
        };
        serde_json::json!({"mean": number(self.mean), "var": number(self.var), "n": self.n})
    }
}

floppy_guest::extension_guest!();

#[cfg(test)]
mod tests;
