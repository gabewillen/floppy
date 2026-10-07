//! The `decision` invoked service: one decision per `<invoke>` (choose an
//! event from a menu, rank candidates, or classify a state), as a
//! `stateforward:scxml/invoked-service` Wasm component.
//!
//! Asking a backend is slow I/O, so a decision is a service, never
//! executable content: the invoking session keeps processing events and
//! timers while the question is in flight, a `<send delay>` timeout in the
//! invoking state preempts it, and leaving that state cancels it. The
//! synchronous half stays executable content: `<decision:menu>`
//! (`namespaces/decision`) reads the enabled `event:schema` transitions from
//! the session snapshot and writes the canonical menu this service takes as
//! its `menu` parameter.
//!
//! Each invocation asks a [`Backend`] ([`TypeSafe`] `system_one` today; a CLM
//! backend implements the same trait) one batch of labelled choice
//! questions about one state document and acts on the probability
//! distributions it answers. Every decision is computed from the
//! distributions alone ([`backend::argmax`]: highest probability, ties broken
//! by the label's UTF-8 bytes), and every request body is canonical JSON, so
//! one request always sends the same bytes and takes the same decision from
//! the same answer. Documents name [`SERVICE_URI`]:
//!
//! ```xml
//! <onentry><decision:menu location="menu"/></onentry>
//! <invoke type="https://github.com/stateforward/floppy/services/decision"
//!         env:apikey="TYPESAFE_API_KEY" env:baseurl="TYPESAFE_BASE_URL">
//!   <param name="menu" expr="menu"/>
//!   <param name="state" expr="({failures: failures})"/>
//! </invoke>
//! ```
//!
//! # Parameters (`<param>`)
//!
//! * `kind` (optional): `choose` (the default), `rank`, or `classify`.
//! * `state` (required): the state document the backend decides about, a
//!   string, a JSON object, or a JSON array.
//! * `instructions` (optional, a non-empty string): what to decide; each
//!   kind has a default.
//! * `model` (optional, a non-empty string): the backend's model; the
//!   backend's default without one.
//! * `choose`: `menu` (required: a JSON array of objects with `event`, an
//!   event name, `schema`, a self-contained JSON Schema, and an optional
//!   `description` string, as `<decision:menu>` writes it; events distinct),
//!   `data` (optional: a JSON object of evidence), and `threshold`
//!   (optional, a number in `[0, 1]`, `0` without it).
//! * `rank`: `candidates` (required: a non-empty JSON array of distinct
//!   candidates; a string is its own label, any other value is labelled by
//!   its canonical JSON, and an object or array is also its description).
//! * `classify`: `labels` (required: a JSON array of strings, a JSON object
//!   mapping each label to its description, a string, object, array, or
//!   `null`, or one string of labels separated by XML white space; labels
//!   non-empty and distinct).
//!
//! Any other parameter, a parameter given twice, or a malformed value
//! rejects the start: the `<invoke>` raises `error.execution` and nothing is
//! sent. The `env` slots: `env:apikey="NAME"` (a bearer token, required at
//! run time) and `env:baseurl="NAME"` (the API base URL, for example
//! `https://api.typesafe.ai`; it must not end with `/`). When no `baseurl`
//! value is held, the base URL is the package's first declared
//! `http-origins` entry (`https://api.typesafe.ai`, read from
//! `package/decision.service.json`), as the deleted scxml.rs `openai`
//! namespace defaulted to its provider's URL; the manifest stays the one
//! place the origin is written.
//!
//! # `choose`
//!
//! * **Grounded data.** The data of each offered event is a JSON object
//!   holding its schema's top-level `required` members, each chosen by its
//!   own question among grounded options, never authored: the evidence's
//!   member of that name (the evidence is `data`, else the state when it is
//!   an object), then the property's `enum` values, then its `const`. An
//!   event with a required member that has no option is not offered.
//! * **The decision.** One batch asks the selection (every offered event,
//!   plus [`ABSTAIN_LABEL`]) and every offered event's member questions.
//!   When nothing is offered, the abstain label wins, or the chosen event's
//!   probability is below `threshold`, the service returns [`ABSTAIN_EVENT`]
//!   `{"reason": "no-candidates" | "declined" | "below-threshold", "confidence": P}`
//!   (`confidence` absent for `no-candidates`, which sends nothing).
//! * **Validation before returning.** The filled data must satisfy the chosen
//!   event's schema from the menu; otherwise the service returns
//!   [`FAILED_EVENT`] `{"code": "schema-mismatch"}` and never the event.
//!
//! # `rank` and `classify`
//!
//! `rank` returns [`RANKED_EVENT`] `{"ranking": [{"candidate": C, "score": P}]}`,
//! every candidate once, by score descending, ties by label. `classify`
//! returns [`CLASSIFIED_EVENT`] `{"labels": {L: P}}` with every label's
//! probability.
//!
//! # Events
//!
//! `invoke` (the component's one export, an `async func`) writes exactly
//! one event to the stream it returns, then resolves its result future with
//! no data. A failure returns [`FAILED_EVENT`] `{"code": C}` with one closed
//! [`FailureCode`]. The chosen event of `choose` is defined by the invoking
//! document (its menu's `event:schema` transitions), and the document
//! describes this service's own events with `event:schema` too
//! (`bots/builder/builder.scxml`); the service validates what it returns
//! itself: the chosen event's data against the menu's schema for it, and
//! its own events by construction (their shapes are documented here). An
//! event name the menu offers is a valid SCXML event name with no `*` and no
//! empty token. The service reads no event: it drops the event stream
//! `invoke` receives. `outbound-http.fetch` is an `async func`: awaiting it
//! suspends only the invocation's own task, and leaving the invoking state
//! cancels the invocation (SCXML 6.4), which closes the request's
//! connection.
//!
//! # Bounds
//!
//! Candidates and labels ([`MAX_CRITERIA`], [`MAX_LABEL_BYTES`]), grounded
//! options ([`MAX_OPTIONS`]), required members ([`MAX_REQUIRED`]), questions
//! ([`MAX_QUESTIONS`]), the state ([`MAX_STATE_BYTES`]), the instructions
//! ([`MAX_INSTRUCTIONS_BYTES`]), the model ([`MAX_MODEL_BYTES`]), and event
//! names ([`MAX_EVENT_NAME_BYTES`]) are bounded on wasm builds only
//! ([`scxml_types::wasm_bound`]); schemas are bounded by
//! `scxml-event-schema`, and the answer by the host's response-body bound.
//!
//! # Telemetry
//!
//! `invoke` records one observation through scxml.rs's `telemetry` WIT
//! facade (operation `start`: its closed outcome, `ok`, `rejected`, or
//! `failed`, and one closed signal event), and one closed signal event
//! (operation `send`) when it returns its event. No attribute carries the
//! state, a label, an event name, or a URL, so the cardinality is the
//! product of the closed enums.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

pub mod backend;

use std::collections::BTreeSet;

use scxml_event_schema::{CompiledSchema, canonical_json};
use scxml_types::{DataValue as Data, exceeds_wasm_bound, wasm_bound};
use serde_json::{Map, Value as Json};

pub use backend::{Backend, Batch, Criterion, Distribution, Question, TypeSafe};

/// The service's name: the last path segment of its repository service URI
/// before the tag.
pub const SERVICE_NAME: &str = "decision";

/// The exact service type URI documents name: tagless, so it names the
/// repository's latest release (decision D19); the manifest names it too.
pub const SERVICE_URI: &str = "https://github.com/stateforward/floppy/services/decision";

/// The `env` slot of the API key (`env:apikey`).
pub const API_KEY_SLOT: &str = "apikey";
/// The `env` slot of the API base URL (`env:baseurl`).
pub const BASE_URL_SLOT: &str = "baseurl";

/// The event `choose` returns when it takes no event.
pub const ABSTAIN_EVENT: &str = "decision.abstain";
/// The event `rank` returns.
pub const RANKED_EVENT: &str = "decision.ranked";
/// The event `classify` returns.
pub const CLASSIFIED_EVENT: &str = "decision.classified";
/// The event a failed decision returns.
pub const FAILED_EVENT: &str = "decision.failed";
/// The selection's reserved criterion: no event fits. It holds spaces, so
/// it is never an event name.
pub const ABSTAIN_LABEL: &str = "none of these";

/// The selection question's name in a `choose` batch.
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
/// Upper bound on a menu event name's bytes (wasm builds only).
pub const MAX_EVENT_NAME_BYTES: usize = wasm_bound(256);

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

/// Why the transport answered no response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportFailure {
    /// No response: refused by the host before sending (an origin the
    /// package does not declare, or no transport), unreachable, timed out,
    /// or failed.
    Unavailable,
    /// The response body exceeds the host's bound.
    TooLarge,
}

/// The closed failure codes of [`FAILED_EVENT`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureCode {
    /// The `apikey` slot names no variable the run holds.
    Unconfigured,
    /// The transport failed: no response.
    Transport,
    /// The backend answered with a non-2xx status.
    Status,
    /// The answer does not answer exactly the questions asked with valid
    /// distributions, or exceeds the host's response bound.
    Malformed,
    /// The filled data does not satisfy the chosen event's schema.
    SchemaMismatch,
}

impl FailureCode {
    /// The wire name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unconfigured => "unconfigured",
            Self::Transport => "transport",
            Self::Status => "status",
            Self::Malformed => "malformed",
            Self::SchemaMismatch => "schema-mismatch",
        }
    }
}

/// The one event an invocation returns before it completes: its name and its
/// data as a canonical JSON object.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Returned {
    /// The event name: a menu event, or one of this service's events.
    pub name: String,
    /// Canonical JSON object text.
    pub data: String,
}

impl Returned {
    fn new(name: &str, data: &Json) -> Self {
        Self {
            name: name.to_owned(),
            data: canonical_json(data),
        }
    }

    /// [`FAILED_EVENT`] with `code`.
    #[must_use]
    pub fn failed(code: FailureCode) -> Self {
        Self::new(FAILED_EVENT, &serde_json::json!({ "code": code.name() }))
    }

    fn abstain(reason: &str, confidence: Option<f64>) -> Self {
        let mut data = Map::new();
        data.insert("reason".into(), reason.into());
        if let Some(confidence) = confidence {
            data.insert("confidence".into(), confidence.into());
        }
        Self::new(ABSTAIN_EVENT, &Json::Object(data))
    }
}

/// One event `choose` may return.
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

/// Whether `name` is an event name a candidate may carry: non-empty, no
/// white space, no `*`, no empty `.`-separated token, bounded.
fn valid_event_name(name: &str) -> bool {
    !name.is_empty()
        && !exceeds_wasm_bound(name.len(), MAX_EVENT_NAME_BYTES)
        && !name.contains('*')
        && !name.chars().any(scxml_types::is_xml_space)
        && name.split('.').all(|token| !token.is_empty())
}

/// The candidates of a `menu` value: an array of objects with `event`,
/// `schema`, and an optional `description`.
///
/// # Errors
///
/// A static detail for a malformed candidate, a duplicate event, or a schema
/// that is not a self-contained JSON Schema.
pub fn menu_from_json(value: &Json) -> Result<Vec<Candidate>, &'static str> {
    const INVALID: &str = "decision menu must be an array of objects with an event name, a schema, and an optional description";
    let items = value.as_array().ok_or(INVALID)?;
    if exceeds_wasm_bound(items.len(), MAX_CRITERIA) {
        return Err("decision menu exceeds the candidate bound");
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
            return Err("decision menu names an event twice");
        }
        let source = members.get("schema").ok_or(INVALID)?;
        let resolved = scxml_event_schema::resolve(canonical_json(source).as_bytes(), None)
            .map_err(|_| "decision menu schema is not a self-contained JSON Schema")?;
        let schema: Json = serde_json::from_str(&resolved.schema).map_err(|_| INVALID)?;
        let compiled = CompiledSchema::new(&resolved.schema)
            .map_err(|_| "decision menu schema is not a self-contained JSON Schema")?;
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

/// A candidate (by menu index) with grounded options for every required
/// member.
#[derive(Debug)]
struct Grounded {
    candidate: usize,
    /// `(member, criteria, values)` in the schema's `required` order.
    members: Vec<(String, Vec<Criterion>, Vec<Json>)>,
}

/// The grounded options of every required member of `menu[index]`, or
/// `None` when one has no option (the event is then not offered).
fn ground(
    menu: &[Candidate],
    index: usize,
    evidence: Option<&Map<String, Json>>,
) -> Option<Grounded> {
    let candidate = menu.get(index)?;
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
    Some(Grounded {
        candidate: index,
        members,
    })
}

/// One validated decision request: what an invocation's parameters ask.
#[derive(Debug)]
pub struct DecisionRequest {
    model: Option<String>,
    instructions: String,
    state: Json,
    kind: Kind,
}

#[derive(Debug)]
enum Kind {
    Choose {
        menu: Vec<Candidate>,
        evidence: Option<Map<String, Json>>,
        threshold: f64,
    },
    Rank {
        candidates: Vec<Json>,
        criteria: Vec<Criterion>,
    },
    Classify {
        criteria: Vec<Criterion>,
    },
}

/// The JSON value of one parameter value; `None` for a value with no JSON
/// form (absent, undefined, XML).
fn json_of(value: &Data) -> Option<Json> {
    match value {
        Data::Null => Some(Json::Null),
        Data::Bool(value) => Some(Json::Bool(*value)),
        Data::Number(number) => serde_json::Number::from_f64(number.get()).map(Json::Number),
        Data::String(text) => Some(Json::String(text.clone())),
        Data::JsonArray(text) | Data::JsonObject(text) => serde_json::from_str(text).ok(),
        Data::Absent | Data::Undefined | Data::Xml(_) => None,
    }
}

fn text_of(value: &Data, bound: usize, invalid: &'static str) -> Result<String, &'static str> {
    match value {
        Data::String(text) if !text.trim().is_empty() && !exceeds_wasm_bound(text.len(), bound) => {
            Ok(text.trim().to_owned())
        }
        _ => Err(invalid),
    }
}

/// The criteria of a `labels` value.
fn label_criteria(value: &Data) -> Result<Vec<Criterion>, &'static str> {
    const INVALID: &str = "decision labels must be an array of strings, an object of descriptions, or a string of labels";
    let criteria: Vec<Criterion> = match value {
        Data::String(text) => text
            .split(scxml_types::is_xml_space)
            .filter(|label| !label.is_empty())
            .map(|label| Criterion {
                label: label.to_owned(),
                description: Json::Null,
            })
            .collect(),
        _ => match json_of(value).ok_or(INVALID)? {
            Json::Array(items) => items
                .into_iter()
                .map(|item| match item {
                    Json::String(label) => Ok(Criterion {
                        label,
                        description: Json::Null,
                    }),
                    _ => Err(INVALID),
                })
                .collect::<Result<_, _>>()?,
            Json::Object(members) => members
                .into_iter()
                .map(|(label, description)| match description {
                    Json::Bool(_) | Json::Number(_) => Err(INVALID),
                    description => Ok(Criterion { label, description }),
                })
                .collect::<Result<_, _>>()?,
            _ => return Err(INVALID),
        },
    };
    let mut labels = BTreeSet::new();
    if criteria.is_empty()
        || exceeds_wasm_bound(criteria.len(), MAX_CRITERIA)
        || !criteria.iter().all(|criterion| {
            valid_label(&criterion.label) && labels.insert(criterion.label.as_str())
        })
    {
        return Err("decision labels must be non-empty, distinct, and within the label bounds");
    }
    Ok(criteria)
}

/// The candidates of a `rank` request and their criteria.
fn rank_criteria(value: &Data) -> Result<(Vec<Json>, Vec<Criterion>), &'static str> {
    const INVALID: &str =
        "decision candidates must be a non-empty JSON array of distinct candidates";
    let Some(Json::Array(candidates)) = json_of(value) else {
        return Err(INVALID);
    };
    if candidates.is_empty() || exceeds_wasm_bound(candidates.len(), MAX_CRITERIA) {
        return Err(INVALID);
    }
    let mut labels = BTreeSet::new();
    let mut criteria = Vec::new();
    for candidate in &candidates {
        let label = label_of(candidate);
        if !valid_label(&label) || !labels.insert(label.clone()) {
            return Err(INVALID);
        }
        criteria.push(Criterion {
            label,
            description: match candidate {
                Json::Object(_) | Json::Array(_) => candidate.clone(),
                _ => Json::Null,
            },
        });
    }
    Ok((candidates, criteria))
}

/// The value of the parameter `name`, when given.
fn find<'a>(given: &[(&str, &'a Data)], name: &str) -> Option<&'a Data> {
    given
        .iter()
        .find(|(seen, _)| *seen == name)
        .map(|(_, value)| *value)
}

/// The `choose` part of a request: its menu, its evidence (`data`, else an
/// object `state`), and its threshold.
fn choose_kind(given: &[(&str, &Data)], state: &Json) -> Result<Kind, &'static str> {
    let menu = menu_from_json(
        &json_of(find(given, "menu").ok_or("decision choose requires a menu parameter")?)
            .ok_or("decision menu must be a JSON array")?,
    )?;
    let evidence = match find(given, "data") {
        None => match state {
            Json::Object(members) => Some(members.clone()),
            _ => None,
        },
        Some(data) => match json_of(data) {
            Some(Json::Object(members)) => Some(members),
            _ => return Err("decision data must be a JSON object"),
        },
    };
    let threshold = match find(given, "threshold") {
        None => 0.0,
        Some(Data::Number(number)) if (0.0..=1.0).contains(&number.get()) => number.get(),
        Some(_) => return Err("decision threshold must be a number in [0, 1]"),
    };
    Ok(Kind::Choose {
        menu,
        evidence,
        threshold,
    })
}

/// The validated decision request of an invocation's parameters (see the
/// crate documentation, "Parameters").
///
/// # Errors
///
/// A static detail for an unknown or repeated parameter, a missing or
/// malformed one, or a value outside its bound.
pub fn parse_parameters<'a>(
    parameters: impl IntoIterator<Item = (&'a str, &'a Data)>,
) -> Result<DecisionRequest, &'static str> {
    let mut given: Vec<(&str, &Data)> = Vec::new();
    for (name, value) in parameters {
        if given.iter().any(|(seen, _)| *seen == name) {
            return Err("decision takes each parameter once");
        }
        given.push((name, value));
    }
    let get = |name: &str| find(&given, name);
    let kind = match get("kind") {
        None => "choose",
        Some(Data::String(kind)) if matches!(kind.as_str(), "choose" | "rank" | "classify") => {
            kind.as_str()
        }
        Some(_) => return Err("decision kind must be choose, rank, or classify"),
    };
    let allowed: &[&str] = match kind {
        "choose" => &["menu", "data", "threshold"],
        "rank" => &["candidates"],
        _ => &["labels"],
    };
    if given.iter().any(|(name, _)| {
        !allowed.contains(name) && !matches!(*name, "kind" | "state" | "instructions" | "model")
    }) {
        return Err("decision takes no parameter of that name for its kind");
    }
    let state = json_of(get("state").ok_or("decision requires a state parameter")?)
        .filter(|state| matches!(state, Json::String(_) | Json::Object(_) | Json::Array(_)))
        .ok_or("decision state must be a string, a JSON object, or a JSON array")?;
    if exceeds_wasm_bound(canonical_json(&state).len(), MAX_STATE_BYTES) {
        return Err("decision state exceeds its bound");
    }
    let model = get("model")
        .map(|model| {
            text_of(
                model,
                MAX_MODEL_BYTES,
                "decision model must be a non-empty string within the model bound",
            )
        })
        .transpose()?;
    let default_instructions = match kind {
        "choose" => CHOOSE_INSTRUCTIONS,
        "rank" => RANK_INSTRUCTIONS,
        _ => CLASSIFY_INSTRUCTIONS,
    };
    let instructions = get("instructions")
        .map(|instructions| {
            text_of(
                instructions,
                MAX_INSTRUCTIONS_BYTES,
                "decision instructions must be a non-empty string within the instructions bound",
            )
        })
        .transpose()?
        .unwrap_or_else(|| default_instructions.to_owned());
    let kind = match kind {
        "choose" => choose_kind(&given, &state)?,
        "rank" => {
            let (candidates, criteria) =
                rank_criteria(get("candidates").ok_or("decision rank requires candidates")?)?;
            Kind::Rank {
                candidates,
                criteria,
            }
        }
        _ => Kind::Classify {
            criteria: label_criteria(get("labels").ok_or("decision classify requires labels")?)?,
        },
    };
    Ok(DecisionRequest {
        model,
        instructions,
        state,
        kind,
    })
}

/// What a request does next: return at once (nothing to ask), or ask.
#[derive(Debug)]
pub enum Plan {
    /// Emit this event and complete without a request (an empty menu).
    Return(Returned),
    /// Ask the backend this batch.
    Ask(Prepared),
}

/// A request ready to ask: its batch, and what decides from the answer.
#[derive(Debug)]
pub struct Prepared {
    batch: Batch,
    decide: Decide,
}

#[derive(Debug)]
enum Decide {
    Choose {
        menu: Vec<Candidate>,
        offered: Vec<Grounded>,
        threshold: f64,
    },
    Rank {
        candidates: Vec<Json>,
    },
    Classify,
}

const NO_ANSWER: FailureCode = FailureCode::Malformed;

/// The questions of one `choose` batch: the selection (every offered event,
/// then [`ABSTAIN_LABEL`]), then each offered event's member questions
/// (`fill.<event>.<member>`), in menu and `required` order.
fn choose_questions(
    menu: &[Candidate],
    offered: &[Grounded],
    instructions: String,
) -> Vec<Question> {
    let mut selection: Vec<Criterion> = offered
        .iter()
        .filter_map(|grounded| menu.get(grounded.candidate))
        .map(|candidate| Criterion {
            label: candidate.event.clone(),
            description: candidate
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
        let event = menu
            .get(grounded.candidate)
            .map_or("", |candidate| candidate.event.as_str());
        for (member_index, (member, criteria, _)) in grounded.members.iter().enumerate() {
            questions.push(Question {
                name: format!("fill.{event_index}.{member_index}"),
                instructions: format!(
                    "Which value fills {} for the event {event}? {FILL_INSTRUCTIONS}",
                    Json::String(member.clone()),
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
    offered: &[Grounded],
    chosen: usize,
    distributions: &[Distribution],
) -> Result<Json, FailureCode> {
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

impl DecisionRequest {
    /// What the request does next: an empty `choose` menu (nothing
    /// grounded) abstains at once; everything else asks one batch.
    ///
    /// # Errors
    ///
    /// A static detail when the batch exceeds a bound (the start is
    /// rejected).
    pub fn plan(self) -> Result<Plan, &'static str> {
        let Self {
            model,
            instructions,
            state,
            kind,
        } = self;
        let (questions, decide) = match kind {
            Kind::Choose {
                menu,
                evidence,
                threshold,
            } => {
                let offered: Vec<Grounded> = (0..menu.len())
                    .filter_map(|index| ground(&menu, index, evidence.as_ref()))
                    .collect();
                if offered.is_empty() {
                    return Ok(Plan::Return(Returned::abstain("no-candidates", None)));
                }
                (
                    choose_questions(&menu, &offered, instructions),
                    Decide::Choose {
                        menu,
                        offered,
                        threshold,
                    },
                )
            }
            Kind::Rank {
                candidates,
                criteria,
            } => (
                vec![Question {
                    name: "rank".into(),
                    instructions,
                    criteria,
                }],
                Decide::Rank { candidates },
            ),
            Kind::Classify { criteria } => (
                vec![Question {
                    name: "classify".into(),
                    instructions,
                    criteria,
                }],
                Decide::Classify,
            ),
        };
        if exceeds_wasm_bound(questions.len(), MAX_QUESTIONS) {
            return Err("decision batch exceeds the question bound");
        }
        Ok(Plan::Ask(Prepared {
            batch: Batch {
                model,
                state,
                questions,
            },
            decide,
        }))
    }
}

impl Prepared {
    /// The batch this request asks.
    #[must_use]
    pub const fn batch(&self) -> &Batch {
        &self.batch
    }

    /// The one request that asks the batch through `backend`.
    #[must_use]
    pub fn request<B: Backend>(&self, backend: &B, api_key: u32, base: Part) -> Request {
        backend.request(&self.batch, api_key, base)
    }

    /// The event the answer decides (see the crate documentation).
    #[must_use]
    pub fn decide<B: Backend>(
        &self,
        backend: &B,
        answered: Result<Response, TransportFailure>,
    ) -> Returned {
        let response = match answered {
            Ok(response) => response,
            Err(TransportFailure::Unavailable) => return Returned::failed(FailureCode::Transport),
            Err(TransportFailure::TooLarge) => return Returned::failed(FailureCode::Malformed),
        };
        if !(200..300).contains(&response.status) {
            return Returned::failed(FailureCode::Status);
        }
        let Ok(distributions) = backend.answers(&self.batch, &response.body) else {
            return Returned::failed(FailureCode::Malformed);
        };
        match self.decided(&distributions) {
            Ok(returned) => returned,
            Err(code) => Returned::failed(code),
        }
    }

    fn decided(&self, distributions: &[Distribution]) -> Result<Returned, FailureCode> {
        let question = self.batch.questions.first().ok_or(NO_ANSWER)?;
        let distribution = distributions.first().ok_or(NO_ANSWER)?;
        match &self.decide {
            Decide::Choose {
                menu,
                offered,
                threshold,
            } => {
                let chosen = backend::argmax(&question.criteria, distribution).ok_or(NO_ANSWER)?;
                let confidence = distribution
                    .probabilities
                    .get(chosen)
                    .copied()
                    .unwrap_or(0.0);
                let Some(grounded) = offered.get(chosen) else {
                    // The abstain label is the selection's last criterion.
                    return Ok(Returned::abstain("declined", Some(confidence)));
                };
                if confidence < *threshold {
                    return Ok(Returned::abstain("below-threshold", Some(confidence)));
                }
                let candidate = menu.get(grounded.candidate).ok_or(NO_ANSWER)?;
                let data = filled_data(offered, chosen, distributions)?;
                if !candidate
                    .compiled
                    .admits(&Data::JsonObject(canonical_json(&data)))
                {
                    return Err(FailureCode::SchemaMismatch);
                }
                Ok(Returned::new(&candidate.event, &data))
            }
            Decide::Rank { candidates } => {
                let mut ranked: Vec<(f64, &str, &Json)> = question
                    .criteria
                    .iter()
                    .zip(candidates)
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
                ranked.sort_by(|left, right| {
                    right.0.total_cmp(&left.0).then_with(|| left.1.cmp(right.1))
                });
                let ranking: Vec<Json> = ranked
                    .into_iter()
                    .map(|(score, _, candidate)| {
                        serde_json::json!({ "candidate": candidate, "score": score })
                    })
                    .collect();
                Ok(Returned::new(
                    RANKED_EVENT,
                    &serde_json::json!({ "ranking": ranking }),
                ))
            }
            Decide::Classify => {
                let labels: Map<String, Json> = question
                    .criteria
                    .iter()
                    .enumerate()
                    .map(|(index, criterion)| {
                        (
                            criterion.label.clone(),
                            distribution
                                .probabilities
                                .get(index)
                                .copied()
                                .unwrap_or(0.0)
                                .into(),
                        )
                    })
                    .collect();
                Ok(Returned::new(
                    CLASSIFIED_EVENT,
                    &serde_json::json!({ "labels": labels }),
                ))
            }
        }
    }
}

/// The package's first declared `http-origins` entry: the base URL when no
/// `baseurl` value is held. Read from the manifest the package ships, so the
/// origin is written in one place; `None` only if that manifest declares no
/// origin.
#[cfg_attr(
    not(any(target_arch = "wasm32", test)),
    expect(dead_code, reason = "only the wasm component reads its package origin")
)]
fn package_origin() -> Option<String> {
    let manifest: Json =
        serde_json::from_str(include_str!("../package/decision.service.json")).ok()?;
    manifest
        .get("http-origins")?
        .as_array()?
        .first()?
        .as_str()
        .map(str::to_owned)
}

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    wit_bindgen::generate!({
        world: "invoked-service",
        path: "../../scxml.rs/crates/scxml-wit/wit/scxml",
        // The world's `wasi:clocks` interfaces too.
        generate_all,
    });

    use exports::stateforward::scxml::invoked_service_api::Guest;
    use stateforward::scxml::outbound_http::{self, BodyPart, HttpHeader, HttpRequest, TextPart};
    use stateforward::scxml::telemetry::{self, Operation, Outcome, SignalEvent};
    use stateforward::scxml::types::{
        ComponentError, DataValue, ErrorCode, ErrorVersion, Event, EventType, InvokeRequest,
        SlotReference,
    };

    use super::{
        FailureCode, Part, Plan, Prepared, Response, Returned, TransportFailure, TypeSafe,
    };

    /// The invocation's result: `ok` (no done data), or its error.
    type Done = Result<Option<DataValue>, ComponentError>;

    struct Component;

    fn error(code: ErrorCode, detail: &'static str) -> ComponentError {
        ComponentError {
            version: ErrorVersion::V1,
            code,
            detail: (detail.len() <= 160).then(|| detail.into()),
        }
    }

    fn text_parts(parts: &[Part]) -> Vec<TextPart> {
        parts
            .iter()
            .map(|part| match part {
                Part::Literal(text) => TextPart::Literal(text.clone()),
                Part::Reference(reference) => TextPart::Reference(*reference),
            })
            .collect()
    }

    fn native_value(value: DataValue) -> Result<scxml_types::DataValue, ComponentError> {
        let invalid = || error(ErrorCode::InvalidInput, "a parameter value is invalid");
        Ok(match value {
            DataValue::Absent => scxml_types::DataValue::Absent,
            DataValue::Undefined => scxml_types::DataValue::Undefined,
            DataValue::Null => scxml_types::DataValue::Null,
            DataValue::Boolean(value) => scxml_types::DataValue::Bool(value),
            DataValue::Number(value) => scxml_types::DataValue::Number(
                scxml_types::FiniteNumber::new(value).map_err(|_| invalid())?,
            ),
            DataValue::Text(value) => scxml_types::DataValue::String(value),
            DataValue::JsonArray(value) => {
                scxml_types::DataValue::canonical_json_array(&value).map_err(|_| invalid())?
            }
            DataValue::JsonObject(value) => {
                scxml_types::DataValue::canonical_json_object(&value).map_err(|_| invalid())?
            }
            DataValue::Xml(value) => scxml_types::DataValue::Xml(value),
        })
    }

    fn event(returned: &Returned) -> Event {
        Event {
            name: returned.name.clone(),
            event_type: EventType::External,
            send_id: None,
            origin: None,
            origin_type: None,
            invoke_id: None,
            data: DataValue::JsonObject(returned.data.clone()),
            raw: None,
            trace_context: None,
        }
    }

    fn slot(references: &[SlotReference], name: &str) -> Option<u32> {
        references
            .iter()
            .find(|bound| bound.slot == name)
            .map(|bound| bound.reference)
    }

    /// What `invoke` found: the one event to return at once, or the batch
    /// to ask first.
    enum Next {
        Return(Returned),
        Ask(Box<Prepared>, HttpRequest),
    }

    fn next(request: InvokeRequest) -> Result<Next, ComponentError> {
        if request.type_uri != super::SERVICE_URI {
            return Err(error(ErrorCode::InvalidInput, "service type URI differs"));
        }
        let parameters = request
            .parameters
            .into_iter()
            .map(|entry| Ok((entry.name, native_value(entry.value)?)))
            .collect::<Result<Vec<_>, ComponentError>>()?;
        let plan = super::parse_parameters(
            parameters
                .iter()
                .map(|(name, value)| (name.as_str(), value)),
        )
        .and_then(super::DecisionRequest::plan)
        .map_err(|detail| error(ErrorCode::InvalidInput, detail))?;
        let prepared = match plan {
            Plan::Return(returned) => return Ok(Next::Return(returned)),
            Plan::Ask(prepared) => prepared,
        };
        let Some(api_key) = slot(&request.references, super::API_KEY_SLOT) else {
            return Ok(Next::Return(Returned::failed(FailureCode::Unconfigured)));
        };
        let base = match slot(&request.references, super::BASE_URL_SLOT) {
            Some(reference) => super::Part::Reference(reference),
            None => match super::package_origin() {
                Some(origin) => super::Part::Literal(origin),
                None => return Ok(Next::Return(Returned::failed(FailureCode::Unconfigured))),
            },
        };
        let built = prepared.request(&TypeSafe, api_key, base);
        let http = HttpRequest {
            method: "POST".into(),
            url: text_parts(&built.url),
            headers: built
                .headers
                .iter()
                .map(|(name, value)| HttpHeader {
                    name: name.clone(),
                    value: text_parts(value),
                })
                .collect(),
            body: vec![BodyPart::Bytes(built.body)],
        };
        Ok(Next::Ask(Box::new(prepared), http))
    }

    #[allow(
        clippy::unused_async_trait_impl,
        reason = "the WIT `invoke` is an `async func`; it returns once its task is spawned"
    )]
    impl Guest for Component {
        async fn invoke(
            request: InvokeRequest,
            _events: wit_bindgen::StreamReader<Event>,
        ) -> Result<
            (
                wit_bindgen::StreamReader<Event>,
                wit_bindgen::FutureReader<Done>,
            ),
            ComponentError,
        > {
            // The service accepts no event: its input stream is dropped.
            let next = observed(Operation::Start, || next(request))?;
            let (mut returned, returned_reader) = wit_stream::new::<Event>();
            let (done, done_reader) = wit_future::new::<Done>(|| {
                Err(error(
                    ErrorCode::Platform,
                    "the service ended without a result",
                ))
            });
            wit_bindgen::spawn(async move {
                let decided = match next {
                    Next::Return(decided) => decided,
                    // Suspends only this task, never the invoking session.
                    Next::Ask(prepared, http) => {
                        let answered = outbound_http::fetch(http)
                            .await
                            .map(|response| Response {
                                status: response.status,
                                body: response.body,
                            })
                            .map_err(|failure| match failure.code {
                                ErrorCode::PayloadTooLarge => TransportFailure::TooLarge,
                                _ => TransportFailure::Unavailable,
                            });
                        prepared.decide(&TypeSafe, answered)
                    }
                };
                telemetry::event(
                    Operation::Send,
                    if decided.name == super::FAILED_EVENT {
                        SignalEvent::InputRejected
                    } else {
                        SignalEvent::TransformationCompleted
                    },
                    None,
                );
                // The one event, then the result: `done.invoke.<id>` follows
                // it.
                if returned.write_one(event(&decided)).await.is_some() {
                    return;
                }
                drop(returned);
                let _ = done.write(Ok(None)).await;
            });
            Ok((returned_reader, done_reader))
        }
    }

    /// One call's closed telemetry: its outcome and one event (no attribute
    /// carries input).
    fn observed<T>(
        operation: Operation,
        body: impl FnOnce() -> Result<T, ComponentError>,
    ) -> Result<T, ComponentError> {
        let result = body();
        let outcome = match &result {
            Ok(_) => Outcome::Ok,
            Err(failure) if failure.code == ErrorCode::InvalidInput => Outcome::Rejected,
            Err(_) => Outcome::Failed,
        };
        telemetry::observe(operation, outcome, None, None);
        telemetry::event(
            operation,
            if result.is_ok() {
                SignalEvent::TransformationCompleted
            } else {
                SignalEvent::InputRejected
            },
            None,
        );
        result
    }

    export!(Component);
}

#[cfg(test)]
mod tests;
