//! The `processing` extension namespace: `<processing:model_facing_schema>`,
//! `<processing:collect_offered_events>`, `<processing:send>` and
//! `<processing:selection_confidence>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! What a stage needs between the offered events and their dispatch: the
//! model-facing schema of each offered event, the turn's offered events
//! with the sessions that offer each, the one `dispatch` function tool a
//! model selects them with ([`dispatch_tool`]), and the confidence of a
//! selection ([`normalize_confidence`], [`least_confidence`]). These are
//! synchronous and quick, so they are executable content here. Dispatching
//! is SCXML's own `<send>` ("dispatch is obviously send"): this namespace
//! works out what may be sent and never sends. Documents bind
//! [`NAMESPACE_URI`] (conventionally with the prefix `processing`):
//!
//! ```xml
//! <onentry><processing:send location="schema" choices="choices"/></onentry>
//! <transition event="generate.result">
//!   <foreach array="_event.data.tool_calls" item="call">
//!     <foreach array="call.args.events" item="s">
//!       <send eventexpr="s.event" targetexpr="'#_scxml_' + s.target"><content expr="selectionData(s)"/></send>
//!     </foreach>
//!   </foreach>
//! </transition>
//! ```
//!
//! # Choices
//!
//! A choice entry is an object with a non-empty string `event`, a `schema`,
//! optionally a string `description`, a non-empty string `target` (the
//! session id of the session that offers it), an `effort` (a whole number
//! from 0 to 1000, how heavy an action it is, decision D206) and a `draft`
//! (an object, the event's draft marker that the generate service's
//! `drafts` reads), and nothing else, as `<decision:choice>` writes it and
//! `<processing:collect_offered_events>` tags it. Every element keeps an
//! entry's `effort` and `draft` as they are; the dispatch tool never shows
//! them.
//!
//! # `<processing:model_facing_schema location="L" bound="N" overlocation="O"/>`
//!
//! Rewrites the choice array at `location` (required) in place to what a
//! model needs to fill each event (under floppy's offer bound, decision
//! D35): every entry becomes `{event, schema, description?, target?, effort?, draft?}`,
//! canonical JSON, in order, and every schema loses `$schema`, `title` and
//! `examples` at every level, keeping property names (the members of
//! `properties`, `patternProperties`, `$defs` and `definitions` are names,
//! not keywords) and copying `enum`, `const`, `required` and `default`
//! verbatim. If the result is over `bound` bytes (optional, a decimal
//! integer, default [`DEFAULT_BOUND`]: the canonical JSON's UTF-8 length,
//! which is at least `JSON.stringify`'s length), every `description` is
//! dropped too, the entries' and the schemas'. If it is still over,
//! `location` receives `[]`. `overlocation` (optional) receives `true` when
//! that happened and `false` otherwise, so the document can log it. It is
//! idempotent. `overlocation` is assigned before `location`.
//!
//! # `<processing:collect_offered_events location="L" choices="X" offers="Y" devices="D" focus="F" bound="N" overlocation="O" sizelocation="S"/>`
//!
//! A turn's offered events, each tagged with the session id of the session
//! that offers it: the choices
//! `choices` (required) evaluates to are the document's own, each tagged
//! with the document's `_sessionid`; each member of the object `offers`
//! (optional) evaluates to (an invoke id of the document's child → its
//! latest offered choices) is tagged with the child's session id,
//! `<_sessionid>.<invoke id>` (decision D110); and with `devices` and
//! `focus` (optional, together), when `focus` evaluates to a string that
//! names a member of the object `devices` evaluates to (a device's
//! reference → its `<device:snapshot>` read `{id, state, attributes,
//! events}`), each of that device's `events` is offered as `{event, schema:
//! {type: object}}` tagged with its `id` (the device's own `event:schema`
//! decides its data when it is delivered). A `focus` that is `null` or
//! names no member offers no device. The document's own come first, then
//! the offers and the focused device in name order (invoke id or device
//! reference). They are compacted as `<processing:model_facing_schema>`
//! does, with descriptions when that fits `bound` (default
//! [`DEFAULT_BOUND`]) and without otherwise; unlike it, a list still over
//! the bound is kept (the document logs it and fails the turn) and
//! `overlocation` (optional) receives `true`. `sizelocation` (optional)
//! receives the assigned list's size in bytes. `overlocation` and
//! `sizelocation` are assigned before `location`.
//!
//! # `<processing:send location="L" choices="X" max="N"/>`
//!
//! `location` receives the `dispatch` tool for the choices `choices`
//! evaluates to as one JSON Schema: the tool's `parameters`, with the tool's
//! description ([`DISPATCH_DESCRIPTION`]) as its root `description`, which
//! the generate service sends as the function's `description`. The
//! parameters are `{events: [anyOf one branch per offered event]}` (the
//! events in choices order, each offered once; `events` required, with a
//! fixed description and the examples `[]`, and the first branch's
//! example when it has one), and with no choices an item is any
//! `{event, data?}`. `max` (optional, a decimal integer) puts `maxItems`
//! on `events` (reflection selects at most one event). A branch is
//! `{event, data, reason?, target?}` with fixed descriptions: `event` a
//! one-value enum; `data` the first such choice's schema without its
//! top-level `$schema` (an empty object for no schema or an empty one, the
//! schema wrapped as the required property `value` for one of another type
//! without `properties`, else with `type` `object` and
//! `additionalProperties` `false` when it names neither);
//! `reason` an optional string; `target`, present and required when some
//! choice of the event has one, an enum of the distinct targets offering
//! it, sorted. `event` and `data` are required (an empty `data` too:
//! sorted, `data` precedes `event`, decision D231); no other property is
//! admitted. The branch's `description` is the
//! data schema's, else the event name, and when the data schema's
//! `examples` starts with an object the branch has one example, `{event,
//! data, target}` (`target` only for a single target). Nothing is
//! normalized for strict decoding. The result is canonical JSON. The
//! tool's name on the wire is `dispatch` (the generate service's
//! `schemaname`).
//!
//! # `<processing:selection_confidence location="L" selections="X"/>`
//!
//! `location` receives the confidence of the selections `selections`
//! evaluates to (an array of objects): the least of their `confidence`s,
//! each normalized by [`normalize_confidence`] (a number in `(0, 1]` is a
//! probability, scaled to 0–100; any other number is on the 0–100 scale
//! already; rounded half to even and clamped to 0–100); a selection whose
//! `confidence` is not a number is left out, and with none left `location`
//! receives `null`.
//!
//! # Failures
//!
//! A malformed element, an expression that does not hold what the element
//! reads, or a failed assignment is an `execution` error with a static
//! detail (SCXML `error.execution`); nothing is assigned then.
//!
//! # Bounds
//!
//! Choices are bounded by the offer bound each element names (decision
//! D35); the data model bounds the values it hands this component.
//!
//! # Telemetry
//!
//! Each entry point (`initialize`, `execute`, `shutdown`) records one
//! observation through scxml.rs's `telemetry` WIT facade: its closed
//! operation, its closed outcome (`ok`, `rejected`, `failed`), and one
//! closed signal event. No attribute carries an event name, a schema, a
//! session id, or a location, so the cardinality is the product of the
//! three closed enums.

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
use serde_json::{Map, Value as Json};

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "processing";

/// The exact namespace URI documents bind: tagless, so it names the
/// repository's latest release (decision D19); the manifest and schema name
/// it too.
pub const NAMESPACE_URI: &str = "https://github.com/stateforward/floppy/namespaces/processing";

/// The offer bound when an element names none: 32 KiB, the largest choice
/// list phase 4 spike (b) relays through two sessions safely (decision D35).
pub const DEFAULT_BOUND: usize = 32_768;

/// The greatest effort a choice entry may carry (decision D206), as
/// `<decision:choice>` declares it.
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
pub trait ProcessingHost {
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
pub fn execute<H: ProcessingHost>(
    local_name: &str,
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    match local_name {
        "model_facing_schema" => model_facing_schema(attributes, content, host),
        "collect_offered_events" => collect_offered_events(attributes, content, host),
        "send" => send(attributes, content, host),
        "selection_confidence" => selection_confidence(attributes, content, host),
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

/// A decimal integer attribute, `default` when absent.
fn decimal<E>(
    value: Option<&str>,
    default: usize,
    malformed: &'static str,
) -> Result<usize, Failure<E>> {
    value.map_or(Ok(default), |value| {
        let value = value.trim();
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(Failure::Execution(malformed));
        }
        value
            .parse::<usize>()
            .map_err(|_| Failure::Execution(malformed))
    })
}

/// Evaluates `expression`, else `malformed`.
fn read_value<H: ProcessingHost>(
    host: &mut H,
    expression: &str,
    malformed: &'static str,
) -> Result<Data, Failure<H::Error>> {
    host.read(expression)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(malformed))
}

/// Evaluates `expression` as JSON (an array or an object), else `malformed`.
fn read_json<H: ProcessingHost>(
    host: &mut H,
    expression: &str,
    malformed: &'static str,
) -> Result<Json, Failure<H::Error>> {
    match read_value(host, expression, malformed)? {
        Data::JsonArray(text) | Data::JsonObject(text) => {
            serde_json::from_str(&text).map_err(|_| Failure::Execution(malformed))
        }
        _ => Err(Failure::Execution(malformed)),
    }
}

/// Assigns `value` to `location`, else `failed`.
fn assign_to<H: ProcessingHost>(
    host: &mut H,
    location: &str,
    value: &Data,
    failed: &'static str,
) -> Result<(), Failure<H::Error>> {
    host.assign(location, value)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(failed))
}

/// Assigns a byte size to `location`, else `failed`.
fn assign_size<H: ProcessingHost>(
    host: &mut H,
    location: &str,
    size: usize,
    too_large: &'static str,
    failed: &'static str,
) -> Result<(), Failure<H::Error>> {
    let size = u32::try_from(size)
        .ok()
        .and_then(|size| scxml_types::FiniteNumber::new(f64::from(size)).ok())
        .ok_or(Failure::Execution(too_large))?;
    assign_to(host, location, &Data::Number(size), failed)
}

/// Executes `<processing:model_facing_schema>`.
fn model_facing_schema<H: ProcessingHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, bound, over_location] = attribute_values(
        attributes,
        ["location", "bound", "overlocation"],
        "processing:model_facing_schema takes only the location, bound and overlocation attributes",
        "processing:model_facing_schema repeats an attribute",
    )?;
    let location = required(
        location,
        "processing:model_facing_schema requires a non-empty location",
    )?;
    let bound = decimal(
        bound,
        DEFAULT_BOUND,
        "processing:model_facing_schema bound is not a decimal integer",
    )?;
    let over_location = optional(
        over_location,
        "processing:model_facing_schema overlocation is empty",
    )?;
    no_content(content, "processing:model_facing_schema holds no content")?;
    let not_choices = "processing:model_facing_schema location does not hold choices";
    let Data::JsonArray(text) = read_value(host, location, not_choices)? else {
        return Err(Failure::Execution(not_choices));
    };
    let choices: Json = serde_json::from_str(&text).map_err(|_| Failure::Execution(not_choices))?;
    let (compacted, over) = model_facing_choices(&choices, bound).map_err(Failure::Execution)?;
    if let Some(over_location) = over_location {
        assign_to(
            host,
            over_location,
            &Data::Bool(over),
            "processing:model_facing_schema could not assign overlocation",
        )?;
    }
    assign_to(
        host,
        location,
        &Data::JsonArray(compacted),
        "processing:model_facing_schema could not assign the choices",
    )?;
    Ok(Executed::Handled)
}

/// The model-facing choices of `choices` as canonical JSON, and whether
/// they were over `bound` even without descriptions (then they are `[]`):
/// see the crate documentation (`<processing:model_facing_schema>`).
///
/// # Errors
///
/// A static detail when `choices` is not an array of choice entries.
pub fn model_facing_choices(choices: &Json, bound: usize) -> Result<(String, bool), &'static str> {
    let entries = checked_entries(choices, MODEL_FACING_ENTRIES)?;
    for descriptions in [true, false] {
        let compacted = canonical_json(&Json::Array(
            entries
                .iter()
                .map(|entry| model_facing_entry(entry, descriptions))
                .collect(),
        ));
        if compacted.len() <= bound {
            return Ok((compacted, false));
        }
    }
    Ok(("[]".to_owned(), true))
}

/// One checked choice entry, model-facing.
fn model_facing_entry(entry: &Json, descriptions: bool) -> Json {
    let mut out = Map::new();
    for (key, value) in entry.as_object().into_iter().flatten() {
        match key.as_str() {
            "schema" => {
                out.insert(key.clone(), model_facing(value, descriptions));
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
fn model_facing(schema: &Json, descriptions: bool) -> Json {
    match schema {
        Json::Array(items) => Json::Array(
            items
                .iter()
                .map(|item| model_facing(item, descriptions))
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
                            .map(|(name, item)| (name.clone(), model_facing(item, descriptions)))
                            .collect();
                        out.insert(key.clone(), Json::Object(named));
                    }
                    "enum" | "const" | "required" | "default" => {
                        out.insert(key.clone(), value.clone());
                    }
                    _ => {
                        out.insert(key.clone(), model_facing(value, descriptions));
                    }
                }
            }
            Json::Object(out)
        }
        _ => schema.clone(),
    }
}

/// Executes `<processing:collect_offered_events>`.
fn collect_offered_events<H: ProcessingHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [
        location,
        own,
        offers,
        devices,
        focus,
        bound,
        over_location,
        size_location,
    ] = attribute_values(
        attributes,
        [
            "location",
            "choices",
            "offers",
            "devices",
            "focus",
            "bound",
            "overlocation",
            "sizelocation",
        ],
        "processing:collect_offered_events takes only the location, choices, offers, devices, focus, bound, overlocation and sizelocation attributes",
        "processing:collect_offered_events repeats an attribute",
    )?;
    let location = required(
        location,
        "processing:collect_offered_events requires a non-empty location",
    )?;
    let own = required(
        own,
        "processing:collect_offered_events requires a non-empty choices",
    )?;
    let offers = optional(offers, "processing:collect_offered_events offers is empty")?;
    let devices = optional(
        devices,
        "processing:collect_offered_events devices is empty",
    )?;
    let focus = optional(focus, "processing:collect_offered_events focus is empty")?;
    if devices.is_some() != focus.is_some() {
        return Err(Failure::Execution(
            "processing:collect_offered_events takes devices and focus together",
        ));
    }
    let bound = decimal(
        bound,
        DEFAULT_BOUND,
        "processing:collect_offered_events bound is not a decimal integer",
    )?;
    let over_location = optional(
        over_location,
        "processing:collect_offered_events overlocation is empty",
    )?;
    let size_location = optional(
        size_location,
        "processing:collect_offered_events sizelocation is empty",
    )?;
    no_content(
        content,
        "processing:collect_offered_events holds no content",
    )?;
    let Data::String(session) = read_value(
        host,
        "_sessionid",
        "processing:collect_offered_events found no _sessionid",
    )?
    else {
        return Err(Failure::Execution(
            "processing:collect_offered_events found no _sessionid",
        ));
    };
    let own = read_json(
        host,
        own,
        "processing:collect_offered_events choices does not hold choices",
    )?;
    let offers = match offers {
        Some(offers) => read_json(
            host,
            offers,
            "processing:collect_offered_events offers does not hold offers",
        )?,
        None => Json::Object(Map::new()),
    };
    let device = match (devices, focus) {
        (Some(devices), Some(focus)) => read_focused_device(host, devices, focus)?,
        _ => None,
    };
    let collected = collected_offered_events(&session, &own, &offers, device.as_ref(), bound)
        .map_err(Failure::Execution)?;
    assign_collected(host, collected, location, over_location, size_location)?;
    Ok(Executed::Handled)
}

/// Assigns collected choices: `overlocation` and `sizelocation` first,
/// then `location`.
fn assign_collected<H: ProcessingHost>(
    host: &mut H,
    collected: Collected,
    location: &str,
    over_location: Option<&str>,
    size_location: Option<&str>,
) -> Result<(), Failure<H::Error>> {
    if let Some(over_location) = over_location {
        assign_to(
            host,
            over_location,
            &Data::Bool(collected.over),
            "processing:collect_offered_events could not assign overlocation",
        )?;
    }
    if let Some(size_location) = size_location {
        assign_size(
            host,
            size_location,
            collected.choices.len(),
            "processing:collect_offered_events choices are too large",
            "processing:collect_offered_events could not assign sizelocation",
        )?;
    }
    assign_to(
        host,
        location,
        &Data::JsonArray(collected.choices),
        "processing:collect_offered_events could not assign the choices",
    )?;
    Ok(())
}

/// The device the `focus` expression names among the `devices` expression's
/// reads, when it names one (see [`focused_device`]).
fn read_focused_device<H: ProcessingHost>(
    host: &mut H,
    devices: &str,
    focus: &str,
) -> Result<Option<Device>, Failure<H::Error>> {
    let devices = read_json(
        host,
        devices,
        "processing:collect_offered_events devices does not hold devices",
    )?;
    let focus = match read_value(
        host,
        focus,
        "processing:collect_offered_events focus did not evaluate",
    )? {
        Data::String(reference) => Some(reference),
        Data::Null => None,
        _ => {
            return Err(Failure::Execution(
                "processing:collect_offered_events focus is neither a string nor null",
            ));
        }
    };
    focused_device(&devices, focus.as_deref()).map_err(Failure::Execution)
}

/// The focused device a turn offers: its reference, its session id, and
/// the events it accepts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Device {
    /// The device's reference (its name among the offers).
    pub reference: String,
    /// The device session's id.
    pub id: String,
    /// The events its live session accepts.
    pub events: Vec<String>,
}

/// The device `focus` names among `devices` (reference → `{id, events,
/// ...}`), when it names one.
///
/// # Errors
///
/// A static detail when `devices` is not an object, or the named member is
/// not an object with a non-empty string `id` and an array of non-empty
/// string `events`.
pub fn focused_device(devices: &Json, focus: Option<&str>) -> Result<Option<Device>, &'static str> {
    const MALFORMED: &str = "processing:collect_offered_events devices does not hold devices";
    let devices = devices.as_object().ok_or(MALFORMED)?;
    let Some((reference, read)) = focus.and_then(|focus| devices.get_key_value(focus)) else {
        return Ok(None);
    };
    let id = read
        .get("id")
        .and_then(Json::as_str)
        .filter(|id| !id.is_empty())
        .ok_or(MALFORMED)?;
    let events = read
        .get("events")
        .and_then(Json::as_array)
        .ok_or(MALFORMED)?
        .iter()
        .map(|event| {
            event
                .as_str()
                .filter(|event| !event.is_empty())
                .map(str::to_owned)
                .ok_or(MALFORMED)
        })
        .collect::<Result<Vec<String>, &'static str>>()?;
    Ok(Some(Device {
        reference: reference.clone(),
        id: id.to_owned(),
        events,
    }))
}

/// A turn's collected offered events (see [`collected_offered_events`]).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Collected {
    /// The tagged, model-facing choices, canonical JSON.
    pub choices: String,
    /// Whether the choices are over the bound even without descriptions.
    pub over: bool,
}

/// The choices `own` tagged with `session`, then each `offers` member's
/// tagged with `<session>.<member>` and the `device`'s tagged with its id,
/// by name; model-facing like `<processing:model_facing_schema>`, except
/// that over the bound without descriptions the description-less list is
/// kept and `over` is set.
///
/// # Errors
///
/// A static detail when `own` is not an array of choice entries or `offers`
/// is not an object of such arrays.
pub fn collected_offered_events(
    session: &str,
    own: &Json,
    offers: &Json,
    device: Option<&Device>,
    bound: usize,
) -> Result<Collected, &'static str> {
    if session.is_empty() {
        return Err("processing:collect_offered_events found no _sessionid");
    }
    let tagged = |entry: &Json, target: &str| {
        let mut entry = entry.clone();
        if let Some(entry) = entry.as_object_mut() {
            entry.insert("target".into(), Json::String(target.to_owned()));
        }
        entry
    };
    let mut all: Vec<Json> = checked_entries(own, COLLECT_ENTRIES)?
        .iter()
        .map(|entry| tagged(entry, session))
        .collect();
    let offers = offers
        .as_object()
        .ok_or("processing:collect_offered_events offers does not hold offers")?;
    // Each offerer by name: (name, its session id, its choices).
    let mut offerers: Vec<(&str, String, Vec<Json>)> = Vec::with_capacity(offers.len() + 1);
    for (name, offered) in offers {
        if name.is_empty() {
            return Err("processing:collect_offered_events offers names an empty invoke id");
        }
        if !offered.is_array() {
            return Err("processing:collect_offered_events offers does not hold offers");
        }
        offerers.push((
            name.as_str(),
            format!("{session}.{name}"),
            checked_entries(offered, COLLECT_ENTRIES)?.to_vec(),
        ));
    }
    if let Some(device) = device {
        offerers.push((
            device.reference.as_str(),
            device.id.clone(),
            device
                .events
                .iter()
                .map(|event| serde_json::json!({"event": event, "schema": {"type": "object"}}))
                .collect(),
        ));
    }
    offerers.sort_by(|left, right| left.0.cmp(right.0));
    for (_, target, choices) in &offerers {
        all.extend(choices.iter().map(|entry| tagged(entry, target)));
    }
    let mut compacted = String::new();
    let mut over = false;
    for descriptions in [true, false] {
        compacted = canonical_json(&Json::Array(
            all.iter()
                .map(|entry| model_facing_entry(entry, descriptions))
                .collect(),
        ));
        over = compacted.len() > bound;
        if !over {
            break;
        }
    }
    Ok(Collected {
        choices: compacted,
        over,
    })
}

/// Executes `<processing:send>`.
fn send<H: ProcessingHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, choices, max] = attribute_values(
        attributes,
        ["location", "choices", "max"],
        "processing:send takes only the location, choices and max attributes",
        "processing:send repeats an attribute",
    )?;
    let location = required(location, "processing:send requires a non-empty location")?;
    let choices = required(choices, "processing:send requires a non-empty choices")?;
    let max = max
        .map(|max| decimal(Some(max), 0, "processing:send max is not a decimal integer"))
        .transpose()?;
    no_content(content, "processing:send holds no content")?;
    let choices = read_json(
        host,
        choices,
        "processing:send choices does not hold choices",
    )?;
    let schema = dispatch_tool(&choices, max).map_err(Failure::Execution)?;
    assign_to(
        host,
        location,
        &Data::JsonObject(schema),
        "processing:send could not assign the schema",
    )?;
    Ok(Executed::Handled)
}

/// The `dispatch` tool of `choices` as one JSON Schema, canonical
/// JSON: its `parameters`, with its `description` as the root
/// `description` (the generate service sends that as the function's), and
/// `maxItems` `max` on `events` when given. See the crate documentation
/// (`<processing:send>`).
///
/// # Errors
///
/// A static detail when `choices` is not an array of choice entries.
pub fn dispatch_tool(choices: &Json, max: Option<usize>) -> Result<String, &'static str> {
    // Each offered event once, in choices order: its first choice and the
    // distinct targets offering it.
    let mut offered: Vec<(&Json, Vec<String>)> = Vec::new();
    for entry in checked_entries(choices, SEND_ENTRIES)? {
        let event = entry.get("event");
        let index = offered
            .iter()
            .position(|(first, _)| first.get("event") == event)
            .unwrap_or_else(|| {
                offered.push((entry, Vec::new()));
                offered.len() - 1
            });
        if let Some(target) = entry
            .get("target")
            .and_then(Json::as_str)
            .filter(|target| !target.is_empty())
            && let Some((_, targets)) = offered.get_mut(index)
            && !targets.iter().any(|known| known == target)
        {
            targets.push(target.to_owned());
        }
    }
    let branches: Vec<Json> = offered
        .into_iter()
        .map(|(choice, mut targets)| {
            // The targets offering the event: sorted, distinct.
            targets.sort();
            selection_item_branch(
                choice.get("event").cloned().unwrap_or(Json::Null),
                choice.get("schema").unwrap_or(&Json::Null),
                &targets,
            )
        })
        .collect();
    let mut examples = vec![Json::Array(Vec::new())];
    if let Some(example) = branches
        .first()
        .and_then(|branch| branch.get("examples"))
        .and_then(Json::as_array)
        .and_then(|examples| examples.first())
    {
        examples.push(example.clone());
    }
    let items = if branches.is_empty() {
        serde_json::json!({
            "type": "object",
            "properties": {
                "event": {"type": "string"},
                "data": {"type": "object", "additionalProperties": false},
            },
            "required": ["event"],
            "additionalProperties": false,
        })
    } else {
        serde_json::json!({"anyOf": branches})
    };
    let mut events = serde_json::json!({
        "type": "array",
        "description": DISPATCH_EVENTS_DESCRIPTION,
        "items": items,
        "examples": examples,
    });
    if let (Some(max), Some(events)) = (max, events.as_object_mut()) {
        events.insert("maxItems".into(), Json::from(max));
    }
    Ok(canonical_json(&serde_json::json!({
        "description": DISPATCH_DESCRIPTION,
        "type": "object",
        "properties": {"events": events},
        "required": ["events"],
        "additionalProperties": false,
    })))
}

/// The `dispatch` tool's description.
pub const DISPATCH_DESCRIPTION: &str = "Dispatch all events necessary for the input. Call once. \
    Multi-select by listing every offered event this turn requires in the events array \
    (for example a device action and speech together when both are needed). \
    Each item must match one offered event branch; payload fields and requirements are \
    defined on that event's data schema. An empty events list leaves the turn unhandled for \
    the host cascade (e.g. deliberative reasoning); use an explicit ignore/pass event when \
    the stage should handle the turn with no environment actions.";

/// The description of the `dispatch` tool's `events`.
const DISPATCH_EVENTS_DESCRIPTION: &str = "All events necessary for the input this turn. Each item is one offered \
    event branch (enum-pinned name + that event's data schema). Include every action \
    required together; empty array selects none.";

/// One offered event's branch: `event` a one-value enum, `data` the event's
/// schema ([`selection_data`]), `reason` optional, `target` an enum of the
/// `targets` offering it when there are any; `event` and `data` required,
/// `target` when present; the
/// schema's description (else the event name) and, when its `examples`
/// starts with an object, one example.
fn selection_item_branch(event: Json, schema: &Json, targets: &[String]) -> Json {
    let data = selection_data(schema);
    let description = data
        .get("description")
        .and_then(Json::as_str)
        .filter(|description| !description.trim().is_empty())
        .map_or_else(|| event.clone(), Json::from);
    // `data` is required in every branch, an empty one included: the
    // wire schema's members are sorted, so `data` comes before `event`,
    // and a backend that decodes members in schema order would otherwise
    // let a call that writes `event` first take only the branches whose
    // `data` is optional (decision D231).
    let mut required = vec![Json::from("event"), Json::from("data")];
    let first_example = data
        .get("examples")
        .and_then(Json::as_array)
        .and_then(|examples| examples.first())
        .filter(|example| example.is_object())
        .cloned();
    let mut properties = Map::new();
    properties.insert(
        "event".into(),
        serde_json::json!({"type": "string", "enum": [event]}),
    );
    properties.insert("data".into(), data);
    properties.insert(
        "reason".into(),
        serde_json::json!({"type": "string", "description": "Optional short reason this event was selected."}),
    );
    match targets {
        [] => {}
        [target] => {
            properties.insert(
                "target".into(),
                serde_json::json!({
                    "type": "string",
                    "enum": [target],
                    "description": "Actor that receives this event. Fixed for this turn because only one actor enables it in the live topology.",
                }),
            );
            required.push("target".into());
        }
        _ => {
            properties.insert(
                "target".into(),
                serde_json::json!({
                    "type": "string",
                    "enum": targets,
                    "description": "Actor that should receive the event. Required because multiple actors enable this event this turn; choose exactly one offered actor key.",
                }),
            );
            required.push("target".into());
        }
    }
    let mut branch = Map::new();
    branch.insert("type".into(), "object".into());
    branch.insert("description".into(), description);
    branch.insert("properties".into(), Json::Object(properties));
    branch.insert("required".into(), Json::Array(required));
    branch.insert("additionalProperties".into(), Json::Bool(false));
    if let Some(first) = first_example {
        let mut example = Map::new();
        example.insert("event".into(), event);
        example.insert("data".into(), first);
        if let [target] = targets {
            example.insert("target".into(), Json::from(target.as_str()));
        }
        branch.insert("examples".into(), Json::Array(vec![Json::Object(example)]));
    }
    Json::Object(branch)
}

/// A choice's schema as a branch's `data`: no schema (or an empty one) is an
/// empty object; a schema of another type without `properties` is wrapped
/// as the required property `value`; otherwise the schema (without a top-level
/// `$schema`), `type` `object` and `additionalProperties` `false` when it
/// names neither.
fn selection_data(schema: &Json) -> Json {
    let mut out: Map<String, Json> = schema
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| key.as_str() != "$schema")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if out.is_empty() {
        return serde_json::json!({"type": "object", "properties": {}, "additionalProperties": false});
    }
    let typed = out.get("type").and_then(Json::as_str);
    if out.get("type").is_some() && typed != Some("object") && !out.contains_key("properties") {
        return serde_json::json!({
            "type": "object",
            "properties": {"value": Json::Object(out)},
            "required": ["value"],
            "additionalProperties": false,
        });
    }
    out.entry("type").or_insert_with(|| "object".into());
    out.entry("additionalProperties")
        .or_insert(Json::Bool(false));
    Json::Object(out)
}

/// Executes `<processing:selection_confidence>`.
fn selection_confidence<H: ProcessingHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, selections] = attribute_values(
        attributes,
        ["location", "selections"],
        "processing:selection_confidence takes only the location and selections attributes",
        "processing:selection_confidence repeats an attribute",
    )?;
    let location = required(
        location,
        "processing:selection_confidence requires a non-empty location",
    )?;
    let selections = required(
        selections,
        "processing:selection_confidence requires a non-empty selections",
    )?;
    no_content(content, "processing:selection_confidence holds no content")?;
    let selections = read_json(
        host,
        selections,
        "processing:selection_confidence selections is not an array of selections",
    )?;
    let confidence = least_confidence(&selections).map_err(Failure::Execution)?;
    let value = match confidence {
        Some(confidence) => Data::Number(
            scxml_types::FiniteNumber::new(f64::from(confidence)).map_err(|_| {
                Failure::Execution(
                    "processing:selection_confidence could not assign the confidence",
                )
            })?,
        ),
        None => Data::Null,
    };
    assign_to(
        host,
        location,
        &value,
        "processing:selection_confidence could not assign the confidence",
    )?;
    Ok(Executed::Handled)
}

/// A confidence on the 0–100 scale: a number in `(0, 1]` is a probability,
/// scaled to 0–100; any other is on the 0–100 scale; rounded half to even
/// and clamped to 0–100.
#[must_use]
pub fn normalize_confidence(value: f64) -> u8 {
    let scaled = if value > 0.0 && value <= 1.0 {
        value * 100.0
    } else {
        value
    };
    let rounded = scaled.round_ties_even().clamp(0.0, 100.0);
    // In 0..=100 after the clamp, so the conversion is exact.
    (0_u8..=100)
        .find(|whole| f64::from(*whole) >= rounded)
        .unwrap_or(100)
}

/// The selection confidence over normalized confidences: the least
/// `confidence` of the `selections` that carry a number, `None` when none
/// does.
///
/// # Errors
///
/// A static detail when `selections` is not an array of objects.
pub fn least_confidence(selections: &Json) -> Result<Option<u8>, &'static str> {
    const MALFORMED: &str =
        "processing:selection_confidence selections is not an array of selections";
    let selections = selections.as_array().ok_or(MALFORMED)?;
    let mut least: Option<u8> = None;
    for selection in selections {
        let selection = selection.as_object().ok_or(MALFORMED)?;
        if let Some(confidence) = selection.get("confidence").and_then(Json::as_f64) {
            let confidence = normalize_confidence(confidence);
            least = Some(least.map_or(confidence, |least| least.min(confidence)));
        }
    }
    Ok(least)
}

/// `<processing:model_facing_schema>`'s choice-entry failures (see `checked_entries`).
const MODEL_FACING_ENTRIES: [&str; 3] = [
    "processing:model_facing_schema location does not hold choices",
    "processing:model_facing_schema found a choice that is not an object",
    "processing:model_facing_schema found a malformed choice",
];

/// `<processing:collect_offered_events>`'s choice-entry failures (see `checked_entries`).
const COLLECT_ENTRIES: [&str; 3] = [
    "processing:collect_offered_events choices does not hold choices",
    "processing:collect_offered_events found a choice that is not an object",
    "processing:collect_offered_events found a malformed choice",
];

/// `<processing:send>`'s choice-entry failures (see `checked_entries`).
const SEND_ENTRIES: [&str; 3] = [
    "processing:send choices does not hold choices",
    "processing:send found a choice that is not an object",
    "processing:send found a malformed choice",
];

/// The entries of `choices`, each checked to be a choice entry: an object
/// with a non-empty string `event`, a `schema`, optionally a string
/// `description`, a non-empty string `target`, an `effort` (a whole
/// number from 0 to [`MAX_EFFORT`]) and a `draft` object, and nothing else;
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
            ![
                "event",
                "schema",
                "description",
                "target",
                "effort",
                "draft",
            ]
            .contains(&key.as_str())
        }) || entry.get("draft").is_some_and(|draft| !draft.is_object())
            || entry.get("effort").is_some_and(|effort| {
                effort
                    .as_u64()
                    .is_none_or(|effort| effort > u64::from(MAX_EFFORT))
            })
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

    use super::{Attribute, Content, Data, Executed, Failure, ProcessingHost};

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

    impl ProcessingHost for Host {
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
