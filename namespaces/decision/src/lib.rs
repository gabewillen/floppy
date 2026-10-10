//! The `decision` extension namespace: `<decision:choice>`,
//! `<decision:compact>`, `<decision:pin>`, `<decision:assemble>`,
//! `<decision:route>`, `<decision:names>`, `<decision:selection>` and
//! `<decision:tuner>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! Executable content is synchronous and quick, so this namespace holds only
//! the half of a decision that is: reading, compacting, assembling and
//! naming the choices, placing the selections, building a model's strict
//! selection schema, and tuning the confidence threshold. Asking a backend to
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
//! # `<decision:pin location="L" event="E" property="P" values="X"/>`
//!
//! Pins one property of one event's schema in the choice array at
//! `location` (mosfet grounds `bot.focus_device`'s `device` in the turn's
//! focus candidates): `values`, an expression that evaluates to a JSON
//! array, becomes the `enum` of `schema.properties.<P>` of every entry whose
//! event is `E`; when the array is empty those entries are removed instead
//! (nothing could fill them). The result is canonical JSON. Every attribute
//! is required. An entry for `E` whose schema has no object
//! `properties.<P>` is `error.execution`.
//!
//! # `<decision:assemble location="L" choices="X" offers="Y" bound="N" overlocation="O" sizelocation="S"/>`
//!
//! A turn's choices (mosfet `collect_offered_events` and the body's
//! dispatch inventory): the choices `choices` evaluates to (a document's
//! own, kept untagged), then the choices of each member of the object
//! `offers` evaluates to (offering invoke id → its latest offered choices),
//! in member-name order, each tagged with the member's name as `target`.
//! They are compacted as `<decision:compact>` does, with descriptions when
//! that fits `bound` (default [`DEFAULT_COMPACT_BOUND`]) and without
//! otherwise; unlike `<decision:compact>`, a list still over the bound is
//! kept (the document logs it and fails the turn) and `overlocation`
//! (optional) receives `true`. `sizelocation` (optional) receives the
//! assigned list's size in bytes. `overlocation` and `sizelocation` are
//! assigned before `location`.
//!
//! # `<decision:route location="L" rejectionlocation="R" selections="X" choices="Y"/>`
//!
//! Places every selection (`{event, data?, target?}`) of the array
//! `selections` evaluates to by the choices `choices` evaluates to (each
//! choice's event is offered by its `target`, a choice without one by the
//! document itself), in order, as mosfet's `dispatch_selected_events`
//! resolves its targets (`fill_unique_selection_targets`, the offered-event
//! check and `_resolve_target`): a selection that names a `target` goes to
//! it when it offers the event; one that names none goes to the event's
//! only offerer. A selection that cannot be placed fails all of them
//! (mosfet's `SelectionRejectionError`, raised before anything is
//! dispatched): `rejectionlocation` receives the first one's message,
//! mosfet's `Processing selected unavailable event: <event>.` (an event
//! nobody offers, or one that names no target and is offered by several)
//! or `Processing selected unavailable event for target <target>:
//! <event>.` (a named target that does not offer it; also an event nobody
//! offers, when it names one), or `Processing selected a malformed event
//! selection.` for one that is not an object with a non-empty string
//! `event` (floppy's text: mosfet refuses it while parsing), and
//! `location` receives `[]`. Otherwise `rejectionlocation` receives `null`
//! and `location` one `{event, data, target}` per selection (`data` `{}`
//! when the selection has none or `null`; `target` `null` for the document
//! itself), canonical JSON. `rejectionlocation` is assigned before
//! `location`. The document delivers each placed selection with a visible
//! `<send>` (this element never sends).
//!
//! # `<decision:names location="L" choices="X"/>`
//!
//! `location` receives the names of the choices `choices` evaluates to,
//! space separated: `event`, or `event@target` for a choice with a target
//! (what documents log of their choices).
//!
//! # `<decision:selection location="L" choices="X"/>`
//!
//! `location` receives the strict structured-output schema a model fills
//! to select among the choices `choices` evaluates to (mosfet
//! `_selection_item_branch` and `dispatch_tool`): `{events: [anyOf one
//! branch per offered event]}`, the events in choices order, each offered
//! once. A branch is `{event (a one-value enum), data, reason, target}`:
//! `data` is the first such choice's schema without its top-level
//! `$schema`, `title` and `examples`, with `type` `object` and
//! `additionalProperties` `false` when it names neither (an empty object
//! for a schema that is not an object); `reason` is a short string; the
//! branch's `description` is the choice's, else the event name; `target`,
//! an enum of the distinct targets offering the event in choices order, is
//! present (and required) only when some choice of the event has a target.
//! Every field is required and no other property is admitted, at the
//! branch and at the top. The result is canonical JSON.
//!
//! # `<decision:tuner location="L" thresholdlocation="T" observe="X"/>`
//!
//! mosfet's `ConfidenceTuner` on its 0–100 scale, with its defaults (floor
//! [`TUNER_FLOOR`], EMA alpha [`TUNER_ALPHA`], `k_sigma` [`TUNER_K_SIGMA`],
//! warmup [`TUNER_WARMUP`] observations, variance floor
//! [`TUNER_VAR_FLOOR`]). `location` (required) holds the tuner state
//! `{mean, var, n}` (numbers, `n` a non-negative integer); anything
//! else there is replaced by the initial state `{mean:`
//! [`TUNER_MEAN`]`, var:` [`TUNER_VAR_FLOOR`]`, n: 0}`. `observe`
//! (optional) is an expression of a reported confidence, a number on the
//! 0–100 scale (clamped to it), which the state observes: the first sets
//! the mean, later ones move the mean and variance by the EMA (variance
//! never under the floor). `thresholdlocation` (optional) then receives
//! the threshold as a probability (the 0–100 threshold divided by 100):
//! the floor during the warmup, else `max(floor / 2, mean - k_sigma *
//! sqrt(max(variance, variance floor)))`. `location` always receives the
//! resulting state, after `thresholdlocation`. An `observe` that is not a
//! number is `error.execution`.
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

/// The tuner's absolute confidence floor (mosfet `_DEFAULT_CONFIDENCE_FLOOR`).
pub const TUNER_FLOOR: f64 = 35.0;

/// The tuner's EMA weight (mosfet `_DEFAULT_TUNER_ALPHA`).
pub const TUNER_ALPHA: f64 = 0.08;

/// The tuner's standard deviations under the mean (mosfet
/// `_DEFAULT_TUNER_K_SIGMA`).
pub const TUNER_K_SIGMA: f64 = 1.0;

/// Observations before the tuner leaves its floor (mosfet
/// `_DEFAULT_TUNER_WARMUP`).
pub const TUNER_WARMUP: u64 = 8;

/// The tuner's variance floor and initial variance (mosfet
/// `_DEFAULT_TUNER_VAR_FLOOR`).
pub const TUNER_VAR_FLOOR: f64 = 100.0;

/// The tuner's initial mean (mosfet `_DEFAULT_TUNER_MEAN`).
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
        "pin" => pin(attributes, content, host),
        "assemble" => assemble(attributes, content, host),
        "route" => route(attributes, content, host),
        "names" => names(attributes, content, host),
        "selection" => selection(attributes, content, host),
        "tuner" => tuner(attributes, content, host),
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

/// A decimal `bound` attribute, [`DEFAULT_COMPACT_BOUND`] when absent.
fn bound_value<E>(value: Option<&str>, malformed: &'static str) -> Result<usize, Failure<E>> {
    value.map_or(Ok(DEFAULT_COMPACT_BOUND), |value| {
        let value = value.trim();
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(Failure::Execution(malformed));
        }
        value
            .parse::<usize>()
            .map_err(|_| Failure::Execution(malformed))
    })
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

/// Executes `<decision:assemble>`.
fn assemble<H: ChoiceHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, own, offers, bound, over_location, size_location] = attribute_values(
        attributes,
        [
            "location",
            "choices",
            "offers",
            "bound",
            "overlocation",
            "sizelocation",
        ],
        "decision:assemble takes only the location, choices, offers, bound, overlocation and sizelocation attributes",
        "decision:assemble repeats an attribute",
    )?;
    let location = required(location, "decision:assemble requires a non-empty location")?;
    let own = required(own, "decision:assemble requires a non-empty choices")?;
    let offers = required(offers, "decision:assemble requires a non-empty offers")?;
    let bound = bound_value(bound, "decision:assemble bound is not a decimal integer")?;
    let over_location = optional(over_location, "decision:assemble overlocation is empty")?;
    let size_location = optional(size_location, "decision:assemble sizelocation is empty")?;
    no_content(content, "decision:assemble holds no content")?;
    let own = read_json(host, own, "decision:assemble choices does not hold choices")?;
    let offers = read_json(
        host,
        offers,
        "decision:assemble offers does not hold offers",
    )?;
    let assembled = assemble_choices(&own, &offers, bound).map_err(Failure::Execution)?;
    if let Some(over_location) = over_location {
        assign_to(
            host,
            over_location,
            &Data::Bool(assembled.over),
            "decision:assemble could not assign overlocation",
        )?;
    }
    if let Some(size_location) = size_location {
        let size = u32::try_from(assembled.choices.len())
            .ok()
            .and_then(|size| scxml_types::FiniteNumber::new(f64::from(size)).ok())
            .ok_or(Failure::Execution(
                "decision:assemble choices are too large",
            ))?;
        assign_to(
            host,
            size_location,
            &Data::Number(size),
            "decision:assemble could not assign sizelocation",
        )?;
    }
    assign_to(
        host,
        location,
        &Data::JsonArray(assembled.choices),
        "decision:assemble could not assign the choices",
    )?;
    Ok(Executed::Handled)
}

/// A turn's assembled choices (see [`assemble_choices`]).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Assembled {
    /// The compacted choices, canonical JSON.
    pub choices: String,
    /// Whether the choices are over the bound even without descriptions.
    pub over: bool,
}

/// The choices `own` (kept as they are, untagged unless they carry a
/// target) followed by each `offers` member's choices tagged with its
/// name as `target`, in name order; compacted like `<decision:compact>`,
/// except that over the bound without descriptions the description-less
/// list is kept and `over` is set.
///
/// # Errors
///
/// A static detail when `own` is not an array of choice entries or `offers`
/// is not an object of such arrays.
pub fn assemble_choices(
    own: &Json,
    offers: &Json,
    bound: usize,
) -> Result<Assembled, &'static str> {
    let mut all: Vec<Json> = checked_entries(own, ASSEMBLE_ENTRIES)?.to_vec();
    let offers = offers
        .as_object()
        .ok_or("decision:assemble offers does not hold offers")?;
    let mut names: Vec<&String> = offers.keys().collect();
    names.sort_unstable();
    for name in names {
        if name.is_empty() {
            return Err("decision:assemble offers names an empty target");
        }
        let offered = &offers[name.as_str()];
        if !offered.is_array() {
            return Err("decision:assemble offers does not hold offers");
        }
        for entry in checked_entries(offered, ASSEMBLE_ENTRIES)? {
            let mut entry = entry.clone();
            if let Some(entry) = entry.as_object_mut() {
                entry.insert("target".into(), Json::String(name.clone()));
            }
            all.push(entry);
        }
    }
    let mut compacted = String::new();
    let mut over = false;
    for descriptions in [true, false] {
        compacted = canonical_json(&Json::Array(
            all.iter()
                .map(|entry| compact_entry(entry, descriptions))
                .collect(),
        ));
        over = compacted.len() > bound;
        if !over {
            break;
        }
    }
    Ok(Assembled {
        choices: compacted,
        over,
    })
}

/// Executes `<decision:route>`.
fn route<H: ChoiceHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, rejection_location, selections, choices] = attribute_values(
        attributes,
        ["location", "rejectionlocation", "selections", "choices"],
        "decision:route takes only the location, rejectionlocation, selections and choices attributes",
        "decision:route repeats an attribute",
    )?;
    let location = required(location, "decision:route requires a non-empty location")?;
    let rejection_location = required(
        rejection_location,
        "decision:route requires a non-empty rejectionlocation",
    )?;
    let selections = required(selections, "decision:route requires a non-empty selections")?;
    let choices = required(choices, "decision:route requires a non-empty choices")?;
    no_content(content, "decision:route holds no content")?;
    let selections = read_json(
        host,
        selections,
        "decision:route selections is not an array",
    )?;
    let choices = read_json(
        host,
        choices,
        "decision:route choices does not hold choices",
    )?;
    let routed = route_selections(&selections, &choices).map_err(Failure::Execution)?;
    let (placed, rejection) = match routed {
        Ok(placed) => (placed, Data::Null),
        Err(rejection) => ("[]".to_owned(), Data::String(rejection)),
    };
    assign_to(
        host,
        rejection_location,
        &rejection,
        "decision:route could not assign rejectionlocation",
    )?;
    assign_to(
        host,
        location,
        &Data::JsonArray(placed),
        "decision:route could not assign the placed selections",
    )?;
    Ok(Executed::Handled)
}

/// mosfet `_unavailable_message`.
fn unavailable(event: &str, target: Option<&str>) -> String {
    match target {
        None => format!("Processing selected unavailable event: {event}."),
        Some(target) => {
            format!("Processing selected unavailable event for target {target}: {event}.")
        }
    }
}

/// Every selection of `selections`, in order, placed by the offerers of
/// `choices`: `Ok` with the canonical JSON of the `{event, data, target}`
/// entries (`target` `null` is the document itself; `data` `{}` when the
/// selection has none), or `Err` with the first rejection's message. See
/// the crate documentation (`<decision:route>`).
///
/// # Errors
///
/// A static detail when `selections` is not an array or `choices` does not
/// hold choices.
pub fn route_selections(
    selections: &Json,
    choices: &Json,
) -> Result<Result<String, String>, &'static str> {
    let selections = selections
        .as_array()
        .ok_or("decision:route selections is not an array")?;
    let mut offerers: Map<String, Json> = Map::new();
    for entry in checked_entries(choices, ROUTE_ENTRIES)? {
        let event = entry
            .get("event")
            .and_then(Json::as_str)
            .unwrap_or_default();
        let target = entry.get("target").cloned().unwrap_or(Json::Null);
        if let Json::Array(targets) = offerers
            .entry(event.to_owned())
            .or_insert_with(|| Json::Array(Vec::new()))
            && !targets.contains(&target)
        {
            targets.push(target);
        }
    }
    let mut placed = Vec::with_capacity(selections.len());
    for selection in selections {
        let Some(event) = selection
            .get("event")
            .and_then(Json::as_str)
            .filter(|event| !event.is_empty())
        else {
            return Ok(Err(
                "Processing selected a malformed event selection.".to_owned()
            ));
        };
        let named = selection.get("target").and_then(Json::as_str);
        let Some(targets) = offerers.get(event).and_then(Json::as_array) else {
            return Ok(Err(unavailable(event, named)));
        };
        let target = match named {
            Some(named) if targets.iter().any(|target| target.as_str() == Some(named)) => {
                Json::String(named.to_owned())
            }
            Some(named) => return Ok(Err(unavailable(event, Some(named)))),
            None => match targets.as_slice() {
                [target] => target.clone(),
                _ => return Ok(Err(unavailable(event, None))),
            },
        };
        let data = match selection.get("data") {
            None | Some(Json::Null) => Json::Object(Map::new()),
            Some(data) => data.clone(),
        };
        let mut out = Map::new();
        out.insert("event".into(), Json::String(event.to_owned()));
        out.insert("data".into(), data);
        out.insert("target".into(), target);
        placed.push(Json::Object(out));
    }
    Ok(Ok(canonical_json(&Json::Array(placed))))
}

/// Executes `<decision:names>`.
fn names<H: ChoiceHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, choices] = attribute_values(
        attributes,
        ["location", "choices"],
        "decision:names takes only the location and choices attributes",
        "decision:names repeats an attribute",
    )?;
    let location = required(location, "decision:names requires a non-empty location")?;
    let choices = required(choices, "decision:names requires a non-empty choices")?;
    no_content(content, "decision:names holds no content")?;
    let choices = read_json(
        host,
        choices,
        "decision:names choices does not hold choices",
    )?;
    let names = choice_names(&choices).map_err(Failure::Execution)?;
    assign_to(
        host,
        location,
        &Data::String(names),
        "decision:names could not assign the names",
    )?;
    Ok(Executed::Handled)
}

/// The choices' names, space separated: `event`, or `event@target` for a
/// choice with a target.
///
/// # Errors
///
/// A static detail when `choices` is not an array of choice entries.
pub fn choice_names(choices: &Json) -> Result<String, &'static str> {
    Ok(checked_entries(choices, NAMES_ENTRIES)?
        .iter()
        .map(|entry| {
            let event = entry
                .get("event")
                .and_then(Json::as_str)
                .unwrap_or_default();
            entry
                .get("target")
                .and_then(Json::as_str)
                .map_or_else(|| event.to_owned(), |target| format!("{event}@{target}"))
        })
        .collect::<Vec<_>>()
        .join(" "))
}

/// Executes `<decision:selection>`.
fn selection<H: ChoiceHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, choices] = attribute_values(
        attributes,
        ["location", "choices"],
        "decision:selection takes only the location and choices attributes",
        "decision:selection repeats an attribute",
    )?;
    let location = required(location, "decision:selection requires a non-empty location")?;
    let choices = required(choices, "decision:selection requires a non-empty choices")?;
    no_content(content, "decision:selection holds no content")?;
    let choices = read_json(
        host,
        choices,
        "decision:selection choices does not hold choices",
    )?;
    let schema = selection_schema(&choices).map_err(Failure::Execution)?;
    assign_to(
        host,
        location,
        &Data::JsonObject(schema),
        "decision:selection could not assign the schema",
    )?;
    Ok(Executed::Handled)
}

/// The strict selection schema of `choices`, canonical JSON: see the crate
/// documentation (`<decision:selection>`).
///
/// # Errors
///
/// A static detail when `choices` is not an array of choice entries.
pub fn selection_schema(choices: &Json) -> Result<String, &'static str> {
    // Each offered event once, in choices order: its first choice and the
    // distinct targets offering it.
    let mut offered: Vec<(&Json, Vec<Json>)> = Vec::new();
    for entry in checked_entries(choices, SELECTION_ENTRIES)? {
        let event = entry.get("event");
        let index = offered
            .iter()
            .position(|(first, _)| first.get("event") == event)
            .unwrap_or_else(|| {
                offered.push((entry, Vec::new()));
                offered.len() - 1
            });
        if let Some(target) = entry.get("target").filter(|target| target.is_string()) {
            let targets = &mut offered[index].1;
            if !targets.contains(target) {
                targets.push(target.clone());
            }
        }
    }
    let branches: Vec<Json> = offered
        .into_iter()
        .map(|(choice, targets)| {
            let event = choice.get("event").cloned().unwrap_or(Json::Null);
            let description = choice
                .get("description")
                .filter(|description| description.is_string())
                .cloned()
                .unwrap_or_else(|| event.clone());
            let mut properties = Map::new();
            properties.insert(
                "event".into(),
                serde_json::json!({"type": "string", "enum": [event]}),
            );
            properties.insert(
                "data".into(),
                selection_data(choice.get("schema").unwrap_or(&Json::Null)),
            );
            properties.insert(
                "reason".into(),
                serde_json::json!({"type": "string", "description": "A short reason this event was selected."}),
            );
            let mut required = vec![
                Json::from("event"),
                Json::from("data"),
                Json::from("reason"),
            ];
            if !targets.is_empty() {
                properties.insert(
                    "target".into(),
                    serde_json::json!({"type": "string", "enum": targets, "description": "The ability that receives this event."}),
                );
                required.push("target".into());
            }
            serde_json::json!({
                "type": "object",
                "description": description,
                "properties": properties,
                "required": required,
                "additionalProperties": false,
            })
        })
        .collect();
    Ok(canonical_json(&serde_json::json!({
        "type": "object",
        "properties": {"events": {
            "type": "array",
            "description": "All events necessary for the input this turn; an empty array selects none.",
            "items": {"anyOf": branches},
        }},
        "required": ["events"],
        "additionalProperties": false,
    })))
}

/// A choice's schema as a selection branch's `data`: the payload contract
/// only (top-level `$schema`, `title` and `examples` dropped), an object
/// admitting no undeclared property unless it says otherwise.
fn selection_data(schema: &Json) -> Json {
    let mut out: Map<String, Json> = schema
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| !["$schema", "title", "examples"].contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    out.entry("type").or_insert_with(|| "object".into());
    out.entry("additionalProperties")
        .or_insert(Json::Bool(false));
    Json::Object(out)
}

/// Executes `<decision:tuner>`.
fn tuner<H: ChoiceHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, threshold_location, observe] = attribute_values(
        attributes,
        ["location", "thresholdlocation", "observe"],
        "decision:tuner takes only the location, thresholdlocation and observe attributes",
        "decision:tuner repeats an attribute",
    )?;
    let location = required(location, "decision:tuner requires a non-empty location")?;
    let threshold_location = optional(
        threshold_location,
        "decision:tuner thresholdlocation is empty",
    )?;
    let observe = optional(observe, "decision:tuner observe is empty")?;
    no_content(content, "decision:tuner holds no content")?;
    let state = match host.read(location).map_err(Failure::Host)? {
        Ok(Data::JsonObject(text)) => serde_json::from_str(&text)
            .ok()
            .as_ref()
            .and_then(Tuner::from_json),
        _ => None,
    }
    .unwrap_or_default();
    let confidence = observe
        .map(|observe| match host.read(observe).map_err(Failure::Host)? {
            Ok(Data::Number(value)) => Ok(value.get()),
            _ => Err(Failure::Execution("decision:tuner observe is not a number")),
        })
        .transpose()?;
    let state = confidence.map_or(state, |confidence| state.observed(confidence));
    if let Some(threshold_location) = threshold_location {
        let threshold = scxml_types::FiniteNumber::new(state.threshold() / 100.0)
            .map_err(|_| Failure::Execution("decision:tuner threshold is not finite"))?;
        assign_to(
            host,
            threshold_location,
            &Data::Number(threshold),
            "decision:tuner could not assign thresholdlocation",
        )?;
    }
    assign_to(
        host,
        location,
        &Data::JsonObject(canonical_json(&state.to_json())),
        "decision:tuner could not assign the tuner state",
    )?;
    Ok(Executed::Handled)
}

/// mosfet's `ConfidenceTuner` state on the 0–100 scale: see the crate
/// documentation (`<decision:tuner>`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tuner {
    /// The running mean of the observed confidences.
    pub mean: f64,
    /// Their running variance.
    pub var: f64,
    /// How many were observed.
    pub n: u64,
}

impl Default for Tuner {
    fn default() -> Self {
        Self {
            mean: TUNER_MEAN,
            var: TUNER_VAR_FLOOR,
            n: 0,
        }
    }
}

impl Tuner {
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

    /// The state after observing `confidence` (clamped to 0–100).
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
            n: self.n + 1,
        }
    }

    /// The escalation threshold on the 0–100 scale.
    #[must_use]
    pub fn threshold(self) -> f64 {
        if self.n < TUNER_WARMUP {
            return TUNER_FLOOR;
        }
        let std = self.var.max(TUNER_VAR_FLOOR).sqrt();
        (0.5 * TUNER_FLOOR).max(self.mean - TUNER_K_SIGMA * std)
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

/// `<decision:selection>`'s choice-entry failures (see `checked_entries`).
const SELECTION_ENTRIES: [&str; 3] = [
    "decision:selection choices does not hold choices",
    "decision:selection found a choice that is not an object",
    "decision:selection found a malformed choice",
];

/// `<decision:compact>`'s choice-entry failures (see `checked_entries`).
const COMPACT_ENTRIES: [&str; 3] = [
    "decision:compact location does not hold choices",
    "decision:compact found a choice that is not an object",
    "decision:compact found a malformed choice",
];

/// `<decision:pin>`'s choice-entry failures (see `checked_entries`).
const PIN_ENTRIES: [&str; 3] = [
    "decision:pin location does not hold choices",
    "decision:pin found a choice that is not an object",
    "decision:pin found a malformed choice",
];

/// `<decision:assemble>`'s choice-entry failures (see `checked_entries`).
const ASSEMBLE_ENTRIES: [&str; 3] = [
    "decision:assemble choices does not hold choices",
    "decision:assemble found a choice that is not an object",
    "decision:assemble found a malformed choice",
];

/// `<decision:route>`'s choice-entry failures (see `checked_entries`).
const ROUTE_ENTRIES: [&str; 3] = [
    "decision:route choices does not hold choices",
    "decision:route found a choice that is not an object",
    "decision:route found a malformed choice",
];

/// `<decision:names>`'s choice-entry failures (see `checked_entries`).
const NAMES_ENTRIES: [&str; 3] = [
    "decision:names choices does not hold choices",
    "decision:names found a choice that is not an object",
    "decision:names found a malformed choice",
];

/// The entries of `choices`, each checked to be a choice entry: an object
/// with a non-empty string `event`, a `schema`, optionally a string
/// `description` and a non-empty string `target`, and nothing else;
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
        if entry
            .keys()
            .any(|key| !["event", "schema", "description", "target"].contains(&key.as_str()))
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
    let entries = checked_entries(choices, COMPACT_ENTRIES)?;
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
