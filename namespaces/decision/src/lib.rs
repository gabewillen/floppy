//! The `decision` extension namespace: `<decision:choice>` and
//! `<decision:compact>` as a `stateforward:scxml/executable-content-extension`
//! Wasm component.
//!
//! Executable content is synchronous and quick, so this namespace holds only
//! the half of a decision that is: reading the choices and compacting them. Asking a backend to
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
//! `event:schema`), and `description` (the schema's `description`, else its
//! `title`, first line; omitted when it has neither), canonical JSON with
//! members sorted by key, in the session snapshot's walk order. `events`
//! (optional) filters it: see "The filter". Every other attribute
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
//! The choices' schemas are the contracts of the events the decision service
//! may return: it validates the chosen event's data against them before it
//! returns it (the document declares its events, not the service package).
//!
//! # `<decision:compact location="L" bound="N" overlocation="O"/>`
//!
//! Rewrites the choice array at `location` (required) in place to what a
//! model needs to fill each event (mosfet `model_facing_event_json_schema`
//! and the offer bound): every entry becomes `{event, schema, description?,
//! target?}`, canonical JSON, in order, and every schema loses `$schema`,
//! `title` and `examples` at every level, keeping property names (the
//! members of `properties`, `patternProperties`, `$defs` and `definitions`
//! are names, not keywords) and copying `enum`, `const`, `required` and
//! `default` verbatim. If the result is over `bound` bytes (optional, a
//! decimal integer, default [`DEFAULT_COMPACT_BOUND`]: the canonical JSON's
//! UTF-8 length, which is at least `JSON.stringify`'s length), every
//! `description` is dropped too, the entries' and the schemas'. If it is
//! still over, `location` receives `[]`. `overlocation` (optional) receives
//! `true` when that happened and `false` otherwise, so the document can log
//! it. Compacting is idempotent.
//!
//! An entry is an object with a non-empty string `event`, a `schema`, and
//! optionally a string `description` and a non-empty string `target` (the
//! offering ability), and nothing else; a `location` that does not hold an
//! array of such entries, a malformed `bound`, any other attribute, or any
//! content is `error.execution`. `overlocation` is assigned before
//! `location`, so a failed `location` assignment can leave `overlocation`
//! written.
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

/// `<decision:compact>`'s bound when it names none: 32 KiB, the largest
/// choice list phase 4 spike (b) relays through two sessions safely.
pub const DEFAULT_COMPACT_BOUND: usize = 32_768;

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
}

/// The canonical choices document of `entries`: a JSON array of
/// `{"description"?, "event", "schema"}` objects in order, canonical JSON.
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
        "compact" => compact(attributes, content, host),
        _ => Ok(Executed::NotHandled),
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
    for attribute in attributes {
        let slot = match (
            attribute.namespace_uri.as_deref(),
            attribute.local_name.as_str(),
        ) {
            (None, "location") => &mut location,
            (None, "events") => &mut events,
            _ => {
                return Err(Failure::Execution(
                    "decision:choice takes only the location and events attributes",
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
    host.assign(location, &Data::JsonArray(choices_json(&choices)))
        .map_err(Failure::Host)?
        .map_err(|()| {
            Failure::Execution("decision:choice could not assign the choices to location")
        })?;
    Ok(Executed::Handled)
}

/// Executes `<decision:compact>`.
fn compact<H: ChoiceHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let mut location = None;
    let mut bound = None;
    let mut over_location = None;
    for attribute in attributes {
        let slot = match (
            attribute.namespace_uri.as_deref(),
            attribute.local_name.as_str(),
        ) {
            (None, "location") => &mut location,
            (None, "bound") => &mut bound,
            (None, "overlocation") => &mut over_location,
            _ => {
                return Err(Failure::Execution(
                    "decision:compact takes only the location, bound and overlocation attributes",
                ));
            }
        };
        if slot.replace(attribute.value.as_str()).is_some() {
            return Err(Failure::Execution("decision:compact repeats an attribute"));
        }
    }
    let location = location
        .map(str::trim)
        .filter(|location| !location.is_empty())
        .ok_or(Failure::Execution(
            "decision:compact requires a non-empty location",
        ))?;
    let over_location = over_location
        .map(|value| {
            Some(value.trim())
                .filter(|value| !value.is_empty())
                .ok_or(Failure::Execution("decision:compact overlocation is empty"))
        })
        .transpose()?;
    let bound = bound.map_or(Ok(DEFAULT_COMPACT_BOUND), |value| {
        let value = value.trim();
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(Failure::Execution(
                "decision:compact bound is not a decimal integer",
            ));
        }
        value
            .parse::<usize>()
            .map_err(|_| Failure::Execution("decision:compact bound is not a decimal integer"))
    })?;
    if content
        .iter()
        .any(|item| !matches!(item, Content::Text(text) if text.trim().is_empty()))
    {
        return Err(Failure::Execution("decision:compact holds no content"));
    }
    let not_choices = || Failure::Execution("decision:compact location does not hold choices");
    let Data::JsonArray(text) = host
        .read(location)
        .map_err(Failure::Host)?
        .map_err(|()| not_choices())?
    else {
        return Err(not_choices());
    };
    let choices: Json = serde_json::from_str(&text).map_err(|_| not_choices())?;
    let (compacted, over) = compact_choices(&choices, bound).map_err(Failure::Execution)?;
    if let Some(over_location) = over_location {
        host.assign(over_location, &Data::Bool(over))
            .map_err(Failure::Host)?
            .map_err(|()| Failure::Execution("decision:compact could not assign overlocation"))?;
    }
    host.assign(location, &Data::JsonArray(compacted))
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution("decision:compact could not assign the choices"))?;
    Ok(Executed::Handled)
}

/// The compacted choices of `choices` as canonical JSON, and whether they
/// were over `bound` even without descriptions (then they are `[]`): see
/// the crate documentation (`<decision:compact>`).
///
/// # Errors
///
/// A static detail when `choices` is not an array of choice entries.
pub fn compact_choices(choices: &Json, bound: usize) -> Result<(String, bool), &'static str> {
    let entries = choices
        .as_array()
        .ok_or("decision:compact location does not hold choices")?;
    for entry in entries {
        let entry = entry
            .as_object()
            .ok_or("decision:compact found a choice that is not an object")?;
        let text = |key: &str| {
            entry.get(key).map(|value| {
                value
                    .as_str()
                    .filter(|text| key == "description" || !text.is_empty())
            })
        };
        if entry
            .keys()
            .any(|key| !["event", "schema", "description", "target"].contains(&key.as_str()))
            || text("event").flatten().is_none()
            || !entry.contains_key("schema")
            || matches!(text("description"), Some(None))
            || matches!(text("target"), Some(None))
        {
            return Err("decision:compact found a malformed choice");
        }
    }
    for descriptions in [true, false] {
        let compacted = canonical_json(&Json::Array(
            entries
                .iter()
                .map(|entry| compact_entry(entry, descriptions))
                .collect(),
        ));
        if compacted.len() <= bound {
            return Ok((compacted, false));
        }
    }
    Ok(("[]".to_owned(), true))
}

/// One checked choice entry, compacted.
fn compact_entry(entry: &Json, descriptions: bool) -> Json {
    let mut out = Map::new();
    for (key, value) in entry.as_object().into_iter().flatten() {
        match key.as_str() {
            "schema" => {
                out.insert(key.clone(), compact_schema(value, descriptions));
            }
            "description" if !descriptions => {}
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    Json::Object(out)
}

/// A schema without `$schema`, `title`, `examples` (and `description` unless
/// `descriptions`), at every level; property names are kept.
fn compact_schema(schema: &Json, descriptions: bool) -> Json {
    match schema {
        Json::Array(items) => Json::Array(
            items
                .iter()
                .map(|item| compact_schema(item, descriptions))
                .collect(),
        ),
        Json::Object(members) => {
            let mut out = Map::new();
            for (key, value) in members {
                match key.as_str() {
                    "$schema" | "title" | "examples" => {}
                    "description" if !descriptions => {}
                    "properties" | "patternProperties" | "$defs" | "definitions"
                        if value.is_object() =>
                    {
                        let named = value
                            .as_object()
                            .into_iter()
                            .flatten()
                            .map(|(name, item)| (name.clone(), compact_schema(item, descriptions)))
                            .collect();
                        out.insert(key.clone(), Json::Object(named));
                    }
                    "enum" | "const" | "required" | "default" => {
                        out.insert(key.clone(), value.clone());
                    }
                    _ => {
                        out.insert(key.clone(), compact_schema(value, descriptions));
                    }
                }
            }
            Json::Object(out)
        }
        _ => schema.clone(),
    }
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
    let malformed = || Failure::Execution("the session snapshot is malformed");
    let document: Json = serde_json::from_str(document).map_err(|_| malformed())?;
    let Some(transitions) = document.pointer("/snapshot/root/transitions") else {
        return Ok(Vec::new());
    };
    let transitions = transitions.as_array().ok_or_else(malformed)?;
    let atomic: Vec<&str> = document
        .pointer("/snapshot/root/configuration/atomic")
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
            (Some(Json::String(cond)), Some(true) | None) => {
                match guard(cond).map_err(Failure::Host)? {
                    Some(true) => Guard::Enabled,
                    Some(false) => Guard::Disabled,
                    None => Guard::Unknown,
                }
            }
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
                Data::Bool(value) => DataValue::Boolean(*value),
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
            match session_snapshot_host::snapshot(&exclude) {
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
