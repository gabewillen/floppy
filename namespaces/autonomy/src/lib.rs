//! The `autonomy` extension namespace: `<autonomy:behavior_input_payload>`
//! as a `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! The original's autonomy module (`abilities/cognition/autonomy.py`): the
//! input a learned behavior runs with (`behavior_input_payload`), the data
//! of the turn's stimulus with the turn's focus added. Autonomy and the
//! reflection (which tells the builder what a behavior will run with)
//! build it alike. Documents bind [`NAMESPACE_URI`] (conventionally with
//! the prefix `autonomy`):
//!
//! ```xml
//! <autonomy:behavior_input_payload location="behavior_input" stimulus="stimulus"
//!     focus="input.focus" focus_candidates="input.focus_candidates"/>
//! ```
//!
//! Every attribute but `location` is an expression of the document's data
//! model. The element writes to its location and never sends.
//!
//! # `<autonomy:behavior_input_payload location="L" stimulus="S" focus="F" focus_candidates="C"/>`
//!
//! The original's `behavior_input_payload` of a cognition input.
//! `stimulus` (required) evaluates to the turn's stimulus envelope (an
//! object; its `data` is the stimulus data), `focus` (required) to the
//! turn's focus and `focus_candidates` (required) to its focus candidates.
//! `location` (required) receives the stimulus data's members when the
//! data is an object (not an array), else `{event: data}` (a missing data
//! is left out), with `focus` and `focus_candidates` set to the values
//! given, whatever they are (one that evaluates to `undefined` is left
//! out, as ECMAScript's `JSON` leaves it out).
//!
//! # Failures
//!
//! A malformed element (a missing, empty, unknown or repeated attribute,
//! or any content but white space), an expression that fails or evaluates
//! to a value of the wrong kind, or a failed assignment is an `execution`
//! error with a static detail (SCXML `error.execution`); nothing is
//! assigned then. A host failure that is not recoverable is passed through
//! unchanged.
//!
//! # Bounds
//!
//! The data model bounds the values it hands this component.
//!
//! # Telemetry
//!
//! Each entry point (`initialize`, `execute`, `shutdown`) records one
//! observation through scxml.rs's `telemetry` WIT facade: its closed
//! operation, its closed outcome (`ok`, `rejected`, `failed`), and one
//! closed signal event. No attribute carries a stimulus, a focus or a
//! location, so the cardinality is the product of the three closed enums.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

use scxml_types::DataValue as Data;
use serde_json::{Map, Value as Json};

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "autonomy";

/// The exact namespace URI documents bind: tagless, so it names the
/// repository's latest release (decision D19); the manifest and schema name
/// it too.
pub const NAMESPACE_URI: &str = "https://github.com/stateforward/floppy/namespaces/autonomy";

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
pub trait AutonomyHost {
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
pub fn execute<H: AutonomyHost>(
    local_name: &str,
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    match local_name {
        "behavior_input_payload" => payload_element(attributes, content, host),
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

/// Evaluates `expression` to JSON, else `failed` (it does not evaluate) or
/// `malformed` (it evaluates to no JSON value).
fn evaluate<H: AutonomyHost>(
    host: &mut H,
    expression: &str,
    failed: &'static str,
    malformed: &'static str,
) -> Result<Json, Failure<H::Error>> {
    let value = host
        .read(expression)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(failed))?;
    json_of(&value).ok_or(Failure::Execution(malformed))
}

/// Evaluates `expression` to JSON, `None` for `undefined` (a missing
/// member), else `failed` (it does not evaluate) or `malformed` (XML or a
/// JSON text that does not parse).
fn evaluate_optional<H: AutonomyHost>(
    host: &mut H,
    expression: &str,
    failed: &'static str,
    malformed: &'static str,
) -> Result<Option<Json>, Failure<H::Error>> {
    let value = host
        .read(expression)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(failed))?;
    match value {
        Data::Absent | Data::Undefined => Ok(None),
        value => json_of(&value)
            .map(Some)
            .ok_or(Failure::Execution(malformed)),
    }
}

/// Assigns `value` to `location`, else `failed`.
fn assign_to<H: AutonomyHost>(
    host: &mut H,
    location: &str,
    value: &Json,
    failed: &'static str,
) -> Result<(), Failure<H::Error>> {
    let value = data_of(value).ok_or(Failure::Execution(failed))?;
    host.assign(location, &value)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(failed))
}

/// A data value as JSON; `None` for `undefined`, an absent value, XML, or
/// a JSON text that does not parse.
fn json_of(value: &Data) -> Option<Json> {
    match value {
        Data::Null => Some(Json::Null),
        Data::Bool(value) => Some(Json::Bool(*value)),
        Data::Number(value) => Some(number(value.get())),
        Data::String(text) => Some(Json::String(text.clone())),
        Data::JsonArray(text) | Data::JsonObject(text) => serde_json::from_str(text).ok(),
        Data::Absent | Data::Undefined | Data::Xml(_) => None,
    }
}

/// A finite number as JSON: an integer when it is one that `f64` holds
/// exactly (ECMAScript prints it without a fraction), `null` when it is
/// not finite (ECMAScript's `JSON.stringify` of `NaN`).
fn number(value: f64) -> Json {
    const EXACT: f64 = 9_007_199_254_740_992.0;
    if value.fract() == 0.0 && value.abs() < EXACT {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "an integral value under 2^53 in magnitude converts exactly"
        )]
        return Json::from(value as i64);
    }
    serde_json::Number::from_f64(value).map_or(Json::Null, Json::Number)
}

/// The data value of a JSON value (canonical JSON text for arrays and
/// objects: `serde_json` sorts object members).
fn data_of(value: &Json) -> Option<Data> {
    Some(match value {
        Json::Null => Data::Null,
        Json::Bool(value) => Data::Bool(*value),
        Json::Number(value) => Data::Number(scxml_types::FiniteNumber::new(value.as_f64()?).ok()?),
        Json::String(text) => Data::String(text.clone()),
        Json::Array(_) => Data::JsonArray(value.to_string()),
        Json::Object(_) => Data::JsonObject(value.to_string()),
    })
}

/// Executes `<autonomy:behavior_input_payload>`.
fn payload_element<H: AutonomyHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, stimulus, focus, focus_candidates] = attribute_values(
        attributes,
        ["location", "stimulus", "focus", "focus_candidates"],
        [
            "autonomy:behavior_input_payload takes only the location, stimulus, focus and focus_candidates attributes",
            "autonomy:behavior_input_payload repeats an attribute",
            "autonomy:behavior_input_payload has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution(
        "autonomy:behavior_input_payload requires a location",
    ))?;
    let stimulus = stimulus.ok_or(Failure::Execution(
        "autonomy:behavior_input_payload requires a stimulus",
    ))?;
    let focus = focus.ok_or(Failure::Execution(
        "autonomy:behavior_input_payload requires a focus",
    ))?;
    let focus_candidates = focus_candidates.ok_or(Failure::Execution(
        "autonomy:behavior_input_payload requires focus_candidates",
    ))?;
    no_content(content, "autonomy:behavior_input_payload holds no content")?;
    let Json::Object(stimulus) = evaluate(
        host,
        stimulus,
        "autonomy:behavior_input_payload stimulus did not evaluate",
        "autonomy:behavior_input_payload stimulus is not an object",
    )?
    else {
        return Err(Failure::Execution(
            "autonomy:behavior_input_payload stimulus is not an object",
        ));
    };
    let focus = evaluate_optional(
        host,
        focus,
        "autonomy:behavior_input_payload focus did not evaluate",
        "autonomy:behavior_input_payload focus is not data",
    )?;
    let focus_candidates = evaluate_optional(
        host,
        focus_candidates,
        "autonomy:behavior_input_payload focus_candidates did not evaluate",
        "autonomy:behavior_input_payload focus_candidates is not data",
    )?;
    let payload = behavior_input_payload(stimulus.get("data"), focus, focus_candidates);
    assign_to(
        host,
        location,
        &Json::Object(payload),
        "autonomy:behavior_input_payload could not assign the payload",
    )?;
    Ok(Executed::Handled)
}

/// The original's `behavior_input_payload` (see the crate documentation):
/// the stimulus data's members, or `{event: data}`, with the focus set (a
/// focus that is `None`, ECMAScript's `undefined`, is left out).
#[must_use]
pub fn behavior_input_payload(
    data: Option<&Json>,
    focus: Option<Json>,
    focus_candidates: Option<Json>,
) -> Map<String, Json> {
    let mut payload = match data {
        Some(Json::Object(members)) => members.clone(),
        Some(other) => {
            let mut payload = Map::new();
            payload.insert("event".into(), other.clone());
            payload
        }
        None => Map::new(),
    };
    for (name, value) in [("focus", focus), ("focus_candidates", focus_candidates)] {
        match value {
            Some(value) => payload.insert(name.into(), value),
            None => payload.remove(name),
        };
    }
    payload
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

    use super::{Attribute, AutonomyHost, Content, Data, Executed, Failure};

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

    impl AutonomyHost for Host {
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
