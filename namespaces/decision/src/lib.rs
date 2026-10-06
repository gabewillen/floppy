//! The `decision` extension namespace: `<decision:rank>`,
//! `<decision:classify>`, and `<decision:choose>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! Provider-neutral decisions: each element asks a backend one batch of
//! labelled choice questions about one state document and acts on the
//! probability distributions it answers. The backend is a [`Backend`]
//! ([`TypeSafe`] `system_one` today); its key and base URL reach the request
//! only through the host's `env` slots. Every decision is computed from the
//! distributions alone ([`backend::argmax`]: highest probability, ties broken
//! by the label's UTF-8 bytes), and every request body is canonical JSON, so
//! one element, one data model, and one session snapshot always send the
//! same bytes and take the same decision from the same answer. Documents
//! bind [`NAMESPACE_URI`] (conventionally with the prefix `decision`).
//!
//! # Common attributes
//!
//! * `statexpr="E"` (optional): the state document the backend decides
//!   about; it must evaluate to a string, a JSON object, or a JSON array.
//!   Without it the state is the executing session's snapshot root
//!   (`session-snapshot-host`, with `transitions`, `queues`, and
//!   `pending-sends` excluded): its configuration, system variables, and
//!   data model.
//! * `instructions="S"` or `instructionsexpr="E"` (optional, at most one, a
//!   non-empty string): what to decide; each element has a default.
//! * `model="M"` or `modelexpr="E"` (optional, at most one, a non-empty
//!   string): the backend's model; the backend's default without one.
//! * the host's `env` slots: `env:apikey="NAME"` (required at run time; sent
//!   as a bearer token) and `env:baseurl="NAME"` (required at run time: the
//!   API base URL, for example `https://api.typesafe.ai`; must not end with
//!   `/`), or their `expr` forms. No backend URL is compiled in: an element
//!   whose `baseurl` slot holds no value fails like one with no key.
//!
//! Every other attribute, any child element, and any non-white-space text
//! is refused.
//!
//! # `<decision:rank>`
//!
//! `location="L"` (required) and `candidatesexpr="E"` (required: a non-empty
//! JSON array of distinct candidates). One question offers every candidate
//! (a string is its own label; any other value is labelled by its canonical
//! JSON, and an object or array is also its description). `L` receives a
//! JSON array of `{"candidate": C, "score": P}`, every candidate once, by
//! score descending, ties by label.
//!
//! # `<decision:classify>`
//!
//! `location="L"` (required) and exactly one of `labels="a b c"` (XML white
//! space separated) or `labelsexpr="E"` (a JSON array of strings, or a JSON
//! object mapping each label to its description: a string, object, array,
//! or `null`). Labels are non-empty and distinct. `L` receives a JSON object
//! mapping every label to its probability.
//!
//! # `<decision:choose>`
//!
//! Chooses one event, fills its data to its schema, validates it, and
//! raises it.
//!
//! * **The menu.** With `candidatesexpr="E"` (a JSON array of objects with
//!   `event`, `schema`, and an optional `description` string), those events.
//!   Without it, the transitions the session snapshot lists for the current
//!   configuration (`transitions` section, in its walk order) that carry an
//!   `event:schema`: each of their event descriptors that names an event
//!   (`*` is skipped; `e.*` names `e`), unless a transition listed earlier
//!   has a descriptor that matches the same event (that one would be
//!   selected for it). Conditions are never evaluated. `event:schemaexpr`
//!   transitions are not offered (a snapshot carries the expression, not
//!   the schema). The description is the schema's `description`, else its
//!   `title` (first line).
//! * **Grounded data.** The filled data is a JSON object holding the
//!   schema's top-level `required` members, each chosen by its own question
//!   among grounded options, never authored: the evidence's member of that
//!   name (the evidence is `dataexpr="E"`, a JSON object, else the state when
//!   `statexpr` gives an object, else none), then the property's `enum`
//!   values, then its `const`. An event with a required member that has no
//!   option is not offered.
//! * **The decision.** One batch asks the selection (every offered event,
//!   plus [`ABSTAIN_LABEL`]) and every offered event's member questions.
//!   When the menu is empty, the abstain label wins, or the chosen event's
//!   probability is below `threshold="P"` (optional, in `[0, 1]`, `0` without
//!   it), the element raises [`ABSTAIN_EVENT`] instead, with data
//!   `{"reason": "no-candidates" | "declined" | "below-threshold", "confidence": P}`
//!   (`confidence` absent for `no-candidates`; an empty menu sends nothing).
//! * **Validation before raise.** The filled data must satisfy the chosen
//!   event's compiled schema; otherwise nothing is raised (`error.execution`).
//!
//! # Failures
//!
//! Every malformed element, evaluation, missing key, refused or failed
//! request, non-2xx status, malformed answer, unsatisfied schema, or failed
//! assignment or raise is an `execution` error with a static detail (SCXML
//! `error.execution`); nothing is assigned or raised then. The state, the
//! labels, the answer, and the key are never echoed.
//!
//! # Bounds
//!
//! Candidates and labels ([`MAX_CRITERIA`], [`MAX_LABEL_BYTES`]), grounded
//! options ([`MAX_OPTIONS`]), required members ([`MAX_REQUIRED`]), questions
//! ([`MAX_QUESTIONS`]), the state ([`MAX_STATE_BYTES`]), the instructions
//! ([`MAX_INSTRUCTIONS_BYTES`]), and the model ([`MAX_MODEL_BYTES`]) are
//! bounded on wasm builds only ([`scxml_types::wasm_bound`]); schemas are
//! bounded by `scxml-event-schema`, and the answer by the host's
//! response-body bound.
//!
//! # Telemetry
//!
//! Each entry point (`initialize`, `execute`, `shutdown`) records one
//! observation through scxml.rs's `telemetry` WIT facade: its closed
//! operation, its closed outcome (`ok`, `rejected`, `failed`), and one closed
//! signal event. No attribute carries the state, a label, an event name, a
//! location, or a URL, so the cardinality is the product of the three closed
//! enums. The host adds one `extension-http-fetch` span per request and one
//! `session-snapshot` span per snapshot read, each with only its closed
//! outcome.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

pub mod backend;

use std::collections::{BTreeMap, BTreeSet};

use scxml_event_schema::{CompiledSchema, canonical_json, sha256_hex};
use scxml_types::{DataValue as Data, exceeds_wasm_bound, wasm_bound};
use serde_json::{Map, Value as Json};

pub use backend::{Backend, Batch, Criterion, Distribution, Question, TypeSafe};

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "decision";

/// The namespace's repository namespace URI without its `@<tag>` suffix.
pub const NAMESPACE_URI_BASE: &str = "https://github.com/stateforward/nemobot/namespaces/decision";

/// The release tag the namespace URI names (the placeholder of nemobot's
/// first release, as for every nemobot namespace).
pub const NAMESPACE_TAG: &str = "v0.1.0";

/// The exact namespace URI documents bind: `{NAMESPACE_URI_BASE}@{NAMESPACE_TAG}`.
pub const NAMESPACE_URI: &str =
    "https://github.com/stateforward/nemobot/namespaces/decision@v0.1.0";

/// The `env` slot of the API key (`env:apikey`).
pub const API_KEY_SLOT: &str = "apikey";
/// The `env` slot of the API base URL (`env:baseurl`).
pub const BASE_URL_SLOT: &str = "baseurl";

/// The event `<decision:choose>` raises when it takes no event.
pub const ABSTAIN_EVENT: &str = "decision.abstain";
/// The selection's reserved criterion: no event fits. It holds spaces, so
/// it is never an event name.
pub const ABSTAIN_LABEL: &str = "none of these";

/// The selection question's name in a `<decision:choose>` batch.
pub const SELECTION_QUESTION: &str = "selection";

const RANK_INSTRUCTIONS: &str =
    "Rank the candidates by how well each fits the state: give the best fit the most probability.";
const CLASSIFY_INSTRUCTIONS: &str = "Classify the state: choose the label that describes it.";
const CHOOSE_INSTRUCTIONS: &str = "Choose the single event the machine should take in this state: each criterion names an event it can take. Choose \"none of these\" when no event fits.";
const ABSTAIN_DESCRIPTION: &str = "None of these: take no event.";
const FILL_INSTRUCTIONS: &str =
    "The criteria are the exact values the evidence offers. Pick the value the state supports.";

/// Upper bound on one question's criteria (wasm builds only).
pub const MAX_CRITERIA: usize = wasm_bound(256);
/// Upper bound on one label's bytes (wasm builds only).
pub const MAX_LABEL_BYTES: usize = wasm_bound(1024);
/// Upper bound on the grounded options of one required member (wasm builds
/// only).
pub const MAX_OPTIONS: usize = wasm_bound(64);
/// Upper bound on the required members of one event's schema (wasm builds
/// only).
pub const MAX_REQUIRED: usize = wasm_bound(64);
/// Upper bound on the questions of one batch (wasm builds only).
pub const MAX_QUESTIONS: usize = wasm_bound(1024);
/// Upper bound on the canonical JSON bytes of the state (wasm builds only).
pub const MAX_STATE_BYTES: usize = wasm_bound(512 * 1024);
/// Upper bound on the instructions' bytes (wasm builds only).
pub const MAX_INSTRUCTIONS_BYTES: usize = wasm_bound(16 * 1024);
/// Upper bound on the model name's bytes (wasm builds only).
pub const MAX_MODEL_BYTES: usize = wasm_bound(256);
/// Upper bound on an explicit candidate's event name bytes.
pub const MAX_EVENT_NAME_BYTES: usize = 256;

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

/// A value the data model evaluated.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `null`.
    Null,
    /// A boolean.
    Bool(bool),
    /// A finite number.
    Number(f64),
    /// A string.
    Text(String),
    /// A JSON object or array, as JSON text.
    Json(String),
    /// Any other value.
    Other,
}

/// One part of a URL or header value: literal text, or an opaque reference
/// the host substitutes with the value it holds for one slot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Part {
    /// Literal text.
    Literal(String),
    /// A slot's reference.
    Reference(u32),
}

/// One `POST` request to the backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    /// The absolute target URL, in parts.
    pub url: Vec<Part>,
    /// Header fields, in order, their values in parts.
    pub headers: Vec<(String, Vec<Part>)>,
    /// The canonical JSON body.
    pub body: Vec<u8>,
}

/// The backend's answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Response {
    /// HTTP status.
    pub status: u16,
    /// Body bytes.
    pub body: Vec<u8>,
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

/// The snapshot read for the default state.
pub const STATE_SNAPSHOT_EXCLUDE: [Section; 3] =
    [Section::Queues, Section::PendingSends, Section::Transitions];
/// The snapshot read for the menu: only its transitions are used.
pub const MENU_SNAPSHOT_EXCLUDE: [Section; 4] = [
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

/// The host capabilities one `execute` call uses.
pub trait DecisionHost {
    /// A non-recoverable host failure.
    type Error;

    /// Evaluates one data-model expression; `Ok(Err(()))` is a recoverable
    /// evaluation failure.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn evaluate(&mut self, expression: &str) -> Result<Result<Value, ()>, Self::Error>;

    /// Assigns one value to one data-model location; `Ok(Err(()))` is a
    /// recoverable assignment failure.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn assign(&mut self, location: &str, value: &Data) -> Result<Result<(), ()>, Self::Error>;

    /// The reference the host bound to one of the element's `env` slots.
    fn slot(&mut self, slot: &str) -> Option<u32>;

    /// Performs one `POST`; `Ok(Err(()))` means the host refused it or the
    /// transport failed.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn post(&mut self, request: &Request) -> Result<Result<Response, ()>, Self::Error>;

    /// The executing session's `scxml.session-snapshot/v2` document with
    /// `exclude` omitted; `Ok(Err(()))` when the host offers none.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn snapshot(&mut self, exclude: &[Section]) -> Result<Result<String, ()>, Self::Error>;

    /// Queues one internal event on the session; `Ok(Err(()))` when the host
    /// refused the event.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn raise(&mut self, name: &str, data: &Data) -> Result<Result<(), ()>, Self::Error>;
}

/// Executes one element with the [`TypeSafe`] backend.
///
/// # Errors
///
/// [`Failure::Execution`] for every document, evaluation, configuration,
/// transport, answer, schema, assignment, or raise failure;
/// [`Failure::Host`] for a non-recoverable host failure.
pub fn execute<H: DecisionHost>(
    local_name: &str,
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    execute_with(&TypeSafe, local_name, attributes, content, host)
}

/// Executes one element with the given backend.
///
/// # Errors
///
/// As [`execute`].
pub fn execute_with<B: Backend, H: DecisionHost>(
    backend: &B,
    local_name: &str,
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let allowed: &[&str] = match local_name {
        "rank" => &["location", "candidatesexpr"],
        "classify" => &["location", "labels", "labelsexpr"],
        "choose" => &["candidatesexpr", "dataexpr", "threshold"],
        _ => return Ok(Executed::NotHandled),
    };
    let parameters = Parameters::parse(attributes, allowed).map_err(Failure::Execution)?;
    no_content(content).map_err(Failure::Execution)?;
    let mut decider = Context {
        backend,
        host,
        parameters: &parameters,
    };
    match local_name {
        "rank" => decider.rank()?,
        "classify" => decider.classify()?,
        _ => decider.choose()?,
    }
    Ok(Executed::Handled)
}

const COMMON_ATTRIBUTES: [&str; 5] = [
    "statexpr",
    "instructions",
    "instructionsexpr",
    "model",
    "modelexpr",
];

/// The unqualified attributes of one element.
#[derive(Debug, Default)]
struct Parameters {
    values: BTreeMap<String, String>,
}

impl Parameters {
    fn parse(attributes: &[Attribute], allowed: &[&str]) -> Result<Self, &'static str> {
        let mut values = BTreeMap::new();
        for attribute in attributes {
            match attribute.namespace_uri.as_deref() {
                // The host's slot vocabulary: checked against the manifest
                // by the validator and bound by the host.
                Some(scxml_types::ENV_NAMESPACE_URI) => continue,
                Some(_) => {
                    return Err(
                        "decision elements take no attributes in another namespace than env",
                    );
                }
                None => {}
            }
            let name = attribute.local_name.as_str();
            if !allowed.contains(&name) && !COMMON_ATTRIBUTES.contains(&name) {
                return Err(
                    "a decision element has an attribute this package version does not define",
                );
            }
            if values
                .insert(name.to_owned(), attribute.value.clone())
                .is_some()
            {
                return Err("a decision element repeats an attribute");
            }
        }
        for (one, other) in [
            ("instructions", "instructionsexpr"),
            ("model", "modelexpr"),
            ("labels", "labelsexpr"),
        ] {
            if values.contains_key(one) && values.contains_key(other) {
                return Err(
                    "a decision element takes at most one of an attribute and its expr form",
                );
            }
        }
        Ok(Self { values })
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }
}

fn no_content(content: &[Content]) -> Result<(), &'static str> {
    for item in content {
        match item {
            Content::Text(text) if text.trim().is_empty() => {}
            _ => return Err("decision elements hold no content"),
        }
    }
    Ok(())
}

/// The JSON value of one evaluated value; `None` for a value with no JSON
/// form.
fn json_of(value: Value) -> Option<Json> {
    match value {
        Value::Null => Some(Json::Null),
        Value::Bool(value) => Some(Json::Bool(value)),
        Value::Number(number) => serde_json::Number::from_f64(number).map(Json::Number),
        Value::Text(text) => Some(Json::String(text)),
        Value::Json(text) => serde_json::from_str(&text).ok(),
        Value::Other => None,
    }
}

/// The label of one value: a string is its own label, any other value its
/// canonical JSON.
fn label_of(value: &Json) -> String {
    match value {
        Json::String(text) => text.clone(),
        other => canonical_json(other),
    }
}

fn valid_label(label: &str) -> bool {
    !label.is_empty() && !exceeds_wasm_bound(label.len(), MAX_LABEL_BYTES)
}

/// One event `<decision:choose>` may raise.
#[derive(Debug)]
pub struct Candidate {
    /// The event name.
    pub event: String,
    /// The resolved schema.
    pub schema: Json,
    /// The schema compiled for checking the filled data.
    pub compiled: CompiledSchema,
    /// What the event means, when known.
    pub description: Option<String>,
}

/// A candidate with grounded options for every required member.
struct Grounded<'a> {
    candidate: &'a Candidate,
    /// `(member, options)` in the schema's `required` order.
    members: Vec<(String, Vec<Criterion>, Vec<Json>)>,
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
        && name.len() <= MAX_EVENT_NAME_BYTES
        && !name.contains('*')
        && !name.chars().any(scxml_types::is_xml_space)
        && name.split('.').all(|token| !token.is_empty())
}

/// The menu of a `scxml.session-snapshot/v2` document: see the crate
/// documentation ("The menu").
///
/// # Errors
///
/// A static detail when the document is malformed or a schema does not
/// compile or match its digest.
pub fn snapshot_menu(document: &str) -> Result<Vec<Candidate>, &'static str> {
    const MALFORMED: &str = "the session snapshot is malformed";
    let document: Json = serde_json::from_str(document).map_err(|_| MALFORMED)?;
    let Some(transitions) = document.pointer("/snapshot/root/transitions") else {
        return Ok(Vec::new());
    };
    let transitions = transitions.as_array().ok_or(MALFORMED)?;
    let mut earlier: Vec<&str> = Vec::new();
    let mut menu: Vec<Candidate> = Vec::new();
    for transition in transitions {
        let descriptors: Vec<&str> = transition
            .get("events")
            .and_then(Json::as_array)
            .ok_or(MALFORMED)?
            .iter()
            .map(|descriptor| descriptor.as_str().ok_or(MALFORMED))
            .collect::<Result<_, _>>()?;
        if let Some(event_schema) = transition.get("event_schema")
            && event_schema.get("kind").and_then(Json::as_str) == Some("schema")
        {
            let text = event_schema
                .get("schema")
                .and_then(Json::as_str)
                .ok_or(MALFORMED)?;
            if event_schema.get("sha256").and_then(Json::as_str)
                != Some(sha256_hex(text.as_bytes()).as_str())
            {
                return Err(MALFORMED);
            }
            let schema: Json = serde_json::from_str(text).map_err(|_| MALFORMED)?;
            let compiled = CompiledSchema::new(text)
                .map_err(|_| "a transition's event schema does not compile")?;
            let mut compiled = Some(compiled);
            for descriptor in &descriptors {
                if *descriptor == "*" {
                    continue;
                }
                let event = descriptor.strip_suffix(".*").unwrap_or(descriptor);
                if !valid_event_name(event)
                    || earlier.iter().any(|before| matches(before, event))
                    || menu.iter().any(|candidate| candidate.event == event)
                {
                    continue;
                }
                // One transition's descriptors share its schema; compile it
                // again only for a second offered descriptor.
                let compiled = match compiled.take() {
                    Some(compiled) => compiled,
                    None => CompiledSchema::new(text)
                        .map_err(|_| "a transition's event schema does not compile")?,
                };
                menu.push(Candidate {
                    event: event.to_owned(),
                    description: description_of(&schema),
                    schema: schema.clone(),
                    compiled,
                });
            }
        }
        earlier.extend(descriptors);
    }
    Ok(menu)
}

/// The menu of a `candidatesexpr` value: an array of objects with `event`,
/// `schema`, and an optional `description`.
///
/// # Errors
///
/// A static detail for a malformed candidate, a duplicate event, or a schema
/// that is not a self-contained JSON Schema.
pub fn explicit_menu(value: &Json) -> Result<Vec<Candidate>, &'static str> {
    const INVALID: &str = "decision:choose candidatesexpr must be an array of objects with an event name, a schema, and an optional description";
    let items = value.as_array().ok_or(INVALID)?;
    if exceeds_wasm_bound(items.len(), MAX_CRITERIA) {
        return Err("decision:choose candidatesexpr exceeds the candidate bound");
    }
    let mut menu: Vec<Candidate> = Vec::new();
    for item in items {
        let members = item.as_object().ok_or(INVALID)?;
        if members
            .keys()
            .any(|key| !matches!(key.as_str(), "event" | "schema" | "description"))
        {
            return Err(INVALID);
        }
        let event = members
            .get("event")
            .and_then(Json::as_str)
            .filter(|event| valid_event_name(event))
            .ok_or(INVALID)?;
        if menu.iter().any(|candidate| candidate.event == event) {
            return Err("decision:choose candidatesexpr names an event twice");
        }
        let source = members.get("schema").ok_or(INVALID)?;
        let resolved = scxml_event_schema::resolve(canonical_json(source).as_bytes(), None)
            .map_err(|_| "decision:choose candidate schema is not a self-contained JSON Schema")?;
        let schema: Json = serde_json::from_str(&resolved.schema).map_err(|_| INVALID)?;
        let compiled = CompiledSchema::new(&resolved.schema)
            .map_err(|_| "decision:choose candidate schema is not a self-contained JSON Schema")?;
        let description = match members.get("description") {
            None => description_of(&schema),
            Some(Json::String(text)) => Some(text.clone()),
            Some(_) => return Err(INVALID),
        };
        menu.push(Candidate {
            event: event.to_owned(),
            schema,
            compiled,
            description,
        });
    }
    Ok(menu)
}

/// The grounded options of every required member of `candidate`, or `None`
/// when one has no option (the event is then not offered).
fn ground<'a>(
    candidate: &'a Candidate,
    evidence: Option<&Map<String, Json>>,
) -> Option<Grounded<'a>> {
    let required: Vec<&str> = match candidate.schema.get("required") {
        None => Vec::new(),
        Some(Json::Array(names)) => names.iter().map(Json::as_str).collect::<Option<_>>()?,
        Some(_) => return None,
    };
    if exceeds_wasm_bound(required.len(), MAX_REQUIRED) {
        return None;
    }
    let mut members = Vec::new();
    for member in required {
        let property = candidate
            .schema
            .get("properties")
            .and_then(|properties| properties.get(member));
        let mut values: Vec<Json> = Vec::new();
        let mut push = |value: &Json| {
            if !values.contains(value) {
                values.push(value.clone());
            }
        };
        if let Some(value) = evidence.and_then(|evidence| evidence.get(member)) {
            push(value);
        }
        if let Some(Json::Array(options)) = property.and_then(|property| property.get("enum")) {
            options.iter().for_each(&mut push);
        }
        if let Some(value) = property.and_then(|property| property.get("const")) {
            push(value);
        }
        if values.is_empty() || exceeds_wasm_bound(values.len(), MAX_OPTIONS) {
            return None;
        }
        let mut labels = BTreeSet::new();
        let mut criteria = Vec::new();
        for value in &values {
            let label = label_of(value);
            // Two values with one label could not be told apart in the
            // answer: the member, and so the event, is not offered.
            if !valid_label(&label) || !labels.insert(label.clone()) {
                return None;
            }
            criteria.push(Criterion {
                label,
                description: Json::Null,
            });
        }
        members.push((member.to_owned(), criteria, values));
    }
    Some(Grounded { candidate, members })
}

const NO_ANSWER: &str = "the decision response answers no question";

/// The questions of one `<decision:choose>` batch: the selection (every
/// offered event, then [`ABSTAIN_LABEL`]), then each offered event's member
/// questions (`fill.<event>.<member>`), in menu and `required` order.
fn choose_questions(offered: &[Grounded<'_>], instructions: String) -> Vec<Question> {
    let mut selection: Vec<Criterion> = offered
        .iter()
        .map(|grounded| Criterion {
            label: grounded.candidate.event.clone(),
            description: grounded
                .candidate
                .description
                .clone()
                .map_or(Json::Null, Json::String),
        })
        .collect();
    selection.push(Criterion {
        label: ABSTAIN_LABEL.into(),
        description: ABSTAIN_DESCRIPTION.into(),
    });
    let mut questions = vec![Question {
        name: SELECTION_QUESTION.into(),
        instructions,
        criteria: selection,
    }];
    for (event_index, grounded) in offered.iter().enumerate() {
        for (member_index, (member, criteria, _)) in grounded.members.iter().enumerate() {
            questions.push(Question {
                name: format!("fill.{event_index}.{member_index}"),
                instructions: format!(
                    "Which value fills {} for the event {}? {FILL_INSTRUCTIONS}",
                    Json::String(member.clone()),
                    grounded.candidate.event
                ),
                criteria: criteria.clone(),
            });
        }
    }
    questions
}

/// The data of the offered event at `chosen`: each required member's most
/// probable grounded option ([`choose_questions`] order).
fn filled_data(
    offered: &[Grounded<'_>],
    chosen: usize,
    distributions: &[Distribution],
) -> Result<Json, &'static str> {
    let grounded = offered.get(chosen).ok_or(NO_ANSWER)?;
    // The member questions of earlier events precede this event's.
    let first = 1 + offered
        .iter()
        .take(chosen)
        .map(|earlier| earlier.members.len())
        .sum::<usize>();
    let mut data = Map::new();
    for (offset, (member, criteria, values)) in grounded.members.iter().enumerate() {
        let distribution = distributions.get(first + offset).ok_or(NO_ANSWER)?;
        let index = backend::argmax(criteria, distribution).ok_or(NO_ANSWER)?;
        let value = values.get(index).ok_or(NO_ANSWER)?;
        data.insert(member.clone(), value.clone());
    }
    Ok(Json::Object(data))
}

struct Context<'a, B, H> {
    backend: &'a B,
    host: &'a mut H,
    parameters: &'a Parameters,
}

impl<B: Backend, H: DecisionHost> Context<'_, B, H> {
    fn evaluate(
        &mut self,
        expression: &str,
        failed: &'static str,
    ) -> Result<Value, Failure<H::Error>> {
        self.host
            .evaluate(expression)
            .map_err(Failure::Host)?
            .map_err(|()| Failure::Execution(failed))
    }

    /// An evaluated attribute's JSON value.
    fn evaluate_json(
        &mut self,
        expression: &str,
        failed: &'static str,
    ) -> Result<Json, Failure<H::Error>> {
        json_of(self.evaluate(expression, failed)?).ok_or(Failure::Execution(failed))
    }

    /// A string attribute or its `expr` form.
    fn text(
        &mut self,
        name: &str,
        expr: &str,
        failed: &'static str,
    ) -> Result<Option<String>, Failure<H::Error>> {
        let text = match (self.parameters.get(name), self.parameters.get(expr)) {
            (Some(text), _) => text.trim().to_owned(),
            (None, Some(expression)) => match self.evaluate(expression, failed)? {
                Value::Text(text) => text.trim().to_owned(),
                _ => return Err(Failure::Execution(failed)),
            },
            (None, None) => return Ok(None),
        };
        if text.is_empty() {
            return Err(Failure::Execution(failed));
        }
        Ok(Some(text))
    }

    fn model(&mut self) -> Result<Option<String>, Failure<H::Error>> {
        const INVALID: &str =
            "decision model or modelexpr must be a non-empty string within the model bound";
        let model = self.text("model", "modelexpr", INVALID)?;
        if model
            .as_ref()
            .is_some_and(|model| exceeds_wasm_bound(model.len(), MAX_MODEL_BYTES))
        {
            return Err(Failure::Execution(INVALID));
        }
        Ok(model)
    }

    fn instructions(&mut self, default: &str) -> Result<String, Failure<H::Error>> {
        const INVALID: &str = "decision instructions or instructionsexpr must be a non-empty string within the instructions bound";
        let instructions = self
            .text("instructions", "instructionsexpr", INVALID)?
            .unwrap_or_else(|| default.to_owned());
        if exceeds_wasm_bound(instructions.len(), MAX_INSTRUCTIONS_BYTES) {
            return Err(Failure::Execution(INVALID));
        }
        Ok(instructions)
    }

    fn snapshot(&mut self, exclude: &[Section]) -> Result<String, Failure<H::Error>> {
        self.host
            .snapshot(exclude)
            .map_err(Failure::Host)?
            .map_err(|()| Failure::Execution("the host offers no session snapshot"))
    }

    /// The state document, and whether `statexpr` gave it.
    fn state(&mut self) -> Result<(Json, bool), Failure<H::Error>> {
        const INVALID: &str =
            "decision statexpr must evaluate to a string, a JSON object, or a JSON array";
        let (state, explicit) = if let Some(expression) = self.parameters.get("statexpr") {
            (
                self.evaluate_json(expression, "decision statexpr could not be evaluated")?,
                true,
            )
        } else {
            let document = self.snapshot(&STATE_SNAPSHOT_EXCLUDE)?;
            let document: Json = serde_json::from_str(&document)
                .map_err(|_| Failure::Execution("the session snapshot is malformed"))?;
            let root = document
                .pointer("/snapshot/root")
                .filter(|root| root.is_object())
                .cloned()
                .ok_or(Failure::Execution("the session snapshot is malformed"))?;
            (root, false)
        };
        if !matches!(state, Json::String(_) | Json::Object(_) | Json::Array(_)) {
            return Err(Failure::Execution(INVALID));
        }
        if exceeds_wasm_bound(canonical_json(&state).len(), MAX_STATE_BYTES) {
            return Err(Failure::Execution("decision state exceeds its bound"));
        }
        Ok((state, explicit))
    }

    fn location(&self) -> Result<&str, Failure<H::Error>> {
        self.parameters
            .get("location")
            .map(str::trim)
            .filter(|location| !location.is_empty())
            .ok_or(Failure::Execution(
                "decision:rank and decision:classify require a non-empty location",
            ))
    }

    fn assign(&mut self, location: &str, value: &Json) -> Result<(), Failure<H::Error>> {
        let data = match value {
            Json::Array(_) => Data::JsonArray(canonical_json(value)),
            _ => Data::JsonObject(canonical_json(value)),
        };
        self.host
            .assign(location, &data)
            .map_err(Failure::Host)?
            .map_err(|()| Failure::Execution("decision could not assign the result to location"))
    }

    fn raise(&mut self, name: &str, data: &Json) -> Result<(), Failure<H::Error>> {
        self.host
            .raise(name, &Data::JsonObject(canonical_json(data)))
            .map_err(Failure::Host)?
            .map_err(|()| Failure::Execution("decision:choose could not raise its event"))
    }

    /// Sends one batch and returns its distributions.
    fn ask(&mut self, batch: &Batch) -> Result<Vec<Distribution>, Failure<H::Error>> {
        if exceeds_wasm_bound(batch.questions.len(), MAX_QUESTIONS) {
            return Err(Failure::Execution(
                "decision batch exceeds the question bound",
            ));
        }
        let api_key = self.host.slot(API_KEY_SLOT).ok_or(Failure::Execution(
            "decision has no API key: env:apikey names no variable the run holds",
        ))?;
        let base_url = self.host.slot(BASE_URL_SLOT).ok_or(Failure::Execution(
            "decision has no base URL: env:baseurl names no variable the run holds",
        ))?;
        let request = self.backend.request(batch, api_key, base_url);
        let response = self
            .host
            .post(&request)
            .map_err(Failure::Host)?
            .map_err(|()| Failure::Execution("the decision request was refused or failed"))?;
        if !(200..300).contains(&response.status) {
            return Err(Failure::Execution(
                "the decision service answered with a non-success status",
            ));
        }
        self.backend
            .answers(batch, &response.body)
            .map_err(Failure::Execution)
    }

    /// One question's single distribution.
    fn ask_one(
        &mut self,
        name: &str,
        instructions: String,
        criteria: Vec<Criterion>,
    ) -> Result<(Vec<Criterion>, Distribution), Failure<H::Error>> {
        let model = self.model()?;
        let (state, _) = self.state()?;
        let batch = Batch {
            model,
            state,
            questions: vec![Question {
                name: name.to_owned(),
                instructions,
                criteria,
            }],
        };
        let mut distributions = self.ask(&batch)?;
        let distribution = distributions.pop().ok_or(Failure::Execution(
            "the decision response answers no question",
        ))?;
        let criteria = batch
            .questions
            .into_iter()
            .next()
            .map(|question| question.criteria)
            .unwrap_or_default();
        Ok((criteria, distribution))
    }

    fn rank(&mut self) -> Result<(), Failure<H::Error>> {
        const INVALID: &str = "decision:rank candidatesexpr must evaluate to a non-empty array of distinct candidates";
        let location = self.location()?.to_owned();
        let expression = self
            .parameters
            .get("candidatesexpr")
            .ok_or(Failure::Execution("decision:rank requires candidatesexpr"))?;
        let Json::Array(candidates) = self.evaluate_json(expression, INVALID)? else {
            return Err(Failure::Execution(INVALID));
        };
        if candidates.is_empty() || exceeds_wasm_bound(candidates.len(), MAX_CRITERIA) {
            return Err(Failure::Execution(INVALID));
        }
        let mut labels = BTreeSet::new();
        let mut criteria = Vec::new();
        for candidate in &candidates {
            let label = label_of(candidate);
            if !valid_label(&label) || !labels.insert(label.clone()) {
                return Err(Failure::Execution(INVALID));
            }
            criteria.push(Criterion {
                label,
                description: match candidate {
                    Json::Object(_) | Json::Array(_) => candidate.clone(),
                    _ => Json::Null,
                },
            });
        }
        let instructions = self.instructions(RANK_INSTRUCTIONS)?;
        let (criteria, distribution) = self.ask_one("rank", instructions, criteria)?;
        let mut ranked: Vec<(f64, &str, &Json)> = criteria
            .iter()
            .zip(&candidates)
            .enumerate()
            .map(|(index, (criterion, candidate))| {
                (
                    distribution
                        .probabilities
                        .get(index)
                        .copied()
                        .unwrap_or(0.0),
                    criterion.label.as_str(),
                    candidate,
                )
            })
            .collect();
        ranked.sort_by(|left, right| right.0.total_cmp(&left.0).then_with(|| left.1.cmp(right.1)));
        let result: Vec<Json> = ranked
            .into_iter()
            .map(|(score, _, candidate)| serde_json::json!({ "candidate": candidate, "score": score }))
            .collect();
        self.assign(&location, &Json::Array(result))
    }

    fn classify(&mut self) -> Result<(), Failure<H::Error>> {
        const INVALID: &str =
            "decision:classify labels must be non-empty, distinct, and within the label bounds";
        let location = self.location()?.to_owned();
        let criteria: Vec<Criterion> = match (
            self.parameters.get("labels"),
            self.parameters.get("labelsexpr"),
        ) {
            (Some(labels), None) => labels
                .split(scxml_types::is_xml_space)
                .filter(|label| !label.is_empty())
                .map(|label| Criterion {
                    label: label.to_owned(),
                    description: Json::Null,
                })
                .collect(),
            (None, Some(expression)) => {
                let failed = "decision:classify labelsexpr must evaluate to an array of strings or an object of descriptions";
                match self.evaluate_json(expression, failed)? {
                    Json::Array(items) => items
                        .into_iter()
                        .map(|item| match item {
                            Json::String(label) => Ok(Criterion {
                                label,
                                description: Json::Null,
                            }),
                            _ => Err(Failure::Execution(failed)),
                        })
                        .collect::<Result<_, _>>()?,
                    Json::Object(members) => members
                        .into_iter()
                        .map(|(label, description)| match description {
                            Json::Bool(_) | Json::Number(_) => Err(Failure::Execution(failed)),
                            description => Ok(Criterion { label, description }),
                        })
                        .collect::<Result<_, _>>()?,
                    _ => return Err(Failure::Execution(failed)),
                }
            }
            _ => {
                return Err(Failure::Execution(
                    "decision:classify takes exactly one of labels or labelsexpr",
                ));
            }
        };
        let mut labels = BTreeSet::new();
        if criteria.is_empty()
            || exceeds_wasm_bound(criteria.len(), MAX_CRITERIA)
            || !criteria.iter().all(|criterion| {
                valid_label(&criterion.label) && labels.insert(criterion.label.as_str())
            })
        {
            return Err(Failure::Execution(INVALID));
        }
        let instructions = self.instructions(CLASSIFY_INSTRUCTIONS)?;
        let (criteria, distribution) = self.ask_one("classify", instructions, criteria)?;
        let result: Map<String, Json> = criteria
            .into_iter()
            .enumerate()
            .map(|(index, criterion)| {
                (
                    criterion.label,
                    distribution
                        .probabilities
                        .get(index)
                        .copied()
                        .unwrap_or(0.0)
                        .into(),
                )
            })
            .collect();
        self.assign(&location, &Json::Object(result))
    }

    fn threshold(&self) -> Result<f64, Failure<H::Error>> {
        match self.parameters.get("threshold") {
            None => Ok(0.0),
            Some(text) => text
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|threshold| (0.0..=1.0).contains(threshold))
                .ok_or(Failure::Execution(
                    "decision:choose threshold must be a number in [0, 1]",
                )),
        }
    }

    fn abstain(&mut self, reason: &str, confidence: Option<f64>) -> Result<(), Failure<H::Error>> {
        let mut data = Map::new();
        data.insert("reason".into(), reason.into());
        if let Some(confidence) = confidence {
            data.insert("confidence".into(), confidence.into());
        }
        self.raise(ABSTAIN_EVENT, &Json::Object(data))
    }

    /// The evidence grounded member options come from: `dataexpr`, else an
    /// object state `statexpr` gave, else none.
    fn evidence(
        &mut self,
        state: &Json,
        explicit_state: bool,
    ) -> Result<Option<Map<String, Json>>, Failure<H::Error>> {
        const INVALID: &str = "decision:choose dataexpr must evaluate to a JSON object";
        if let Some(expression) = self.parameters.get("dataexpr") {
            return match self.evaluate_json(expression, INVALID)? {
                Json::Object(members) => Ok(Some(members)),
                _ => Err(Failure::Execution(INVALID)),
            };
        }
        Ok(match state {
            Json::Object(members) if explicit_state => Some(members.clone()),
            _ => None,
        })
    }

    /// The menu: `candidatesexpr`, else the snapshot's schema transitions.
    fn menu(&mut self) -> Result<Vec<Candidate>, Failure<H::Error>> {
        if let Some(expression) = self.parameters.get("candidatesexpr") {
            let value = self.evaluate_json(
                expression,
                "decision:choose candidatesexpr could not be evaluated",
            )?;
            return explicit_menu(&value).map_err(Failure::Execution);
        }
        let document = self.snapshot(&MENU_SNAPSHOT_EXCLUDE)?;
        snapshot_menu(&document).map_err(Failure::Execution)
    }

    fn choose(&mut self) -> Result<(), Failure<H::Error>> {
        let threshold = self.threshold()?;
        let model = self.model()?;
        let instructions = self.instructions(CHOOSE_INSTRUCTIONS)?;
        let (state, explicit_state) = self.state()?;
        let evidence = self.evidence(&state, explicit_state)?;
        let menu = self.menu()?;
        let offered: Vec<Grounded<'_>> = menu
            .iter()
            .filter_map(|candidate| ground(candidate, evidence.as_ref()))
            .collect();
        if offered.is_empty() {
            return self.abstain("no-candidates", None);
        }
        if exceeds_wasm_bound(offered.len(), MAX_CRITERIA) {
            return Err(Failure::Execution(
                "decision:choose exceeds the candidate bound",
            ));
        }
        let batch = Batch {
            model,
            state,
            questions: choose_questions(&offered, instructions),
        };
        let distributions = self.ask(&batch)?;
        let (Some(selection_question), Some(selection_distribution)) =
            (batch.questions.first(), distributions.first())
        else {
            return Err(Failure::Execution(NO_ANSWER));
        };
        let chosen = backend::argmax(&selection_question.criteria, selection_distribution)
            .ok_or(Failure::Execution(NO_ANSWER))?;
        let confidence = selection_distribution
            .probabilities
            .get(chosen)
            .copied()
            .unwrap_or(0.0);
        let Some(grounded) = offered.get(chosen) else {
            // The abstain label is the selection's last criterion.
            return self.abstain("declined", Some(confidence));
        };
        if confidence < threshold {
            return self.abstain("below-threshold", Some(confidence));
        }
        let data = filled_data(&offered, chosen, &distributions).map_err(Failure::Execution)?;
        if !grounded
            .candidate
            .compiled
            .admits(&Data::JsonObject(canonical_json(&data)))
        {
            return Err(Failure::Execution(
                "the filled data does not satisfy the chosen event's schema; nothing was raised",
            ));
        }
        let event = grounded.candidate.event.clone();
        self.raise(&event, &data)
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
    use stateforward::scxml::telemetry::{self, Operation, Outcome, SignalEvent};
    use stateforward::scxml::types::{
        ComponentError, DataValue, ErrorCode, ErrorVersion, Event, EventType,
        ExecutableContentExtension, ExtensionContent, ExtensionOutcome, SlotReference,
        TraceContext,
    };
    use stateforward::scxml::{
        data_model_host, execution_host, outbound_http, session_snapshot_host,
    };

    use super::{
        Attribute, Content, Data, DecisionHost, Executed, Failure, Part, Request, Response,
        Section, Value,
    };

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

    /// The host capabilities of one `execute` call, with its slot references.
    struct Host {
        references: Vec<SlotReference>,
    }

    /// Evaluation, assignment, and raise failures (and a session with no
    /// data model) are document errors.
    fn recoverable(code: ErrorCode) -> bool {
        matches!(
            code,
            ErrorCode::InvalidInput
                | ErrorCode::Execution
                | ErrorCode::Unavailable
                | ErrorCode::PayloadTooLarge
        )
    }

    fn text_parts(parts: &[Part]) -> Vec<outbound_http::TextPart> {
        parts
            .iter()
            .map(|part| match part {
                Part::Literal(text) => outbound_http::TextPart::Literal(text.clone()),
                Part::Reference(reference) => outbound_http::TextPart::Reference(*reference),
            })
            .collect()
    }

    /// One value as the WIT data value it is assigned or raised as; the
    /// core never produces absent, undefined, or XML data.
    fn data_value(value: &Data) -> Option<DataValue> {
        Some(match value {
            Data::Null => DataValue::Null,
            Data::Bool(value) => DataValue::Boolean(*value),
            Data::Number(number) => DataValue::Number(number.get()),
            Data::String(text) => DataValue::Text(text.clone()),
            Data::JsonArray(text) => DataValue::JsonArray(text.clone()),
            Data::JsonObject(text) => DataValue::JsonObject(text.clone()),
            Data::Absent | Data::Undefined | Data::Xml(_) => return None,
        })
    }

    impl DecisionHost for Host {
        type Error = ComponentError;

        fn evaluate(&mut self, expression: &str) -> Result<Result<Value, ()>, ComponentError> {
            match data_model_host::evaluate(expression) {
                Ok(DataValue::Null) => Ok(Ok(Value::Null)),
                Ok(DataValue::Boolean(value)) => Ok(Ok(Value::Bool(value))),
                Ok(DataValue::Number(number)) if number.is_finite() => {
                    Ok(Ok(Value::Number(number)))
                }
                Ok(DataValue::Text(text)) => Ok(Ok(Value::Text(text))),
                Ok(DataValue::JsonObject(text) | DataValue::JsonArray(text)) => {
                    Ok(Ok(Value::Json(text)))
                }
                Ok(_) => Ok(Ok(Value::Other)),
                Err(error) if recoverable(error.code) => Ok(Err(())),
                Err(error) => Err(error),
            }
        }

        fn assign(
            &mut self,
            location: &str,
            value: &Data,
        ) -> Result<Result<(), ()>, ComponentError> {
            let Some(value) = data_value(value) else {
                return Ok(Err(()));
            };
            match data_model_host::assign(location, &value) {
                Ok(()) => Ok(Ok(())),
                Err(error) if recoverable(error.code) => Ok(Err(())),
                Err(error) => Err(error),
            }
        }

        fn slot(&mut self, slot: &str) -> Option<u32> {
            self.references
                .iter()
                .find(|bound| bound.slot == slot)
                .map(|bound| bound.reference)
        }

        fn post(&mut self, request: &Request) -> Result<Result<Response, ()>, ComponentError> {
            let request = outbound_http::HttpRequest {
                method: "POST".into(),
                url: text_parts(&request.url),
                headers: request
                    .headers
                    .iter()
                    .map(|(name, value)| outbound_http::HttpHeader {
                        name: name.clone(),
                        value: text_parts(value),
                    })
                    .collect(),
                body: vec![outbound_http::BodyPart::Bytes(request.body.clone())],
            };
            match outbound_http::fetch(&request) {
                Ok(response) => Ok(Ok(Response {
                    status: response.status,
                    body: response.body,
                })),
                // A refusal, a transport failure, or a response over the
                // host's bound: SCXML `error.execution`.
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
                // bound: the element cannot decide (`error.execution`).
                Err(error)
                    if recoverable(error.code) || error.code == ErrorCode::CapacityExhausted =>
                {
                    Ok(Err(()))
                }
                Err(error) => Err(error),
            }
        }

        fn raise(&mut self, name: &str, data: &Data) -> Result<Result<(), ()>, ComponentError> {
            let Some(data) = data_value(data) else {
                return Ok(Err(()));
            };
            let event = Event {
                name: name.to_owned(),
                event_type: EventType::Internal,
                send_id: None,
                origin: None,
                origin_type: None,
                invoke_id: None,
                data,
                raw: None,
                trace_context: None,
            };
            match execution_host::raise(&event) {
                Ok(()) => Ok(Ok(())),
                Err(error) if error.code == ErrorCode::InvalidInput => Ok(Err(())),
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
            references: Vec<SlotReference>,
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
                let mut host = Host { references };
                match super::execute(&element.name.local_name, &attributes, &flattened, &mut host) {
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
