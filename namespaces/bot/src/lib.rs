//! The `bot` extension namespace: `<bot:stimulus>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! A bot body's per-turn bookkeeping that is synchronous and quick: the
//! stimulus envelope the body hands its cognition (`bot.ability.cognition.input`
//! `stimulus`). The turn's attention bias is the `cognition` namespace's
//! (`<cognition:attention_bias_for_turn>`), and the identity ability's text
//! name recognition the `identity` namespace's (`<identity:hears_name>`).
//! Documents bind [`NAMESPACE_URI`] (conventionally with the
//! prefix `bot`):
//!
//! ```xml
//! <bot:stimulus location="stimulus" event="_event.name" data="_event.data"
//!               source="_event.invokeid"/>
//! ```
//!
//! Every attribute but `location` is an expression of
//! the document's data model. Each element writes to its locations and
//! never sends: the document's own `<send>` delivers what it computes.
//!
//! # `<bot:stimulus location="L" event="E" data="X" source="S" target="T" id="I"/>`
//!
//! `location` (required) receives the stimulus envelope
//! (`abilities/schemas/bot.ability.cognition.input.json`, `stimulus`):
//! `{event, data, source, target?, id?}`, canonical JSON. `event`
//! (required) evaluates to a non-empty string, `data` (required) to the
//! event's data (any value but `undefined` and XML). `source` (required) is
//! the invoke id the event came from: a non-empty string, or `null` when it
//! evaluates to `null`, `undefined` or the empty string (an event from
//! outside the session). `target` and `id` (optional) are included when
//! present and their expression is not `undefined`; each is a string or
//! `null`.
//!
//! # Failures
//!
//! A malformed element (a missing, empty, unknown or repeated attribute,
//! or any content but white space), an expression that fails or evaluates
//! to a value of the wrong kind, or a failed assignment is an `execution`
//! error with a static detail (SCXML `error.execution`). A host failure
//! that is not recoverable is passed through unchanged.
//!
//! # Telemetry
//!
//! Each entry point (`initialize`, `execute`, `shutdown`) records one
//! observation through scxml.rs's `telemetry` WIT facade: its closed
//! operation, its closed outcome (`ok`, `rejected`, `failed`), and one closed
//! signal event. No attribute carries a device, an event name, or a
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
use serde_json::Value as Json;

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "bot";

/// The exact namespace URI documents bind: tagless, so it names the
/// repository's latest release (decision D19); the manifest and schema name
/// it too.
pub const NAMESPACE_URI: &str = "https://github.com/stateforward/floppy/namespaces/bot";

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
pub trait BotHost {
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
pub fn execute<H: BotHost>(
    local_name: &str,
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    match local_name {
        "stimulus" => stimulus(attributes, content, host),
        _ => Ok(Executed::NotHandled),
    }
}

/// The values of an element's unqualified attributes named in `names`, in
/// that order, each trimmed and non-empty; any other attribute is `refused`,
/// a repeated one `repeated`, an empty one `empty`.
fn attribute_values<'a, E, const N: usize>(
    attributes: &'a [Attribute],
    names: [&str; N],
    refused: &'static str,
    repeated: &'static str,
    empty: &'static str,
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
fn evaluate<H: BotHost>(
    host: &mut H,
    expression: &str,
    failed: &'static str,
) -> Result<Data, Failure<H::Error>> {
    host.read(expression)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(failed))
}

/// Assigns `value` to `location`, else `failed`.
fn assign_to<H: BotHost>(
    host: &mut H,
    location: &str,
    value: &Data,
    failed: &'static str,
) -> Result<(), Failure<H::Error>> {
    host.assign(location, value)
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
/// exactly (ECMAScript prints it without a fraction).
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

/// Executes `<bot:stimulus>`.
fn stimulus<H: BotHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, event, data, source, target, id] = attribute_values(
        attributes,
        ["location", "event", "data", "source", "target", "id"],
        "bot:stimulus takes only the location, event, data, source, target and id attributes",
        "bot:stimulus repeats an attribute",
        "bot:stimulus has an empty attribute",
    )?;
    let location = location.ok_or(Failure::Execution("bot:stimulus requires a location"))?;
    let event = event.ok_or(Failure::Execution("bot:stimulus requires an event"))?;
    let data = data.ok_or(Failure::Execution("bot:stimulus requires data"))?;
    let source = source.ok_or(Failure::Execution("bot:stimulus requires a source"))?;
    no_content(content, "bot:stimulus holds no content")?;
    let event = match evaluate(host, event, "bot:stimulus event did not evaluate")? {
        Data::String(event) if !event.is_empty() => event,
        _ => {
            return Err(Failure::Execution(
                "bot:stimulus event is not a non-empty string",
            ));
        }
    };
    let data = match evaluate(host, data, "bot:stimulus data did not evaluate")? {
        // Canonical JSON already: kept byte for byte.
        Data::JsonArray(text) | Data::JsonObject(text) => text,
        value => json_of(&value)
            .ok_or(Failure::Execution("bot:stimulus data is not a JSON value"))?
            .to_string(),
    };
    let source = match evaluate(host, source, "bot:stimulus source did not evaluate")? {
        Data::String(source) if !source.is_empty() => Json::String(source),
        Data::String(_) | Data::Null | Data::Undefined | Data::Absent => Json::Null,
        _ => {
            return Err(Failure::Execution(
                "bot:stimulus source is not a string or null",
            ));
        }
    };
    let mut optional = |expression: Option<&str>,
                        failed: &'static str,
                        malformed: &'static str|
     -> Result<Option<Json>, Failure<H::Error>> {
        let Some(expression) = expression else {
            return Ok(None);
        };
        match evaluate(host, expression, failed)? {
            Data::Undefined | Data::Absent => Ok(None),
            Data::Null => Ok(Some(Json::Null)),
            Data::String(text) => Ok(Some(Json::String(text))),
            _ => Err(Failure::Execution(malformed)),
        }
    };
    let id = optional(
        id,
        "bot:stimulus id did not evaluate",
        "bot:stimulus id is not a string or null",
    )?;
    let target = optional(
        target,
        "bot:stimulus target did not evaluate",
        "bot:stimulus target is not a string or null",
    )?;
    assign_to(
        host,
        location,
        &Data::JsonObject(stimulus_json(
            &event,
            &data,
            &source,
            id.as_ref(),
            target.as_ref(),
        )),
        "bot:stimulus could not assign the stimulus",
    )?;
    Ok(Executed::Handled)
}

/// The canonical JSON of one stimulus envelope, members sorted by key;
/// `data` is JSON text, kept byte for byte.
#[must_use]
pub fn stimulus_json(
    event: &str,
    data: &str,
    source: &Json,
    id: Option<&Json>,
    target: Option<&Json>,
) -> String {
    let id = id.map_or(String::new(), |id| format!(",\"id\":{id}"));
    let target = target.map_or(String::new(), |target| format!(",\"target\":{target}"));
    format!(
        "{{\"data\":{data},\"event\":{}{id},\"source\":{source}{target}}}",
        Json::from(event)
    )
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

    use super::{Attribute, BotHost, Content, Data, Executed, Failure};

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

    impl BotHost for Host {
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
