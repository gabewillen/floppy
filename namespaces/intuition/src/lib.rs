//! The `intuition` extension namespace: `<intuition:observe>` and
//! `<intuition:escalate>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! The original's `abilities/cognition/intuition.py` owns a
//! `ConfidenceTuner`: a self-tuning confidence baseline on the 0–100 scale
//! that decides when the fast stage escalates a turn to reasoning. Its
//! `_tune_confidence` observes the turn's confidence first and then asks
//! `should_escalate` against the updated threshold. Both are synchronous
//! and quick, so they are executable content here, named after the class's
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
//! original's defaults (floor [`CONFIDENCE_FLOOR`], EMA alpha
//! [`TUNER_ALPHA`], `k_sigma` [`TUNER_K_SIGMA`], warmup [`TUNER_WARMUP`]
//! observations, variance floor [`TUNER_VAR_FLOOR`]). Any other value is
//! read as the initial state `{mean:` [`TUNER_MEAN`]`, var:`
//! [`TUNER_VAR_FLOOR`]`, n: 0}`, the original's fresh `ConfidenceTuner()`.
//!
//! # `<intuition:observe location="L" confidence="X"/>`
//!
//! The original's `ConfidenceTuner.observe`. `location` (required) holds
//! the tuner state; `confidence` (required) is an expression of the turn's
//! reported confidence. A number (clamped to 0–100) is observed: the first
//! sets the mean, later ones move the mean and variance by the EMA
//! (variance never under the floor). `null` (also `undefined`) is not
//! observed, as the original observes only a confidence that is not
//! `None`. `location` receives the resulting state, canonical JSON.
//!
//! # `<intuition:escalate location="L" confidence="X" tuner="T"/>`
//!
//! The original's `ConfidenceTuner.should_escalate`. `location` (required)
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

/// The tuner's absolute confidence floor (the original's
/// `_DEFAULT_CONFIDENCE_FLOOR`).
pub const CONFIDENCE_FLOOR: f64 = 35.0;

/// The tuner's EMA weight (the original's `_DEFAULT_TUNER_ALPHA`).
pub const TUNER_ALPHA: f64 = 0.08;

/// The tuner's standard deviations under the mean (the original's
/// `_DEFAULT_TUNER_K_SIGMA`).
pub const TUNER_K_SIGMA: f64 = 1.0;

/// Observations before the tuner leaves its floor (the original's
/// `_DEFAULT_TUNER_WARMUP`).
pub const TUNER_WARMUP: u64 = 8;

/// The tuner's variance floor and initial variance (the original's
/// `_DEFAULT_TUNER_VAR_FLOOR`).
pub const TUNER_VAR_FLOOR: f64 = 100.0;

/// The tuner's initial mean (the original's `_DEFAULT_TUNER_MEAN`).
pub const TUNER_MEAN: f64 = 70.0;

/// One attribute of an element.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attribute {
    /// Its namespace URI, when it has one.
    pub namespace_uri: Option<String>,
    /// Local name.
    pub local_name: String,
    /// Value exactly as authored.
    pub value: String,
}

/// One item of an element's flattened content, in document order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Content {
    /// Character data.
    Text(String),
    /// The start tag of a nested element.
    Start,
    /// The end tag of the innermost open nested element.
    End,
}

/// What `execute` did with the element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Executed {
    /// The element was executed.
    Handled,
    /// The local name is not one of this namespace's elements.
    NotHandled,
}

/// Why `execute` failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Failure<E> {
    /// SCXML `error.execution`, with a static detail.
    Execution(&'static str),
    /// A non-recoverable host failure, passed through unchanged.
    Host(E),
}

/// The host capabilities one `execute` call uses.
pub trait IntuitionHost {
    /// A non-recoverable host failure.
    type Error;

    /// Evaluates one value expression in the current data model;
    /// `Ok(Err(()))` is a recoverable evaluation failure.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn read(&mut self, expression: &str) -> Result<Result<Data, ()>, Self::Error>;

    /// Assigns one value to one data-model location; `Ok(Err(()))` is a
    /// recoverable assignment failure.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn assign(&mut self, location: &str, value: &Data) -> Result<Result<(), ()>, Self::Error>;
}

/// Executes one element with the given local name, attributes, and
/// flattened content.
///
/// # Errors
///
/// [`Failure::Execution`] for every malformed element, value, or
/// assignment; [`Failure::Host`] for a non-recoverable host failure.
pub fn execute<H: IntuitionHost>(
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

/// The values of an element's unqualified attributes named in `names`, in
/// that order, each trimmed and non-empty; any other attribute is `refused`,
/// a repeated one `repeated`, an empty one `empty`.
fn attribute_values<'a, E, const N: usize>(
    attributes: &'a [Attribute],
    names: [&str; N],
    [refused, repeated, empty]: [&'static str; 3],
) -> Result<[Option<&'a str>; N], Failure<E>> {
    let mut values = [None; N];
    for attribute in attributes {
        let index = match attribute.namespace_uri {
            None => names.iter().position(|name| *name == attribute.local_name),
            Some(_) => None,
        }
        .ok_or(Failure::Execution(refused))?;
        let value = attribute.value.trim();
        if value.is_empty() {
            return Err(Failure::Execution(empty));
        }
        if values[index].replace(value).is_some() {
            return Err(Failure::Execution(repeated));
        }
    }
    Ok(values)
}

/// Refuses any content but white space with `detail`.
fn no_content<E>(content: &[Content], detail: &'static str) -> Result<(), Failure<E>> {
    if content
        .iter()
        .any(|item| !matches!(item, Content::Text(text) if text.trim().is_empty()))
    {
        return Err(Failure::Execution(detail));
    }
    Ok(())
}

/// Evaluates `expression`, else `failed`.
fn evaluate<H: IntuitionHost>(
    host: &mut H,
    expression: &str,
    failed: &'static str,
) -> Result<Data, Failure<H::Error>> {
    host.read(expression)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(failed))
}

/// Assigns `value` to `location`, else `failed`.
fn assign_to<H: IntuitionHost>(
    host: &mut H,
    location: &str,
    value: &Data,
    failed: &'static str,
) -> Result<(), Failure<H::Error>> {
    host.assign(location, value)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(failed))
}

/// The tuner state `expression` holds: a [`ConfidenceTuner`] when it is
/// one, else the initial state.
fn read_tuner<H: IntuitionHost>(
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
fn read_confidence<H: IntuitionHost>(
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
fn observe<H: IntuitionHost>(
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
fn escalate<H: IntuitionHost>(
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

/// The original's `ConfidenceTuner` state on the 0–100 scale: see the crate
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

    /// The original's `observe`: the state after observing `confidence`
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

    /// The original's `threshold`: the escalation threshold on the 0–100
    /// scale.
    #[must_use]
    pub fn threshold(self) -> f64 {
        if self.n < TUNER_WARMUP {
            return CONFIDENCE_FLOOR;
        }
        let std = self.var.max(TUNER_VAR_FLOOR).sqrt();
        (0.5 * CONFIDENCE_FLOOR).max(self.mean - TUNER_K_SIGMA * std)
    }

    /// The original's `should_escalate`: a confidence below the threshold;
    /// none never escalates.
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

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    wit_bindgen::generate!({
        world: "executable-content-extension",
        path: "../../scxml.rs/crates/scxml-wit/wit/scxml",
    });

    use std::cell::Cell;

    use exports::stateforward::scxml::executable_content_extension_api::Guest;
    use stateforward::scxml::data_model_host;
    use stateforward::scxml::telemetry::{self, Operation, Outcome, SignalEvent};
    use stateforward::scxml::types::{
        ComponentError, DataValue, ErrorCode, ErrorVersion, ExecutableContentExtension,
        ExtensionContent, ExtensionOutcome, TraceContext,
    };

    use super::{Attribute, Content, Data, Executed, Failure, IntuitionHost};

    /// The lifecycle of this instance.
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Lifecycle {
        Created,
        Live,
        ShutDown,
    }

    thread_local! {
        static STATE: Cell<Lifecycle> = const { Cell::new(Lifecycle::Created) };
    }

    struct Component;

    /// The host capabilities of one `execute` call.
    struct Host;

    /// Evaluation and assignment failures (and a session with no data
    /// model) are document errors.
    fn recoverable(code: ErrorCode) -> bool {
        matches!(
            code,
            ErrorCode::InvalidInput
                | ErrorCode::Execution
                | ErrorCode::Unavailable
                | ErrorCode::PayloadTooLarge
        )
    }

    impl IntuitionHost for Host {
        type Error = ComponentError;

        fn read(&mut self, expression: &str) -> Result<Result<Data, ()>, ComponentError> {
            match data_model_host::evaluate(expression) {
                Ok(value) => Ok(match value {
                    DataValue::Absent => Ok(Data::Absent),
                    DataValue::Undefined => Ok(Data::Undefined),
                    DataValue::Null => Ok(Data::Null),
                    DataValue::Boolean(value) => Ok(Data::Bool(value)),
                    DataValue::Number(value) => scxml_types::FiniteNumber::new(value)
                        .map(Data::Number)
                        .map_err(|_| ()),
                    DataValue::Text(text) => Ok(Data::String(text)),
                    DataValue::JsonArray(text) => Ok(Data::JsonArray(text)),
                    DataValue::JsonObject(text) => Ok(Data::JsonObject(text)),
                    DataValue::Xml(text) => Ok(Data::Xml(text)),
                }),
                Err(error) if recoverable(error.code) => Ok(Err(())),
                Err(error) => Err(error),
            }
        }

        fn assign(
            &mut self,
            location: &str,
            value: &Data,
        ) -> Result<Result<(), ()>, ComponentError> {
            let value = match value {
                Data::Null => DataValue::Null,
                Data::Bool(value) => DataValue::Boolean(*value),
                Data::Number(value) => DataValue::Number(value.get()),
                Data::String(text) => DataValue::Text(text.clone()),
                Data::JsonArray(text) => DataValue::JsonArray(text.clone()),
                Data::JsonObject(text) => DataValue::JsonObject(text.clone()),
                Data::Absent | Data::Undefined | Data::Xml(_) => return Ok(Err(())),
            };
            match data_model_host::assign(location, &value) {
                Ok(()) => Ok(Ok(())),
                Err(error) if recoverable(error.code) => Ok(Err(())),
                Err(error) => Err(error),
            }
        }
    }

    fn content(items: Vec<ExtensionContent>) -> Vec<Content> {
        items
            .into_iter()
            .map(|item| match item {
                ExtensionContent::Text(text) => Content::Text(text),
                ExtensionContent::StartElement(_) => Content::Start,
                ExtensionContent::EndElement(_) => Content::End,
            })
            .collect()
    }

    impl Guest for Component {
        fn namespace_uri() -> String {
            super::NAMESPACE_URI.into()
        }

        fn initialize() -> Result<(), ComponentError> {
            observed(Operation::Initialize, None, || {
                if STATE.get() != Lifecycle::Created {
                    return Err(error(ErrorCode::Protocol, "initialize was already called"));
                }
                STATE.set(Lifecycle::Live);
                Ok(())
            })
        }

        fn execute(
            element: ExecutableContentExtension,
            context: Option<TraceContext>,
        ) -> Result<ExtensionOutcome, ComponentError> {
            observed(Operation::Execute, context.as_ref(), || {
                if element.name.namespace_uri.as_deref() != Some(super::NAMESPACE_URI) {
                    return Err(error(
                        ErrorCode::Protocol,
                        "element namespace does not match extension component identity",
                    ));
                }
                if STATE.get() != Lifecycle::Live {
                    return Err(error(
                        ErrorCode::Protocol,
                        "execute outside the initialized lifecycle",
                    ));
                }
                let attributes: Vec<Attribute> = element
                    .attributes
                    .into_iter()
                    .map(|attribute| Attribute {
                        namespace_uri: attribute.name.namespace_uri,
                        local_name: attribute.name.local_name,
                        value: attribute.value,
                    })
                    .collect();
                let flattened = content(element.content);
                match super::execute(&element.name.local_name, &attributes, &flattened, &mut Host) {
                    Ok(Executed::Handled) => Ok(ExtensionOutcome::Handled),
                    Ok(Executed::NotHandled) => Ok(ExtensionOutcome::NotHandled),
                    Err(Failure::Execution(detail)) => Err(error(ErrorCode::Execution, detail)),
                    Err(Failure::Host(host)) => Err(host),
                }
            })
        }

        fn shutdown() -> Result<(), ComponentError> {
            observed(Operation::Shutdown, None, || {
                STATE.set(Lifecycle::ShutDown);
                Ok(())
            })
        }
    }

    /// Records one closed-label observation per entry point: the operation,
    /// its outcome, and one signal event (no attribute carries input).
    fn observed<T>(
        operation: Operation,
        context: Option<&TraceContext>,
        body: impl FnOnce() -> Result<T, ComponentError>,
    ) -> Result<T, ComponentError> {
        let result = body();
        let outcome = match &result {
            Ok(_) => Outcome::Ok,
            Err(error) if error.code == ErrorCode::Execution => Outcome::Failed,
            Err(_) => Outcome::Rejected,
        };
        telemetry::observe(operation, outcome, None, context);
        telemetry::event(
            operation,
            if result.is_ok() {
                SignalEvent::TransformationCompleted
            } else {
                SignalEvent::InputRejected
            },
            context,
        );
        result
    }

    fn error(code: ErrorCode, detail: &'static str) -> ComponentError {
        ComponentError {
            version: ErrorVersion::V1,
            code,
            detail: (detail.len() <= 160).then(|| detail.into()),
        }
    }

    export!(Component);
}

#[cfg(test)]
mod tests;
