//! The `conversation` extension namespace: `<conversation:commit>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! One conversation message as the conversation commits it (mosfet
//! `_message_for_turn`, `_model_safe_content`, `_stable_identity_basis`,
//! `_stable_context_ref`, `relationship_context_ref`, `MessageProvenance`),
//! shared by the inbound path (`abilities/conversation/conversation.scx`)
//! and the outbound record (`abilities/speaking/speaking.scx`). Documents bind [`NAMESPACE_URI`]
//! (conventionally with the prefix `conversation`):
//!
//! ```xml
//! <conversation:commit location="message" keylocation="context_ref"
//!                      direction="inbound" event="parent.event"
//!                      message="input" sequence="history.length"/>
//! ```
//!
//! The element builds the message and writes it to its locations; it never
//! reads or writes the history. The document appends the message to its
//! own history and relationships with short `<assign>`s, so the history is
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
//! * `provenance.id`, `source` and `target`: the provenance's, each when
//!   it is a string, else `null` (mosfet's provenance of an append is its
//!   event's id, source and target); `turn_ref` is the provenance's
//!   `turn_ref`, else `null`.
//! * `provenance.session_ref`: the provenance's `session_ref` when it is a
//!   string; else, inbound, the message's own relationship reference
//!   `relationship-<context_ref>` (mosfet `_new_relationship_ref`), and
//!   outbound `null`.
//!
//! The relationship context reference (`context_ref`) of a message is
//! mosfet's `relationship_context_ref` of its source and target ids
//! ([`relationship_context_ref`]): the canonical JSON text
//! `{"identity_groups":[<group>,<group>]}`, the same in either direction.
//! `keylocation` (optional) receives it, the key the conversation's memory
//! is kept under and the reference a new relationship takes; it is
//! assigned before `location`.
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

/// The committed message and its relationship context reference (see the
/// crate documentation, `<conversation:commit>`).
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
    let context_ref = relationship_context_ref(&source_ids, &target_ids);
    let string = |name: &str| match provenance.get(name) {
        Some(Json::String(text)) => Json::String(text.clone()),
        _ => Json::Null,
    };
    let session_ref = match (direction, string("session_ref")) {
        (Direction::Inbound, Json::Null) => Json::String(format!("relationship-{context_ref}")),
        (_, session_ref) => session_ref,
    };
    let mut committed_provenance = Map::new();
    committed_provenance.insert("event".into(), Json::String(event.into()));
    committed_provenance.insert("id".into(), string("id"));
    committed_provenance.insert("source".into(), string("source"));
    committed_provenance.insert("target".into(), string("target"));
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
    Ok((committed, context_ref))
}

/// An identity: a string, or an array of numbers (an embedding).
fn is_identity(value: &Json) -> bool {
    match value {
        Json::String(_) => true,
        Json::Array(values) => values.iter().all(Json::is_number),
        _ => false,
    }
}

/// One identity as mosfet's relationship key holds it: a name, or an
/// embedding.
#[derive(Clone, Debug, PartialEq)]
enum Identity {
    Name(String),
    Embedding(Vec<f64>),
}

impl Identity {
    /// The identity of a JSON identity (a string, or an array of numbers).
    fn of(value: &Json) -> Option<Self> {
        match value {
            Json::String(name) => Some(Self::Name(name.clone())),
            Json::Array(values) => values
                .iter()
                .map(Json::as_f64)
                .collect::<Option<Vec<_>>>()
                .map(Self::Embedding),
            _ => None,
        }
    }

    /// mosfet `identity_sort_key`: names before embeddings, a name by its
    /// text, an embedding by its compact Python JSON text.
    fn sort_key(&self) -> (u8, String) {
        match self {
            Self::Name(name) => (0, name.clone()),
            Self::Embedding(components) => (1, python_vector(components)),
        }
    }

    /// The identity as Python's `json.dumps(..., ensure_ascii=True)` writes
    /// it.
    fn json(&self) -> String {
        match self {
            Self::Name(name) => ascii_json_string(name),
            Self::Embedding(components) => python_vector(components),
        }
    }
}

/// The relationship context reference of a source and a target identity
/// set: mosfet `relationship_context_ref` of `_stable_identity_basis` of
/// each (`_stable_context_ref`), the canonical JSON text
/// `{"identity_groups":[<group>,<group>]}`. Each embedding is normalized to
/// its unit vector twice, as that composition does (an embedding without
/// one, empty or zero, is kept as given: mosfet refuses it at its input);
/// each group drops repeated identities (a set) and is sorted by
/// [`Identity::sort_key`]; the two groups are ordered by their keys, so the
/// reference is the same in either direction. Numbers are written as
/// Python's `repr` writes a float and text as `json.dumps` with
/// `ensure_ascii` (every character outside printable ASCII escaped).
#[must_use]
pub fn relationship_context_ref(source_ids: &[Json], target_ids: &[Json]) -> String {
    let group = |ids: &[Json]| {
        let mut identities: Vec<((u8, String), Identity)> = ids
            .iter()
            .filter_map(Identity::of)
            .map(|identity| match identity {
                Identity::Embedding(components) => {
                    Identity::Embedding(unit_vector(&unit_vector(&components)))
                }
                name @ Identity::Name(_) => name,
            })
            .map(|identity| (identity.sort_key(), identity))
            .collect();
        identities.sort_by(|left, right| left.0.cmp(&right.0));
        identities.dedup_by(|left, right| left.0 == right.0);
        identities
    };
    let mut groups = [group(source_ids), group(target_ids)];
    groups.sort_by(|left, right| {
        left.iter()
            .map(|(key, _)| key)
            .cmp(right.iter().map(|(key, _)| key))
    });
    let groups: Vec<String> = groups
        .iter()
        .map(|group| {
            let identities: Vec<String> =
                group.iter().map(|(_, identity)| identity.json()).collect();
            format!("[{}]", identities.join(","))
        })
        .collect();
    format!("{{\"identity_groups\":[{}]}}", groups.join(","))
}

/// mosfet `normalize_embedding`: the vector over its L2 norm (the squares
/// summed exactly, as `math.fsum`; scaled by the largest magnitude first
/// when they overflow), each zero component a positive zero; an empty or
/// zero vector is returned as given.
fn unit_vector(components: &[f64]) -> Vec<f64> {
    let components: Vec<f64> = components
        .iter()
        .map(|component| if *component == 0.0 { 0.0 } else { *component })
        .collect();
    let squared = fsum(components.iter().map(|component| component * component));
    let (components, norm) = if squared.is_finite() {
        (components, squared.sqrt())
    } else {
        let scale = components
            .iter()
            .fold(0.0_f64, |largest, component| largest.max(component.abs()));
        let scaled: Vec<f64> = components
            .iter()
            .map(|component| component / scale)
            .collect();
        let norm = fsum(scaled.iter().map(|component| component * component)).sqrt();
        (scaled, norm)
    };
    if components.is_empty() || norm == 0.0 || !norm.is_finite() {
        return components;
    }
    components
        .iter()
        .map(|component| component / norm)
        .collect()
}

/// Python's `math.fsum`: the correctly rounded sum (Shewchuk's partials,
/// rounded half to even); a non-finite partial makes the sum non-finite.
fn fsum(values: impl Iterator<Item = f64>) -> f64 {
    let mut partials: Vec<f64> = Vec::new();
    for value in values {
        let mut x = value;
        let mut kept = 0;
        for index in 0..partials.len() {
            let mut y = partials[index];
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let high = x + y;
            let low = y - (high - x);
            if low != 0.0 {
                partials[kept] = low;
                kept += 1;
            }
            x = high;
        }
        partials.truncate(kept);
        partials.push(x);
        if !x.is_finite() {
            return x;
        }
    }
    let Some(mut high) = partials.pop() else {
        return 0.0;
    };
    let mut low = 0.0;
    while let Some(y) = partials.pop() {
        let x = high;
        high = x + y;
        low = y - (high - x);
        if low != 0.0 {
            break;
        }
    }
    if let Some(next) = partials.last()
        && ((low < 0.0 && *next < 0.0) || (low > 0.0 && *next > 0.0))
    {
        let y = low * 2.0;
        let x = high + y;
        if y == x - high {
            high = x;
        }
    }
    high
}

/// An embedding as Python's compact `json.dumps` writes a list of floats.
fn python_vector(components: &[f64]) -> String {
    let texts: Vec<String> = components.iter().copied().map(python_float).collect();
    format!("[{}]", texts.join(","))
}

/// Python's `repr` of a finite float (`float_repr_style` `short`): the
/// shortest round-trip digits, positional with at least one fraction digit
/// when the decimal exponent is from -4 to 15, else `d[.ddd]e±XX`.
#[must_use]
pub fn python_float(value: f64) -> String {
    if !value.is_finite() {
        return "NaN".into();
    }
    let sign = if value.is_sign_negative() { "-" } else { "" };
    if value == 0.0 {
        return format!("{sign}0.0");
    }
    // Rust's `{:e}` writes the shortest round-trip digits: `d[.ddd]e<exp>`.
    let scientific = format!("{:e}", value.abs());
    let Some((mantissa, exponent)) = scientific.split_once('e') else {
        return scientific;
    };
    let Ok(exponent) = exponent.parse::<i64>() else {
        return scientific;
    };
    if !(-4..16).contains(&exponent) {
        let exponent_sign = if exponent < 0 { "-" } else { "+" };
        return format!("{sign}{mantissa}e{exponent_sign}{:02}", exponent.abs());
    }
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let whole_digits = usize::try_from(exponent + 1).unwrap_or(0);
    let body = if exponent < 0 {
        let zeros = usize::try_from(-exponent - 1).unwrap_or(0);
        format!("0.{}{digits}", "0".repeat(zeros))
    } else if digits.len() <= whole_digits {
        format!("{digits}{}.0", "0".repeat(whole_digits - digits.len()))
    } else {
        let (whole, fraction) = digits.split_at(whole_digits);
        format!("{whole}.{fraction}")
    };
    format!("{sign}{body}")
}

/// A string as Python's `json.dumps(..., ensure_ascii=True)` writes it:
/// quote, backslash and the control characters `\b \f \n \r \t` escaped
/// short, every other character outside `' '..='~'` as lower-case `\uxxxx`
/// (UTF-16 surrogate pairs beyond the BMP).
fn ascii_json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(character),
            _ => {
                let mut units = [0_u16; 2];
                for unit in character.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
    out
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
