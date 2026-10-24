//! The `decision` extension namespace: `<decision:choice>` and
//! `<decision:pin>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! Executable content is synchronous and quick, so this namespace holds only
//! the half of a decision that is: reading the choices (the choices of a
//! `TypeSafe` `Choice` question) and grounding one property of them. What
//! becomes of the choices (their model-facing schemas, the turn's offered events, the
//! dispatch tool, the selection confidence) is the `processing` namespace
//! (`namespaces/processing`). Asking a backend to
//! choose is slow I/O and belongs to the `decision` invoked service
//! (`services/decision`), which takes the choices this element writes as its
//! `choices` parameter. Documents bind [`NAMESPACE_URI`] (conventionally with
//! the prefix `decision`):
//!
//! ```xml
//! <onentry><decision:choice location="choices"/></onentry>
//! <invoke type="https://github.com/stateforward/floppy/services/decision" ...>
//!   <param name="choices" expr="choices"/>
//!   <param name="state" expr="..."/>
//! </invoke>
//! ```
//!
//! # `<decision:choice location="L"/>`
//!
//! `location` (required) receives the canonical choices: a JSON array of
//! objects with `event` (an event name), `schema` (the transition's resolved
//! `event:schema`), `description` (the schema's `description`, else its
//! `title`, first line; omitted when it has neither), and `effort` (only with
//! the `effort` attribute), canonical JSON with
//! members sorted by key, in the session snapshot's walk order. `events`
//! (optional) filters it: see "The filter". `effort` (optional) declares
//! what the choices cost: see "The effort". Every other attribute
//! (unqualified or namespaced), any child element, and any non-white-space
//! text is refused.
//!
//! * **The choices.** The transitions the session snapshot lists for the
//!   current configuration (`session-snapshot-host`, `transitions` section,
//!   in its walk order) that carry an `event:schema`: each of their event
//!   descriptors that names an event (`*` is skipped; `e.*` names `e`),
//!   unless a transition listed earlier in the same active atomic state's
//!   chain has a descriptor that matches the same event (that one would be
//!   selected for it); each event is offered once. The walk lists a shared
//!   ancestor's transitions once, in the first chain that reaches it, and a
//!   leaf without eventful transitions of its own opens no chain, so in
//!   those two cases the choices can leave out an event SCXML would also
//!   select, never offer one it would not. `event:schemaexpr` transitions
//!   are not offered (a snapshot carries the expression, not the schema).
//! * **Guards.** A transition with a `cond` is offered only when its
//!   condition, evaluated in the current data model (with the current
//!   `_event`, not the event being offered), is exactly `true`; one whose
//!   condition is exactly `false` is skipped and takes no event from a later
//!   transition; one whose condition fails or is not a boolean is not offered
//!   but still takes its events from later transitions. So an event among the
//!   choices is never one only a disabled transition would take, but a guard
//!   that reads `_event` of the offered event is judged against the current
//!   event (author such guards without `event:schema`, or expect them not
//!   to be offered). Conditions run once per element, so they must be free
//!   of side effects, as SCXML conditions should be anyway.
//! * **The filter.** `events="D1 D2 ..."` is a white-space separated list
//!   of SCXML event descriptors (SCXML 3.12.1: `*`, or an event name with an
//!   optional trailing `.*`). The choices keep only the events one of them
//!   matches (a token prefix match: `bot.ability` keeps
//!   `bot.ability.cognition.ignore`), in choices order. The filter applies
//!   after selection order is worked out, so a transition it leaves out
//!   still takes its events from later transitions, as SCXML would select
//!   it. An `events` that lists no descriptor, or one that is not an event
//!   descriptor, is refused. Without `events` the choices are unfiltered. A
//!   document uses it to offer a model only the events meant for it (tool
//!   events) and not, say, its own input or completion events.
//!
//! * **The effort** (decision D206). `effort="EXPR"` is an expression of
//!   the data model that evaluates to a whole number from 0 to
//!   [`MAX_EFFORT`], and every choice the element writes carries it as
//!   `effort`: how heavy an action it is to take, which the cognition holds
//!   against each tier's effort ceiling (`<cognition:within_ceiling>`). A
//!   choice without `effort` weighs 0, within every ceiling. An `effort`
//!   that fails to evaluate or is not such a number is `error.execution`.
//!   The offering document declares it because it is what the action costs
//!   in this bot, not part of the event's data contract; an expression lets
//!   an invoker's `<param>` set it.
//!
//! The choices' schemas are the contracts of the events the decision service
//! may return: it validates the chosen event's data against them before it
//! returns it (the document declares its events, not the service package).
//!
//! # `<decision:pin location="L" event="E" property="P" values="X"/>`
//!
//! Pins one property of one event's schema in the choice array at
//! `location` (as the floppy bot (`bots/floppy`) grounds `bot.focus_device`'s `device` in the
//! turn's focus candidates): `values`, an expression that evaluates to a JSON
//! array, becomes the `enum` of `schema.properties.<P>` of every entry whose
//! event is `E`; when the array is empty those entries are removed instead
//! (nothing could fill them). The result is canonical JSON. Every attribute
//! is required. An entry for `E` whose schema has no object
//! `properties.<P>` is `error.execution`.
//!
//! # Failures
//!
//! A malformed element, a missing or malformed snapshot, a schema that does
//! not compile or match its digest, or a failed assignment is an `execution`
//! error with a static detail (SCXML `error.execution`); nothing is assigned
//! then.
//!
//! # Bounds
//!
//! Event names are bounded by [`MAX_EVENT_NAME_BYTES`] on wasm builds only
//! ([`scxml_types::wasm_bound`]); schemas are bounded by
//! `scxml-event-schema`, and the snapshot by the host.
//!
//! # Telemetry
//!
//! Each entry point (`initialize`, `execute`, `shutdown`) records one
//! observation through scxml.rs's `telemetry` WIT facade: its closed
//! operation, its closed outcome (`ok`, `rejected`, `failed`), and one closed
//! signal event. No attribute carries an event name, a schema, or a
//! location, so the cardinality is the product of the three closed enums.
//! The host adds one `session-snapshot` span per snapshot read with only its
//! closed outcome.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

use scxml_event_schema::{CompiledSchema, canonical_json, sha256_hex};
use scxml_types::{DataValue as Data, exceeds_wasm_bound, wasm_bound};
use serde_json::{Map, Value as Json};

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "decision";

/// The exact namespace URI documents bind: tagless, so it names the
/// repository's latest release (decision D19); the manifest and schema name
/// it too.
pub const NAMESPACE_URI: &str = "https://github.com/stateforward/floppy/namespaces/decision";

/// Upper bound on a choice event name's bytes (wasm builds only).
pub const MAX_EVENT_NAME_BYTES: usize = wasm_bound(256);

/// The greatest effort a choice may declare (decision D206): an effort is a
/// small whole number, compared only with tier ceilings.
pub const MAX_EFFORT: u32 = 1_000;

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

/// A section of the session snapshot a read can omit
/// (`session-snapshot-host.section`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Section {
    /// SCXML 5.10 system variables.
    System,
    /// User data-model variables.
    DataModel,
    /// The internal and external event queues.
    Queues,
    /// Pending delayed sends.
    PendingSends,
    /// Transitions an event could trigger.
    Transitions,
}

/// The snapshot read for the choices: only its transitions are used.
pub const CHOICES_SNAPSHOT_EXCLUDE: [Section; 4] = [
    Section::System,
    Section::DataModel,
    Section::Queues,
    Section::PendingSends,
];

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

/// What a guard evaluated to: `Some(b)` for a boolean, `None` for a failed
/// or non-boolean evaluation.
pub type GuardValue = Option<bool>;

/// The host capabilities one `execute` call uses.
pub trait ChoiceHost {
    /// A non-recoverable host failure.
    type Error;

    /// Evaluates one transition's `cond` in the current data model.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn guard(&mut self, cond: &str) -> Result<GuardValue, Self::Error>;

    /// Evaluates one data-model location; `Ok(Err(()))` is a recoverable
    /// evaluation failure.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn read(&mut self, location: &str) -> Result<Result<Data, ()>, Self::Error>;

    /// Assigns one value to one data-model location; `Ok(Err(()))` is a
    /// recoverable assignment failure.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn assign(&mut self, location: &str, value: &Data) -> Result<Result<(), ()>, Self::Error>;

    /// The executing session's `scxml.session-snapshot/v2` document with
    /// `exclude` omitted; `Ok(Err(()))` when the host offers none.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn snapshot(&mut self, exclude: &[Section]) -> Result<Result<String, ()>, Self::Error>;
}

/// One event among the choices.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    /// The event name.
    pub event: String,
    /// The transition's resolved schema.
    pub schema: Json,
    /// What the event means, when its schema says.
    pub description: Option<String>,
    /// How heavy an action it is (decision D206), when the offering document
    /// declares it.
    pub effort: Option<u32>,
}

/// The canonical choices document of `entries`: a JSON array of
/// `{"description"?, "effort"?, "event", "schema"}` objects in order,
/// canonical JSON.
#[must_use]
pub fn choices_json(entries: &[Choice]) -> String {
    canonical_json(&Json::Array(
        entries
            .iter()
            .map(|entry| {
                let mut object = Map::new();
                object.insert("event".into(), entry.event.clone().into());
                object.insert("schema".into(), entry.schema.clone());
                if let Some(description) = &entry.description {
                    object.insert("description".into(), description.clone().into());
                }
                if let Some(effort) = entry.effort {
                    object.insert("effort".into(), effort.into());
                }
                Json::Object(object)
            })
            .collect(),
    ))
}

/// Executes one element with the given local name, attributes, and
/// flattened content.
///
/// # Errors
///
/// [`Failure::Execution`] for every malformed element, snapshot, or
/// assignment; [`Failure::Host`] for a non-recoverable host failure.
pub fn execute<H: ChoiceHost>(
    local_name: &str,
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    match local_name {
        "choice" => choice(attributes, content, host),
        "pin" => pin(attributes, content, host),
        _ => Ok(Executed::NotHandled),
    }
}

/// The values of an element's unqualified attributes named in `names`, in
/// that order; any other attribute is `refused`, a repeated one `repeated`.
fn attribute_values<'a, E, const N: usize>(
    attributes: &'a [Attribute],
    names: [&str; N],
    refused: &'static str,
    repeated: &'static str,
) -> Result<[Option<&'a str>; N], Failure<E>> {
    let mut values = [None; N];
    for attribute in attributes {
        let index = match attribute.namespace_uri {
            None => names.iter().position(|name| *name == attribute.local_name),
            Some(_) => None,
        }
        .ok_or(Failure::Execution(refused))?;
        if values[index].replace(attribute.value.as_str()).is_some() {
            return Err(Failure::Execution(repeated));
        }
    }
    Ok(values)
}

/// A required, non-empty (trimmed) attribute value, else `missing`.
fn required<'a, E>(value: Option<&'a str>, missing: &'static str) -> Result<&'a str, Failure<E>> {
    optional(value, missing)?.ok_or(Failure::Execution(missing))
}

/// An optional attribute value that must not be empty (trimmed) when
/// present, else `empty`.
fn optional<'a, E>(
    value: Option<&'a str>,
    empty: &'static str,
) -> Result<Option<&'a str>, Failure<E>> {
    value
        .map(|value| {
            Some(value.trim())
                .filter(|value| !value.is_empty())
                .ok_or(Failure::Execution(empty))
        })
        .transpose()
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

/// Evaluates `expression` as JSON (an array or an object), else `malformed`.
fn read_json<H: ChoiceHost>(
    host: &mut H,
    expression: &str,
    malformed: &'static str,
) -> Result<Json, Failure<H::Error>> {
    match host
        .read(expression)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(malformed))?
    {
        Data::JsonArray(text) | Data::JsonObject(text) => {
            serde_json::from_str(&text).map_err(|_| Failure::Execution(malformed))
        }
        _ => Err(Failure::Execution(malformed)),
    }
}

/// Assigns `value` to `location`, else `failed`.
fn assign_to<H: ChoiceHost>(
    host: &mut H,
    location: &str,
    value: &Data,
    failed: &'static str,
) -> Result<(), Failure<H::Error>> {
    host.assign(location, value)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(failed))
}

/// Executes `<decision:pin>`.
fn pin<H: ChoiceHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, event, property, values] = attribute_values(
        attributes,
        ["location", "event", "property", "values"],
        "decision:pin takes only the location, event, property and values attributes",
        "decision:pin repeats an attribute",
    )?;
    let location = required(location, "decision:pin requires a non-empty location")?;
    let event = required(event, "decision:pin requires a non-empty event")?;
    let property = required(property, "decision:pin requires a non-empty property")?;
    let values = required(values, "decision:pin requires a non-empty values")?;
    no_content(content, "decision:pin holds no content")?;
    let choices = read_json(
        host,
        location,
        "decision:pin location does not hold choices",
    )?;
    let Json::Array(values) = read_json(host, values, "decision:pin values is not an array")?
    else {
        return Err(Failure::Execution("decision:pin values is not an array"));
    };
    let pinned = pin_choices(&choices, event, property, &values).map_err(Failure::Execution)?;
    assign_to(
        host,
        location,
        &Data::JsonArray(pinned),
        "decision:pin could not assign the choices",
    )?;
    Ok(Executed::Handled)
}

/// `choices` with every `event` entry's `schema.properties.<property>.enum`
/// set to `values`, or without the `event` entries when `values` is empty,
/// as canonical JSON: see the crate documentation (`<decision:pin>`).
///
/// # Errors
///
/// A static detail when `choices` is not an array of choice entries or an
/// `event` entry's schema has no object `properties.<property>`.
pub fn pin_choices(
    choices: &Json,
    event: &str,
    property: &str,
    values: &[Json],
) -> Result<String, &'static str> {
    let entries = checked_entries(choices, PIN_ENTRIES)?;
    let mut pinned = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.get("event").and_then(Json::as_str) != Some(event) {
            pinned.push(entry.clone());
            continue;
        }
        if values.is_empty() {
            continue;
        }
        let mut entry = entry.clone();
        let slot = entry
            .pointer_mut("/schema/properties")
            .and_then(Json::as_object_mut)
            .and_then(|properties| properties.get_mut(property))
            .and_then(Json::as_object_mut)
            .ok_or("decision:pin found no such property in the event's schema")?;
        slot.insert("enum".into(), Json::Array(values.to_vec()));
        pinned.push(entry);
    }
    Ok(canonical_json(&Json::Array(pinned)))
}

/// `<decision:pin>`'s choice-entry failures (see `checked_entries`).
const PIN_ENTRIES: [&str; 3] = [
    "decision:pin location does not hold choices",
    "decision:pin found a choice that is not an object",
    "decision:pin found a malformed choice",
];

/// The entries of `choices`, each checked to be a choice entry: an object
/// with a non-empty string `event`, a `schema`, optionally a string
/// `description`, a non-empty string `target` and an `effort` (a whole
/// number from 0 to [`MAX_EFFORT`]), and nothing else;
/// `details` are the failures: not an array, an entry not an object, a
/// malformed entry.
fn checked_entries<'a>(
    choices: &'a Json,
    [not_choices, not_object, malformed]: [&'static str; 3],
) -> Result<&'a [Json], &'static str> {
    let entries = choices.as_array().ok_or(not_choices)?;
    for entry in entries {
        let entry = entry.as_object().ok_or(not_object)?;
        let text = |key: &str| {
            entry.get(key).map(|value| {
                value
                    .as_str()
                    .filter(|text| key == "description" || !text.is_empty())
            })
        };
        if entry.keys().any(|key| {
            !["event", "schema", "description", "target", "effort"].contains(&key.as_str())
        }) || entry
            .get("effort")
            .is_some_and(|effort| effort_of(effort).is_none())
            || text("event").flatten().is_none()
            || !entry.contains_key("schema")
            || matches!(text("description"), Some(None))
            || matches!(text("target"), Some(None))
        {
            return Err(malformed);
        }
    }
    Ok(entries)
}

/// A declared effort: a whole number from 0 to [`MAX_EFFORT`], else `None`.
#[must_use]
pub fn effort_of(value: &Json) -> Option<u32> {
    value
        .as_u64()
        .and_then(|effort| u32::try_from(effort).ok())
        .filter(|effort| *effort <= MAX_EFFORT)
}

/// The effort `expression` evaluates to (see [`effort_of`]).
fn read_effort<H: ChoiceHost>(host: &mut H, expression: &str) -> Result<u32, Failure<H::Error>> {
    const MALFORMED: &str = "decision:choice effort is not a whole number from 0 to 1000";
    match host
        .read(expression)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution("decision:choice effort did not evaluate"))?
    {
        Data::Number(number) => {
            let number = number.get();
            if number.fract() != 0.0 || !(0.0..=f64::from(MAX_EFFORT)).contains(&number) {
                return Err(Failure::Execution(MALFORMED));
            }
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a whole number from 0 to MAX_EFFORT, checked above"
            )]
            Ok(number as u32)
        }
        _ => Err(Failure::Execution(MALFORMED)),
    }
}

/// Executes `<decision:choice>`.
fn choice<H: ChoiceHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let mut location = None;
    let mut events = None;
    let mut effort = None;
    for attribute in attributes {
        let slot = match (
            attribute.namespace_uri.as_deref(),
            attribute.local_name.as_str(),
        ) {
            (None, "location") => &mut location,
            (None, "events") => &mut events,
            (None, "effort") => &mut effort,
            _ => {
                return Err(Failure::Execution(
                    "decision:choice takes only the location, events and effort attributes",
                ));
            }
        };
        if slot.replace(attribute.value.as_str()).is_some() {
            return Err(Failure::Execution("decision:choice repeats an attribute"));
        }
    }
    let location = location
        .map(str::trim)
        .filter(|location| !location.is_empty())
        .ok_or(Failure::Execution(
            "decision:choice requires a non-empty location",
        ))?;
    let filter = events.map(event_filter).transpose()?;
    if content
        .iter()
        .any(|item| !matches!(item, Content::Text(text) if text.trim().is_empty()))
    {
        return Err(Failure::Execution("decision:choice holds no content"));
    }
    let effort = effort
        .map(|effort| {
            Some(effort.trim())
                .filter(|effort| !effort.is_empty())
                .ok_or(Failure::Execution("decision:choice has an empty effort"))
        })
        .transpose()?
        .map(|effort| read_effort(host, effort))
        .transpose()?;
    let document = host
        .snapshot(&CHOICES_SNAPSHOT_EXCLUDE)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution("the host offers no session snapshot"))?;
    let mut choices = snapshot_choices(&document, &mut |cond| host.guard(cond))?;
    if let Some(filter) = filter {
        choices.retain(|entry| {
            filter
                .iter()
                .any(|descriptor| matches(descriptor, &entry.event))
        });
    }
    for entry in &mut choices {
        entry.effort = effort;
    }
    host.assign(location, &Data::JsonArray(choices_json(&choices)))
        .map_err(Failure::Host)?
        .map_err(|()| {
            Failure::Execution("decision:choice could not assign the choices to location")
        })?;
    Ok(Executed::Handled)
}

/// The event descriptors of an `events` attribute: its white-space
/// separated tokens, each `*` or an event name with an optional trailing
/// `.*` (SCXML 3.12.1).
///
/// # Errors
///
/// [`Failure::Execution`] when the value lists no descriptor or one that
/// is not an event descriptor.
pub fn event_filter<E>(value: &str) -> Result<Vec<&str>, Failure<E>> {
    let descriptors: Vec<&str> = value
        .split(scxml_types::is_xml_space)
        .filter(|token| !token.is_empty())
        .collect();
    if descriptors.is_empty() {
        return Err(Failure::Execution(
            "decision:choice events names no event descriptor",
        ));
    }
    if descriptors.iter().any(|descriptor| {
        *descriptor != "*" && !valid_event_name(descriptor.strip_suffix(".*").unwrap_or(descriptor))
    }) {
        return Err(Failure::Execution(
            "decision:choice events holds a malformed event descriptor",
        ));
    }
    Ok(descriptors)
}

/// The first line of the schema's `description`, else of its `title`.
fn description_of(schema: &Json) -> Option<String> {
    ["description", "title"].iter().find_map(|key| {
        schema
            .get(key)
            .and_then(Json::as_str)
            .and_then(|text| text.lines().map(str::trim).find(|line| !line.is_empty()))
            .map(str::to_owned)
    })
}

/// Whether one event descriptor matches `event` (SCXML 3.12.1: a token
/// prefix match; `*` matches every event; a trailing `.*` is ignored).
fn matches(descriptor: &str, event: &str) -> bool {
    if descriptor == "*" {
        return true;
    }
    let descriptor = descriptor.strip_suffix(".*").unwrap_or(descriptor);
    event == descriptor
        || event
            .strip_prefix(descriptor)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// Whether `name` is an event name a candidate may carry: non-empty, no
/// white space, no `*`, no empty `.`-separated token, bounded.
fn valid_event_name(name: &str) -> bool {
    !name.is_empty()
        && !exceeds_wasm_bound(name.len(), MAX_EVENT_NAME_BYTES)
        && !name.contains('*')
        && !name.chars().any(scxml_types::is_xml_space)
        && name.split('.').all(|token| !token.is_empty())
}

/// What one transition's guard evaluated to (see [`snapshot_choices`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Guard {
    /// No `cond`, or a `cond` that evaluated to `true`.
    Enabled,
    /// A `cond` that evaluated to `false`.
    Disabled,
    /// A `cond` whose evaluation failed or was not a boolean.
    Unknown,
}

/// The choices of a `scxml.session-snapshot/v2` document: see the crate
/// documentation ("The choices"). `guard` evaluates one transition's `cond`
/// text: `Ok(Some(b))` for a boolean, `Ok(None)` for a failed or
/// non-boolean evaluation.
///
/// # Errors
///
/// [`Failure::Execution`] with a static detail when the document is
/// malformed or a schema does not compile or match its digest;
/// [`Failure::Host`] when `guard` fails non-recoverably.
pub fn snapshot_choices<E>(
    document: &str,
    guard: &mut dyn FnMut(&str) -> Result<Option<bool>, E>,
) -> Result<Vec<Choice>, Failure<E>> {
    let document: Json = serde_json::from_str(document).map_err(|_| malformed())?;
    // A document without its session object offers nothing, as one
    // without the transitions section does.
    let Some(root) = document.pointer("/snapshot/root") else {
        return Ok(Vec::new());
    };
    selection_walk(root, &mut |cond| {
        Ok(match guard(cond)? {
            Some(true) => Guard::Enabled,
            Some(false) => Guard::Disabled,
            None => Guard::Unknown,
        })
    })
}

/// A malformed snapshot's failure.
const fn malformed<E>() -> Failure<E> {
    Failure::Execution("the session snapshot is malformed")
}

/// The choices of one session object of a `scxml.session-snapshot/v2`
/// document (its `root`), by SCXML 3.13's selection walk: see the crate
/// documentation ("The choices"). `guard` judges one transition's `cond`
/// text; a transition without one is [`Guard::Enabled`]. A snapshot without
/// its transitions section offers nothing.
///
/// # Errors
///
/// [`Failure::Execution`] with a static detail when the session object is
/// malformed or a schema does not compile or match its digest;
/// [`Failure::Host`] when `guard` fails non-recoverably.
pub fn selection_walk<E>(
    root: &Json,
    guard: &mut dyn FnMut(&str) -> Result<Guard, E>,
) -> Result<Vec<Choice>, Failure<E>> {
    let Some(transitions) = root.get("transitions") else {
        return Ok(Vec::new());
    };
    let transitions = transitions.as_array().ok_or_else(malformed)?;
    let atomic: Vec<&str> = root
        .pointer("/configuration/atomic")
        .and_then(Json::as_array)
        .ok_or_else(malformed)?
        .iter()
        .map(|state| state.as_str().ok_or_else(malformed))
        .collect::<Result<_, _>>()?;
    // SCXML 3.13 selects per active atomic state: the first enabled
    // matching transition of that state's chain (the state, then its
    // ancestors). The walk lists each leaf's chain in turn, so a transition
    // only preempts later ones of the same chain: `earlier` restarts where
    // a listed transition's source is another active atomic state.
    let mut leaf: Option<&str> = None;
    let mut earlier: Vec<&str> = Vec::new();
    let mut choices: Vec<Choice> = Vec::new();
    for transition in transitions {
        let source = transition
            .get("source")
            .and_then(Json::as_str)
            .ok_or_else(malformed)?;
        if leaf != Some(source) && atomic.contains(&source) {
            leaf = Some(source);
            earlier.clear();
        }
        let descriptors: Vec<&str> = transition
            .get("events")
            .and_then(Json::as_array)
            .ok_or_else(malformed)?
            .iter()
            .map(|descriptor| descriptor.as_str().ok_or_else(malformed))
            .collect::<Result<_, _>>()?;
        let state = match (
            transition.get("cond"),
            transition.get("has_cond").and_then(Json::as_bool),
        ) {
            (None, Some(false) | None) => Guard::Enabled,
            (Some(Json::String(cond)), Some(true) | None) => guard(cond).map_err(Failure::Host)?,
            _ => return Err(malformed()),
        };
        if state == Guard::Disabled {
            continue;
        }
        if state == Guard::Enabled
            && let Some(event_schema) = transition.get("event_schema")
            && event_schema.get("kind").and_then(Json::as_str) == Some("schema")
        {
            let text = event_schema
                .get("schema")
                .and_then(Json::as_str)
                .ok_or_else(malformed)?;
            if event_schema.get("sha256").and_then(Json::as_str)
                != Some(sha256_hex(text.as_bytes()).as_str())
            {
                return Err(malformed());
            }
            let schema: Json = serde_json::from_str(text).map_err(|_| malformed())?;
            // Fail closed on a schema the decision service could not
            // compile, before it is offered.
            CompiledSchema::new(text)
                .map_err(|_| Failure::Execution("a transition's event schema does not compile"))?;
            for descriptor in &descriptors {
                if *descriptor == "*" {
                    continue;
                }
                let event = descriptor.strip_suffix(".*").unwrap_or(descriptor);
                if !valid_event_name(event)
                    || earlier.iter().any(|before| matches(before, event))
                    || choices.iter().any(|entry| entry.event == event)
                {
                    continue;
                }
                choices.push(Choice {
                    event: event.to_owned(),
                    description: description_of(&schema),
                    effort: None,
                    schema: schema.clone(),
                });
            }
        }
        earlier.extend(descriptors);
    }
    Ok(choices)
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
    use stateforward::scxml::telemetry::{self, Operation, Outcome, SignalEvent};
    use stateforward::scxml::types::{
        ComponentError, DataValue, ErrorCode, ErrorVersion, ExecutableContentExtension,
        ExtensionContent, ExtensionOutcome, TraceContext,
    };
    use stateforward::scxml::{data_model_host, session_snapshot_host};

    use super::{Attribute, ChoiceHost, Content, Data, Executed, Failure, GuardValue, Section};

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

    impl ChoiceHost for Host {
        type Error = ComponentError;

        fn guard(&mut self, cond: &str) -> Result<GuardValue, ComponentError> {
            match data_model_host::evaluate(cond) {
                Ok(DataValue::Boolean(value)) => Ok(Some(value)),
                Ok(_) => Ok(None),
                Err(error) if recoverable(error.code) => Ok(None),
                Err(error) => Err(error),
            }
        }

        fn read(&mut self, location: &str) -> Result<Result<Data, ()>, ComponentError> {
            match data_model_host::evaluate(location) {
                Ok(DataValue::JsonArray(text)) => Ok(Ok(Data::JsonArray(text))),
                Ok(DataValue::JsonObject(text)) => Ok(Ok(Data::JsonObject(text))),
                Ok(DataValue::Number(value)) => Ok(scxml_types::FiniteNumber::new(value)
                    .map(Data::Number)
                    .map_err(|_| ())),
                Ok(_) => Ok(Err(())),
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
                Data::JsonArray(text) => DataValue::JsonArray(text.clone()),
                Data::JsonObject(text) => DataValue::JsonObject(text.clone()),
                Data::String(text) => DataValue::Text(text.clone()),
                Data::Number(value) => DataValue::Number(value.get()),
                Data::Bool(value) => DataValue::Boolean(*value),
                Data::Null => DataValue::Null,
                _ => return Ok(Err(())),
            };
            match data_model_host::assign(location, &value) {
                Ok(()) => Ok(Ok(())),
                Err(error) if recoverable(error.code) => Ok(Err(())),
                Err(error) => Err(error),
            }
        }

        fn snapshot(&mut self, exclude: &[Section]) -> Result<Result<String, ()>, ComponentError> {
            let exclude: Vec<session_snapshot_host::Section> = exclude
                .iter()
                .map(|section| match section {
                    Section::System => session_snapshot_host::Section::System,
                    Section::DataModel => session_snapshot_host::Section::DataModel,
                    Section::Queues => session_snapshot_host::Section::Queues,
                    Section::PendingSends => session_snapshot_host::Section::PendingSends,
                    Section::Transitions => session_snapshot_host::Section::Transitions,
                })
                .collect();
            match session_snapshot_host::snapshot(&exclude, None) {
                Ok(document) => Ok(Ok(document)),
                // No image for this call, the call bound, or a document
                // bound: the element cannot read the choices (`error.execution`).
                Err(error)
                    if recoverable(error.code) || error.code == ErrorCode::CapacityExhausted =>
                {
                    Ok(Err(()))
                }
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
