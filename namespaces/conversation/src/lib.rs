//! The `conversation` extension namespace: `<conversation:commit>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! One conversation message as the conversation commits it (mosfet
//! `_message_for_turn`, `_model_safe_content`, `_stable_identity_basis`,
//! `_stable_context_ref`, `MessageProvenance`), shared by the inbound path
//! (`abilities/conversation/conversation.scx`) and the outbound record
//! (`abilities/communication/communication.scx`). Documents bind [`NAMESPACE_URI`]
//! (conventionally with the prefix `conversation`):
//!
//! ```xml
//! <conversation:commit location="message" keylocation="participants"
//!                      direction="inbound" event="parent.event"
//!                      message="input" sequence="history.length"/>
//! ```
//!
//! The element builds the message and writes it to its locations; it never
//! reads or writes the history. The document appends the message to its
//! own history and transcripts with short `<assign>`s, so the history is
//! never copied through the extension boundary (whose values are bounded on
//! wasm builds) and grows as the conversation does.
//!
//! # `<conversation:commit location="L" keylocation="K" direction="D" event="E" message="M" sequence="S" provenance="P"/>`
//!
//! `direction` is the literal `inbound` or `outbound` (not an expression).
//! `event` (required) evaluates to the committing event's name, a non-empty
//! string. `message` (required) evaluates to an object with `source_ids`
//! and `target_ids` (arrays of identities: a string, or an array of
//! numbers), `content_type` (a non-empty string) and optionally `content`
//! (any value; absent is `null`); its other members are not committed.
//! `sequence` (required) evaluates to the message's position in the
//! history, a non-negative integer. `provenance` (optional) evaluates to an
//! object (`null` or `undefined` is none) whose `id`, `session_ref` and
//! `turn_ref` the provenance takes.
//!
//! `location` (required) receives the committed message,
//! `{sequence, direction, source_ids, target_ids, content, content_type,
//! provenance: {event, id, source, target, session_ref, turn_ref}}`:
//!
//! * `content`: inbound, `null` when the content type, lower-cased, starts
//!   with `audio/` (media never enters the model-facing history), else the
//!   content as given; outbound, the content as given.
//! * `provenance.id`: the provenance's `id` when it is a string, else
//!   `null`; `source` and `target` are `null`; `turn_ref` is the
//!   provenance's `turn_ref`, else `null`.
//! * `provenance.session_ref`: inbound, the relationship reference
//!   `relationship-<sorted source ids><sorted target ids>`; outbound, the
//!   provenance's `session_ref`, else `null`.
//!
//! The sorted ids of a list are its identities in the order of their
//! ECMAScript `JSON.stringify` text (UTF-16 code units), written as one
//! `JSON.stringify` array text. `keylocation` (optional) receives the
//! sorted source ids, the key of the message's transcript (one transcript
//! per participant set); it is assigned before `location`.
//!
//! # Failures
//!
//! A malformed element (a missing, empty, unknown or repeated attribute,
//! another `direction`, or any content but white space), an expression that
//! fails or evaluates to a value of the wrong kind, or a failed assignment
//! is an `execution` error with a static detail (SCXML `error.execution`).
//! A host failure that is not recoverable is passed through unchanged.
//!
//! # Telemetry
//!
//! Each entry point (`initialize`, `execute`, `shutdown`) records one
//! observation through scxml.rs's `telemetry` WIT facade: its closed
//! operation, its closed outcome (`ok`, `rejected`, `failed`), and one closed
//! signal event. No attribute carries an identity, content, an event name,
//! or a location, so the cardinality is the product of the three closed
//! enums.

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
pub const NAMESPACE_NAME: &str = "conversation";

/// The exact namespace URI documents bind: tagless, so it names the
/// repository's latest release (decision D19); the manifest and schema name
/// it too.
pub const NAMESPACE_URI: &str = "https://github.com/stateforward/floppy/namespaces/conversation";

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
pub trait ConversationHost {
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

/// The direction of a committed message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    /// From a participant.
    Inbound,
    /// From the bot.
    Outbound,
}

impl Direction {
    fn name(self) -> &'static str {
        match self {
            Self::Inbound => "inbound",
            Self::Outbound => "outbound",
        }
    }
}

/// Executes one element with the given local name, attributes, and
/// flattened content.
///
/// # Errors
///
/// [`Failure::Execution`] for every malformed element, value, or
/// assignment; [`Failure::Host`] for a non-recoverable host failure.
pub fn execute<H: ConversationHost>(
    local_name: &str,
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    match local_name {
        "commit" => commit(attributes, content, host),
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
fn evaluate<H: ConversationHost>(
    host: &mut H,
    expression: &str,
    failed: &'static str,
) -> Result<Data, Failure<H::Error>> {
    host.read(expression)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(failed))
}

/// Assigns `value` to `location`, else `failed`.
fn assign_to<H: ConversationHost>(
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

/// Executes `<conversation:commit>`.
fn commit<H: ConversationHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [
        location,
        key_location,
        direction,
        event,
        message,
        sequence,
        provenance,
    ] = attribute_values(
        attributes,
        [
            "location",
            "keylocation",
            "direction",
            "event",
            "message",
            "sequence",
            "provenance",
        ],
        "conversation:commit takes only the location, keylocation, direction, event, message, sequence and provenance attributes",
        "conversation:commit repeats an attribute",
        "conversation:commit has an empty attribute",
    )?;
    let location = location.ok_or(Failure::Execution(
        "conversation:commit requires a location",
    ))?;
    let direction = match direction {
        Some("inbound") => Direction::Inbound,
        Some("outbound") => Direction::Outbound,
        Some(_) => {
            return Err(Failure::Execution(
                "conversation:commit direction is not inbound or outbound",
            ));
        }
        None => {
            return Err(Failure::Execution(
                "conversation:commit requires a direction",
            ));
        }
    };
    let event = event.ok_or(Failure::Execution("conversation:commit requires an event"))?;
    let message = message.ok_or(Failure::Execution("conversation:commit requires a message"))?;
    let sequence = sequence.ok_or(Failure::Execution(
        "conversation:commit requires a sequence",
    ))?;
    no_content(content, "conversation:commit holds no content")?;
    let event = match evaluate(host, event, "conversation:commit event did not evaluate")? {
        Data::String(event) if !event.is_empty() => event,
        _ => {
            return Err(Failure::Execution(
                "conversation:commit event is not a non-empty string",
            ));
        }
    };
    let Some(Json::Object(message)) = json_of(&evaluate(
        host,
        message,
        "conversation:commit message did not evaluate",
    )?) else {
        return Err(Failure::Execution(
            "conversation:commit message is not an object",
        ));
    };
    let sequence = sequence_of(host, sequence)?;
    let provenance = provenance_of(host, provenance)?;
    let (committed, key) = committed_message(direction, &event, &message, sequence, &provenance)?;
    if let Some(key_location) = key_location {
        assign_to(
            host,
            key_location,
            &Data::String(key),
            "conversation:commit could not assign keylocation",
        )?;
    }
    assign_to(
        host,
        location,
        &Data::JsonObject(Json::Object(committed).to_string()),
        "conversation:commit could not assign the message",
    )?;
    Ok(Executed::Handled)
}

/// Evaluates `sequence` to a non-negative integer.
fn sequence_of<H: ConversationHost>(
    host: &mut H,
    sequence: &str,
) -> Result<serde_json::Number, Failure<H::Error>> {
    match evaluate(
        host,
        sequence,
        "conversation:commit sequence did not evaluate",
    )? {
        Data::Number(value) => match number(value.get()) {
            Json::Number(sequence) if sequence.as_u64().is_some() => Ok(sequence),
            _ => Err(Failure::Execution(
                "conversation:commit sequence is not a non-negative integer",
            )),
        },
        _ => Err(Failure::Execution(
            "conversation:commit sequence is not a non-negative integer",
        )),
    }
}

/// Evaluates `provenance`, when present, to an object (`null` or
/// `undefined` is none).
fn provenance_of<H: ConversationHost>(
    host: &mut H,
    provenance: Option<&str>,
) -> Result<Map<String, Json>, Failure<H::Error>> {
    let Some(provenance) = provenance else {
        return Ok(Map::new());
    };
    match evaluate(
        host,
        provenance,
        "conversation:commit provenance did not evaluate",
    )? {
        Data::Null | Data::Undefined | Data::Absent => Ok(Map::new()),
        value => match json_of(&value) {
            Some(Json::Object(provenance)) => Ok(provenance),
            _ => Err(Failure::Execution(
                "conversation:commit provenance is not an object",
            )),
        },
    }
}

/// The committed message and its transcript key (see the crate
/// documentation, `<conversation:commit>`).
///
/// # Errors
///
/// [`Failure::Execution`] when the message's identities or content type are
/// malformed.
pub fn committed_message<E>(
    direction: Direction,
    event: &str,
    message: &Map<String, Json>,
    sequence: serde_json::Number,
    provenance: &Map<String, Json>,
) -> Result<(Map<String, Json>, String), Failure<E>> {
    let identities = |name: &str, malformed: &'static str| match message.get(name) {
        Some(Json::Array(ids)) if ids.iter().all(is_identity) => Ok(ids.clone()),
        _ => Err(Failure::Execution(malformed)),
    };
    let source_ids = identities(
        "source_ids",
        "conversation:commit message source_ids is not an array of identities",
    )?;
    let target_ids = identities(
        "target_ids",
        "conversation:commit message target_ids is not an array of identities",
    )?;
    let content_type = match message.get("content_type") {
        Some(Json::String(content_type)) if !content_type.is_empty() => content_type.clone(),
        _ => {
            return Err(Failure::Execution(
                "conversation:commit message content_type is not a non-empty string",
            ));
        }
    };
    let content = match direction {
        Direction::Inbound if content_type.to_lowercase().starts_with("audio/") => Json::Null,
        _ => message.get("content").cloned().unwrap_or(Json::Null),
    };
    let key = sorted_ids(&source_ids);
    let session_ref = match direction {
        Direction::Inbound => {
            Json::String(format!("relationship-{key}{}", sorted_ids(&target_ids)))
        }
        Direction::Outbound => provenance.get("session_ref").cloned().unwrap_or(Json::Null),
    };
    let id = match provenance.get("id") {
        Some(Json::String(id)) => Json::String(id.clone()),
        _ => Json::Null,
    };
    let mut committed_provenance = Map::new();
    committed_provenance.insert("event".into(), Json::String(event.into()));
    committed_provenance.insert("id".into(), id);
    committed_provenance.insert("source".into(), Json::Null);
    committed_provenance.insert("target".into(), Json::Null);
    committed_provenance.insert("session_ref".into(), session_ref);
    committed_provenance.insert(
        "turn_ref".into(),
        provenance.get("turn_ref").cloned().unwrap_or(Json::Null),
    );
    let mut committed = Map::new();
    committed.insert("sequence".into(), Json::Number(sequence));
    committed.insert("direction".into(), Json::String(direction.name().into()));
    committed.insert("source_ids".into(), Json::Array(source_ids));
    committed.insert("target_ids".into(), Json::Array(target_ids));
    committed.insert("content".into(), content);
    committed.insert("content_type".into(), Json::String(content_type));
    committed.insert("provenance".into(), Json::Object(committed_provenance));
    Ok((committed, key))
}

/// An identity: a string, or an array of numbers (an embedding).
fn is_identity(value: &Json) -> bool {
    match value {
        Json::String(_) => true,
        Json::Array(values) => values.iter().all(Json::is_number),
        _ => false,
    }
}

/// The identities sorted by their ECMAScript `JSON.stringify` text (UTF-16
/// code-unit order), as one `JSON.stringify` array text (mosfet
/// `_stable_identity_basis`: one stable basis per identity set).
#[must_use]
pub fn sorted_ids(ids: &[Json]) -> String {
    let mut texts: Vec<String> = ids.iter().map(js_json).collect();
    texts.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    format!("[{}]", texts.join(","))
}

/// ECMAScript `JSON.stringify` of a JSON value (object members in their
/// map's order).
fn js_json(value: &Json) -> String {
    match value {
        Json::Number(number) => number
            .as_f64()
            .map_or_else(|| number.to_string(), js_number),
        Json::Array(values) => format!(
            "[{}]",
            values.iter().map(js_json).collect::<Vec<_>>().join(",")
        ),
        Json::Object(members) => format!(
            "{{{}}}",
            members
                .iter()
                .map(|(key, value)| format!("{}:{}", Json::from(key.as_str()), js_json(value)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        // Strings escape as ECMAScript does (quote, backslash, and control
        // characters, the latter as lower-case \u00xx); null and booleans
        // are their literals.
        Json::Null | Json::Bool(_) | Json::String(_) => value.to_string(),
    }
}

/// ECMAScript `Number.prototype.toString` of a finite number (ECMA-262
/// `Number::toString`: the shortest round-trip digits, positional between
/// 1e-7 and 1e21, else exponential).
#[must_use]
pub fn js_number(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    if !value.is_finite() {
        return "null".into();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    // Rust's `{:e}` writes the shortest round-trip digits: `d[.ddd]e<exp>`.
    let scientific = format!("{:e}", value.abs());
    let Some((mantissa, exponent)) = scientific.split_once('e') else {
        return scientific;
    };
    let Ok(exponent) = exponent.parse::<i64>() else {
        return scientific;
    };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let k = i64::try_from(digits.len()).unwrap_or(i64::MAX);
    let n = exponent + 1;
    let body = if k <= n && n <= 21 {
        format!(
            "{digits}{}",
            "0".repeat(usize::try_from(n - k).unwrap_or(0))
        )
    } else if 0 < n && n <= 21 {
        let (whole, fraction) = digits.split_at(usize::try_from(n).unwrap_or(0));
        format!("{whole}.{fraction}")
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat(usize::try_from(-n).unwrap_or(0)))
    } else {
        let (first, rest) = digits.split_at(1);
        let fraction = if rest.is_empty() {
            String::new()
        } else {
            format!(".{rest}")
        };
        let exponent = n - 1;
        let exponent_sign = if exponent < 0 { "-" } else { "+" };
        format!("{first}{fraction}e{exponent_sign}{}", exponent.abs())
    };
    format!("{sign}{body}")
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

    use super::{Attribute, Content, ConversationHost, Data, Executed, Failure};

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

    impl ConversationHost for Host {
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
