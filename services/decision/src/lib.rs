//! The `decision` invoked service: one decision per `<invoke>`, asked as
//! `TypeSafe`'s `system_one` `choice` questions (decision D243): the one
//! chosen event with its confidence, or a pass to a later tier, as a
//! `stateforward:scxml/invoked-service` Wasm component.
//!
//! Asking a backend is slow I/O, so a decision is a service, never
//! executable content: the invoking session keeps processing events and
//! timers while the question is in flight, a `<send delay>` timeout in the
//! invoking state preempts it, and leaving that state cancels it. The
//! synchronous half stays executable content: `<decision:choice>`
//! (`namespaces/decision`) reads the enabled `event:schema` transitions from
//! the session snapshot and writes the canonical choices this service takes as
//! its `choices` parameter.
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
//! <onentry><decision:choice location="choices"/></onentry>
//! <invoke type="https://github.com/stateforward/floppy/services/decision"
//!         env:apikey="TYPESAFE_API_KEY" env:baseurl="TYPESAFE_BASE_URL">
//!   <param name="choices" expr="choices"/>
//!   <param name="state" expr="({failures: failures})"/>
//! </invoke>
//! ```
//!
//! # Parameters (`<param>`)
//!
//! * `state` (required): the state document the backend decides about, a
//!   string, a JSON object, or a JSON array (the decision layer's is the
//!   model-facing text of its stimulus, `{"turn": T}`, decision D150).
//! * `choices` (required): a JSON array of objects with `event`, an event
//!   name, `schema`, a self-contained JSON Schema, and an optional
//!   `description` string, as `<decision:choice>` writes it, and an optional
//!   `target` string, as `<processing:collect_offered_events>` tags it; an
//!   event is listed once, or once per target with the same schema, and is
//!   one candidate; no event is named a reserved pass criterion.
//! * `instructions` (optional, a non-empty string): what to decide; the
//!   selection's default without one.
//! * `model` (optional, a non-empty string): the backend's model; the
//!   backend's default without one.
//! * at most one of `data` (a JSON object of evidence) and `stimulus` (the
//!   stimulus the decision is about, read only for grounding: the evidence
//!   is the `data` member of a JSON object whose `event` is a string, else
//!   the stimulus itself, when that is a JSON object; no other member is
//!   read); without either, an object `state` is the evidence.
//! * `hand_ons` (optional): a non-empty JSON array of distinct tiers,
//!   `intuition` or `reasoning`, the later tiers a pass may hand the turn
//!   to; `["reasoning"]` without one (decision D150).
//! * `max_context_tokens` (optional: the model's context in tokens, a whole
//!   number from 1 to [`MAX_CONTEXT_TOKENS`], or `null` to send the request
//!   whole) and `bytes_per_token` (the invoking document's learned
//!   estimate, in `(0, 3]`, [`DEFAULT_BYTES_PER_TOKEN`] without one): see
//!   "Context fit".
//!
//! Any other parameter, a parameter given twice, or a malformed value
//! rejects the start: the `<invoke>` raises `error.execution` and nothing
//! is sent. The
//! `env` slots: `env:apikey="NAME"` (a bearer token) and
//! `env:baseurl="NAME"` (the API base URL, for example
//! `https://api.typesafe.ai`; it must not end with `/`). When no `baseurl`
//! value is held, a held key goes to the package's first declared
//! `http-origins` entry (`https://api.typesafe.ai`, read from
//! `package/decision.service.json`), as the deleted scxml.rs `openai`
//! namespace defaulted to its provider's URL; the manifest stays the one
//! place the origin is written. Without a held key, a held base URL is
//! asked keyless (no authorization header is sent, decision D127); with
//! neither, the service sends nothing and answers `unconfigured`.
//!
//! # The decision
//!
//! One batch of a `TypeSafe` backend:
//!
//! * **The deliberative handoff.** The event [`DELIBERATIVE_HANDOFF_EVENT`]
//!   is never offered (it is skipped by its name), so a decision about it
//!   is reasoning's.
//! * **Grounded data.** The data of each offered event is a JSON object
//!   holding its schema's top-level `required` members, each chosen by its
//!   own question among grounded options, never authored: the evidence's
//!   non-null member of that name (the evidence is `data`, the stimulus's
//!   data, else the state when it is an object), then the property's
//!   non-null `enum` values. An event with a required member that has no
//!   option is not offered.
//! * **The batch.** The question `selection` criteria are every offered
//!   event (described by the first line of its description, else by its
//!   name) plus one pass criterion per tier of `hand_ons` ([`HandOn`]:
//!   `__intuition__` and `__reasoning__`, each with its own wording,
//!   decision D150); each offered event's required member is its own
//!   question `<event>::<member>` whose criteria are its options. When
//!   nothing is offered, the service returns [`ABSTAIN_EVENT`]
//!   `{"reason": "no-candidates"}` without a request; when a pass criterion
//!   wins, [`ABSTAIN_EVENT`]
//!   `{"reason": "declined", "confidence": P, "hand_to": tier}` (an
//!   explicit decline naming its tier).
//! * **Only the chosen option.** The service returns [`SELECTED_EVENT`]
//!   `{"selections": [{"event": E, "data": D, "confidence": P, "target": T}]}`:
//!   only the chosen event (decision D150: the other options'
//!   probabilities are not selections), with its filled data, its
//!   probability in the selection question as its confidence, and `target`
//!   its only offerer, the one `target` its listings name (decision D113;
//!   absent when no listing or several name one); the
//!   invoking document sends it to its target (or, untargeted, to itself).
//! * **Validation before returning.** The filled data of the returned
//!   event must satisfy its schema from the choices; otherwise the service
//!   returns [`FAILED_EVENT`] `{"code": "schema-mismatch"}` and never the
//!   event.
//! * **Context fit.** With `max_context_tokens`, a request body over
//!   `floor(max_context_tokens * bytes_per_token)` bytes has the selection's
//!   instructions cut to as many leading whole lines as fit, then to the
//!   selection's default instructions (decision D150); the
//!   state and every criterion are never cut, and a request still over is
//!   sent anyway, so the backend's refusal is the decision's failure.
//!
//! # Events
//!
//! `invoke` (the component's one export, an `async func`) writes exactly
//! one event to the stream it returns, then resolves its result future: with
//! no data when nothing was sent, else with `{"request_bytes": N,
//! "context_fit": O, "usage": {"input_tokens": I, "output_tokens": W}}`
//! (`done.invoke.<id>`'s data): the request body's bytes, its fit (`fit`,
//! `cut` or `overflow`, only with `max_context_tokens`), and the tokens a
//! successful answer says the backend counted, from which the invoking
//! document learns its bytes per token (decisions D143, D150). A failure returns [`FAILED_EVENT`] `{"code": C}` with one closed
//! [`FailureCode`]. The chosen event is defined by the invoking
//! document (its choices' `event:schema` transitions), and the document
//! describes this service's own events with `event:schema` too
//! (`bots/builder/builder.scx`); the service validates what it returns
//! itself: the selected event's data against the choices' schema for it,
//! and its own events by construction (their shapes are
//! documented here). An event name the choices offer is a valid SCXML event
//! name with no `*` and no empty token. The service reads no event: it
//! drops the event stream `invoke` receives. `outbound-http.fetch` is an
//! `async func`: awaiting it suspends only the invocation's own task, and
//! leaving the invoking state cancels the invocation (SCXML 6.4), which
//! closes the request's connection.
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
//! `max_context_tokens` is range-checked on every build to at most
//! [`MAX_CONTEXT_TOKENS`], which keeps the context fit's arithmetic exact.
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

pub use backend::{Backend, Batch, Criterion, Distribution, Question, TypeSafe, Usage};

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

/// The event a choice returns when it takes no event.
pub const ABSTAIN_EVENT: &str = "decision.abstain";
/// The event a choice returns with the chosen event.
pub const SELECTED_EVENT: &str = "decision.selected";
/// The event a failed decision returns.
pub const FAILED_EVENT: &str = "decision.failed";

/// A later tier a pass hands the turn to (decision D150): each
/// has its own reserved pass criterion, which the choices never name, and
/// its own wording (never as leaving the turn unhandled).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandOn {
    /// Answer it in words now, with intuition.
    Intuition,
    /// Think it through first, with reasoning.
    Reasoning,
}

impl HandOn {
    /// Every tier, earliest first.
    pub const ALL: [Self; 2] = [Self::Intuition, Self::Reasoning];

    /// The tier's name (`hand_ons` items and `hand_to`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Intuition => "intuition",
            Self::Reasoning => "reasoning",
        }
    }

    /// The tier's reserved pass criterion.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Intuition => "__intuition__",
            Self::Reasoning => "__reasoning__",
        }
    }

    /// What passing to the tier does.
    #[must_use]
    pub const fn wording(self) -> &'static str {
        match self {
            Self::Intuition => "None of these fits: answer it in words now, with intuition.",
            Self::Reasoning => {
                "None of these fits: it needs thought, so think it through first, with reasoning."
            }
        }
    }

    fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tier| tier.name() == name)
    }
}

/// Whether `label` is a reserved pass criterion, which no choice may name.
fn reserved_label(label: &str) -> bool {
    HandOn::ALL.iter().any(|tier| tier.label() == label)
}

/// The bytes of request body a token before an answer teaches fewer (the
/// starting estimate, decisions D143, D150).
pub const DEFAULT_BYTES_PER_TOKEN: f64 = 3.0;

/// The selection question's name in a batch.
pub const SELECTION_QUESTION: &str = "selection";

/// The deliberative handoff (the event whose schema is marked
/// `__deliberative_handoff__`): never offered, skipped by this name.
pub const DELIBERATIVE_HANDOFF_EVENT: &str = "bot.ability.reasoning.input";

/// The selection's default instructions.
const SELECTION_INSTRUCTIONS: &str = "Choose the single criterion that answers this turn from the bot's own peripheral transducers: the criterion text names the situation. Choose the pass criterion when none offered fits.";
/// The instructions of a member question.
const FILL_INSTRUCTIONS: &str = "Fill one payload field for the selected event: the criteria are the exact values the evidence offers. Pick the value the turn supports.";

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
/// Upper bound on a choice event name's bytes (wasm builds only).
pub const MAX_EVENT_NAME_BYTES: usize = wasm_bound(256);
/// Upper bound on `max_context_tokens` (a model's context, in tokens).
pub const MAX_CONTEXT_TOKENS: u32 = 1 << 30;

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
    /// Neither the `apikey` nor the `baseurl` slot names a variable the run
    /// holds, or no base URL is known.
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
    /// The event name: one of this service's events.
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

    fn abstain(reason: &str, confidence: Option<f64>, hand_to: Option<HandOn>) -> Self {
        let mut data = Map::new();
        data.insert("reason".into(), reason.into());
        if let Some(confidence) = confidence {
            data.insert("confidence".into(), confidence.into());
        }
        if let Some(tier) = hand_to {
            data.insert("hand_to".into(), tier.name().into());
        }
        Self::new(ABSTAIN_EVENT, &Json::Object(data))
    }
}

/// One event a choice may return.
#[derive(Debug)]
pub struct Candidate {
    /// The event name.
    pub event: String,
    /// The resolved schema.
    pub schema: Json,
    /// The schema compiled for checking the filled data.
    pub compiled: CompiledSchema,
    /// The first line of what the event means, when known.
    pub description: Option<String>,
    /// The targets that offer it, in listing order (the session ids
    /// `<processing:collect_offered_events>` tags them with).
    pub targets: Vec<String>,
}

/// The first non-blank line of `text`, trimmed: an event's wording.
fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
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

/// The candidates of a `choices` value: an array of objects with `event`,
/// `schema`, an optional `description`, and an optional `target` (the
/// offering session's id, as `<processing:collect_offered_events>` tags it).
///
/// An event is one candidate however many targets offer it (one event per
/// name): it may be listed
/// again only when every listing has a `target` and the same schema, and
/// the first listing's description is kept. The targets are never part of
/// the decision; a selection carries its only one.
///
/// # Errors
///
/// A static detail for a malformed candidate, a duplicate event without
/// distinct targets or with another schema, or a schema that is not a
/// self-contained JSON Schema.
pub fn choices_from_json(value: &Json) -> Result<Vec<Candidate>, &'static str> {
    const INVALID: &str = "decision choices must be an array of objects with an event name, a schema, an optional description, and an optional target";
    const TWICE: &str = "decision choices name an event twice";
    let items = value.as_array().ok_or(INVALID)?;
    if exceeds_wasm_bound(items.len(), MAX_CRITERIA) {
        return Err("decision choices exceed the candidate bound");
    }
    let mut choices: Vec<Candidate> = Vec::new();
    // Per candidate: its schema as given, and the targets that offer it.
    let mut listings: Vec<(&Json, Vec<&str>)> = Vec::new();
    for item in items {
        let members = item.as_object().ok_or(INVALID)?;
        if members
            .keys()
            .any(|key| !matches!(key.as_str(), "event" | "schema" | "description" | "target"))
        {
            return Err(INVALID);
        }
        let event = members
            .get("event")
            .and_then(Json::as_str)
            .filter(|event| valid_event_name(event))
            .ok_or(INVALID)?;
        if reserved_label(event) {
            return Err("decision choices name a reserved pass criterion");
        }
        let target = match members.get("target") {
            None => None,
            Some(Json::String(target)) if !target.is_empty() => Some(target.as_str()),
            Some(_) => return Err(INVALID),
        };
        let source = members.get("schema").ok_or(INVALID)?;
        if members
            .get("description")
            .is_some_and(|text| !text.is_string())
        {
            return Err(INVALID);
        }
        if let Some(index) = choices
            .iter()
            .position(|candidate| candidate.event == event)
        {
            let (schema, targets) = listings.get_mut(index).ok_or(TWICE)?;
            let Some(target) = target else {
                return Err(TWICE);
            };
            if targets.is_empty() || targets.contains(&target) || *schema != source {
                return Err(TWICE);
            }
            targets.push(target);
            if let Some(candidate) = choices.get_mut(index) {
                candidate.targets.push(target.to_owned());
            }
            continue;
        }
        listings.push((source, target.into_iter().collect()));
        let resolved = scxml_event_schema::resolve(canonical_json(source).as_bytes(), None)
            .map_err(|_| "decision choices schema is not a self-contained JSON Schema")?;
        let schema: Json = serde_json::from_str(&resolved.schema).map_err(|_| INVALID)?;
        let compiled = CompiledSchema::new(&resolved.schema)
            .map_err(|_| "decision choices schema is not a self-contained JSON Schema")?;
        let description = match members.get("description") {
            None => schema
                .get("description")
                .and_then(Json::as_str)
                .and_then(first_line),
            Some(Json::String(text)) => first_line(text),
            Some(_) => return Err(INVALID),
        };
        choices.push(Candidate {
            event: event.to_owned(),
            schema,
            compiled,
            description,
            targets: target.into_iter().map(str::to_owned).collect(),
        });
    }
    Ok(choices)
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

/// A candidate (by choices index) with grounded options for every required
/// member.
#[derive(Debug)]
struct Grounded {
    candidate: usize,
    /// `(member, criteria, values)` in the schema's `required` order.
    members: Vec<(String, Vec<Criterion>, Vec<Json>)>,
}

/// The grounded options of every required member of `choices[index]`, or
/// `None` when one has no option (the event is then not offered).
fn ground(
    choices: &[Candidate],
    index: usize,
    evidence: Option<&Map<String, Json>>,
) -> Option<Grounded> {
    let candidate = choices.get(index)?;
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
        // A member's options: the evidence's value, then the schema's enum
        // options, never null.
        if let Some(value) = evidence
            .and_then(|evidence| evidence.get(member))
            .filter(|value| !value.is_null())
        {
            push(value);
        }
        if let Some(Json::Array(options)) = property.and_then(|property| property.get("enum")) {
            options
                .iter()
                .filter(|value| !value.is_null())
                .for_each(&mut push);
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
    fit: Option<ContextFit>,
    choices: Vec<Candidate>,
    /// The evidence the required members are grounded in.
    evidence: Option<Map<String, Json>>,
    /// The tiers a pass may hand the turn to, one pass criterion each.
    hand_ons: Vec<HandOn>,
}

/// A model's context window in bytes of request body (decision D150): the
/// invoking document holds the learned
/// estimate across requests and passes it in, so the service stays stateless.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContextFit {
    /// The model's context, in tokens.
    pub max_context_tokens: u32,
    /// Bytes of request body a token: [`DEFAULT_BYTES_PER_TOKEN`] until an
    /// answer teaches fewer.
    pub bytes_per_token: f64,
}

impl ContextFit {
    /// The request body bytes that fit.
    #[must_use]
    pub fn budget_bytes(&self) -> usize {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a whole number in 0..=MAX_CONTEXT_TOKENS * 3, both checked at the parameter"
        )]
        let budget = (f64::from(self.max_context_tokens) * self.bytes_per_token).floor() as usize;
        budget
    }
}

/// How a request was fitted to a model's context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextFitOutcome {
    /// Sent whole.
    Fit,
    /// The selection's instructions were cut to fit.
    Cut,
    /// Still over at its smallest, and sent anyway.
    Overflow,
}

impl ContextFitOutcome {
    /// The wire name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Fit => "fit",
            Self::Cut => "cut",
            Self::Overflow => "overflow",
        }
    }
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

/// The value of the parameter `name`, when given.
fn find<'a>(given: &[(&str, &'a Data)], name: &str) -> Option<&'a Data> {
    given
        .iter()
        .find(|(seen, _)| *seen == name)
        .map(|(_, value)| *value)
}

/// The evidence a stimulus grounds: the data of
/// a stimulus object whose `event` is a string, else the stimulus itself;
/// evidence only when that is a JSON object.
fn stimulus_evidence(stimulus: Json) -> Option<Map<String, Json>> {
    let data = match stimulus {
        Json::Object(mut members) if members.get("event").is_some_and(Json::is_string) => {
            members.remove("data").unwrap_or(Json::Null)
        }
        other => other,
    };
    match data {
        Json::Object(members) => Some(members),
        _ => None,
    }
}

/// The tiers of a `hand_ons` value: a non-empty JSON array of distinct tier
/// names.
fn hand_ons_of(value: &Data) -> Result<Vec<HandOn>, &'static str> {
    const INVALID: &str =
        "decision hand_ons must be a non-empty array of distinct tiers, intuition or reasoning";
    let Some(Json::Array(items)) = json_of(value) else {
        return Err(INVALID);
    };
    let mut tiers: Vec<HandOn> = Vec::new();
    for item in &items {
        let tier = item.as_str().and_then(HandOn::named).ok_or(INVALID)?;
        if tiers.contains(&tier) {
            return Err(INVALID);
        }
        tiers.push(tier);
    }
    if tiers.is_empty() {
        return Err(INVALID);
    }
    Ok(tiers)
}

/// The context fit of `max_context_tokens` (a positive whole number within
/// [`MAX_CONTEXT_TOKENS`], or `null` to send every request whole) and
/// `bytes_per_token` (in `(0, 3]`, [`DEFAULT_BYTES_PER_TOKEN`] when absent).
fn context_fit(
    max_context_tokens: Option<&Data>,
    bytes_per_token: Option<&Data>,
) -> Result<Option<ContextFit>, &'static str> {
    let bytes_per_token = match bytes_per_token {
        None => DEFAULT_BYTES_PER_TOKEN,
        Some(Data::Number(number))
            if number.get() > 0.0 && number.get() <= DEFAULT_BYTES_PER_TOKEN =>
        {
            number.get()
        }
        Some(_) => return Err("decision bytes_per_token must be a number in (0, 3]"),
    };
    match max_context_tokens {
        None | Some(Data::Null) => Ok(None),
        Some(Data::Number(number))
            if number.get().fract() == 0.0
                && number.get() >= 1.0
                && number.get() <= f64::from(MAX_CONTEXT_TOKENS) =>
        {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a whole number checked to lie in 1..=MAX_CONTEXT_TOKENS"
            )]
            let max_context_tokens = number.get() as u32;
            Ok(Some(ContextFit {
                max_context_tokens,
                bytes_per_token,
            }))
        }
        Some(_) => Err("decision max_context_tokens must be a positive whole number or null"),
    }
}

/// The parameters a request takes.
const PARAMETERS: [&str; 9] = [
    "state",
    "choices",
    "instructions",
    "model",
    "data",
    "stimulus",
    "hand_ons",
    "max_context_tokens",
    "bytes_per_token",
];

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
    if given.iter().any(|(name, _)| !PARAMETERS.contains(name)) {
        return Err("decision takes no parameter of that name");
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
    let instructions = get("instructions")
        .map(|instructions| {
            text_of(
                instructions,
                MAX_INSTRUCTIONS_BYTES,
                "decision instructions must be a non-empty string within the instructions bound",
            )
        })
        .transpose()?
        .unwrap_or_else(|| SELECTION_INSTRUCTIONS.to_owned());
    let fit = context_fit(get("max_context_tokens"), get("bytes_per_token"))?;
    let choices = choices_from_json(
        &json_of(get("choices").ok_or("decision requires a choices parameter")?)
            .ok_or("decision choices must be a JSON array")?,
    )?;
    // The evidence: `data`, else the stimulus's object data, else an
    // object `state`.
    let evidence = match (get("stimulus"), get("data")) {
        (Some(_), Some(_)) => {
            return Err("decision takes a stimulus or data as evidence, not both");
        }
        (Some(stimulus), None) => {
            stimulus_evidence(json_of(stimulus).ok_or("decision stimulus must be a JSON value")?)
        }
        (None, None) => match &state {
            Json::Object(members) => Some(members.clone()),
            _ => None,
        },
        (None, Some(data)) => match json_of(data) {
            Some(Json::Object(members)) => Some(members),
            _ => return Err("decision data must be a JSON object"),
        },
    };
    // Reasoning alone when absent: a pass goes to reasoning when the turn
    // names none (decision D150).
    let hand_ons = get("hand_ons")
        .map(hand_ons_of)
        .transpose()?
        .unwrap_or_else(|| vec![HandOn::Reasoning]);
    Ok(DecisionRequest {
        model,
        instructions,
        state,
        fit,
        choices,
        evidence,
        hand_ons,
    })
}

/// What a request does next: return at once (nothing to ask), or ask.
#[derive(Debug)]
pub enum Plan {
    /// Emit this event and complete without a request (an empty choice list).
    Return(Returned),
    /// Ask the backend this batch.
    Ask(Prepared),
}

/// A request ready to ask: its batch, what decides from the answer, and how
/// the request is fitted to the model's context.
#[derive(Debug)]
pub struct Prepared {
    batch: Batch,
    choices: Vec<Candidate>,
    offered: Vec<Grounded>,
    hand_ons: Vec<HandOn>,
    fit: Option<ContextFit>,
    /// The body bytes and fit of the request last built ([`Prepared::request`]).
    sent: Option<(usize, Option<ContextFitOutcome>)>,
}

const NO_ANSWER: FailureCode = FailureCode::Malformed;

/// The questions of one batch: the selection (every offered event, then
/// one pass criterion per tier of `hand_ons`), then each offered event's
/// member questions
/// (`<event>::<member>`, each criterion described by the question's
/// wording), in choices and `required` order.
///
/// # Errors
///
/// A static detail when two member questions would share a name.
fn event_questions(
    choices: &[Candidate],
    offered: &[Grounded],
    hand_ons: &[HandOn],
    instructions: String,
) -> Result<Vec<Question>, &'static str> {
    let mut selection: Vec<Criterion> = offered
        .iter()
        .filter_map(|grounded| choices.get(grounded.candidate))
        .map(|candidate| Criterion {
            label: candidate.event.clone(),
            description: Json::String(
                candidate
                    .description
                    .clone()
                    .unwrap_or_else(|| candidate.event.clone()),
            ),
        })
        .collect();
    selection.extend(hand_ons.iter().map(|tier| Criterion {
        label: tier.label().into(),
        description: tier.wording().into(),
    }));
    let mut questions = vec![Question {
        name: SELECTION_QUESTION.into(),
        instructions,
        criteria: selection,
    }];
    let mut names = BTreeSet::from([SELECTION_QUESTION.to_owned()]);
    for grounded in offered {
        let event = choices
            .get(grounded.candidate)
            .map_or("", |candidate| candidate.event.as_str());
        for (member, criteria, _) in &grounded.members {
            let name = format!("{event}::{member}");
            if !names.insert(name.clone()) {
                return Err("decision member questions would share a name");
            }
            let wording = format!(
                "Which value fills {} for {event}?",
                Json::String(member.clone())
            );
            questions.push(Question {
                name,
                instructions: format!("{wording} {FILL_INSTRUCTIONS}"),
                criteria: criteria
                    .iter()
                    .map(|criterion| Criterion {
                        label: criterion.label.clone(),
                        description: Json::String(wording.clone()),
                    })
                    .collect(),
            });
        }
    }
    Ok(questions)
}

/// The data of the offered event at `chosen`: each required member's most
/// probable grounded option ([`event_questions`] order).
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
    /// What the request does next: choices with nothing grounded abstain
    /// at once; everything else asks one batch.
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
            fit,
            choices,
            evidence,
            hand_ons,
        } = self;
        let offered: Vec<Grounded> = (0..choices.len())
            .filter(|index| {
                choices
                    .get(*index)
                    .is_some_and(|candidate| candidate.event != DELIBERATIVE_HANDOFF_EVENT)
            })
            .filter_map(|index| ground(&choices, index, evidence.as_ref()))
            .collect();
        if offered.is_empty() {
            return Ok(Plan::Return(Returned::abstain("no-candidates", None, None)));
        }
        let questions = event_questions(&choices, &offered, &hand_ons, instructions)?;
        if exceeds_wasm_bound(questions.len(), MAX_QUESTIONS) {
            return Err("decision batch exceeds the question bound");
        }
        Ok(Plan::Ask(Prepared {
            batch: Batch {
                model,
                state,
                questions,
            },
            choices,
            offered,
            hand_ons,
            fit,
            sent: None,
        }))
    }
}

impl Prepared {
    /// The batch this request asks.
    #[must_use]
    pub const fn batch(&self) -> &Batch {
        &self.batch
    }

    /// The one request that asks the batch through `backend`, fitted to the
    /// model's context when the request has a [`ContextFit`] (decision
    /// D150): over its budget, the selection's instructions keep
    /// as many leading whole lines as fit, then the selection's default
    /// instructions; the state and every criterion are never cut, and a
    /// request still over at its smallest is sent anyway (the backend's
    /// refusal is the decision's failure). The batch keeps the instructions
    /// sent.
    pub fn request<B: Backend>(
        &mut self,
        backend: &B,
        api_key: Option<u32>,
        base: Part,
    ) -> Request {
        let Some(fit) = self.fit else {
            let request = backend.request(&self.batch, api_key, base);
            self.sent = Some((request.body.len(), None));
            return request;
        };
        let budget = fit.budget_bytes();
        let instructed = |batch: &mut Batch, instructions: String| {
            if let Some(selection) = batch.questions.first_mut() {
                selection.instructions = instructions;
            }
            backend.request(batch, api_key, base.clone())
        };
        let mut request = backend.request(&self.batch, api_key, base.clone());
        let mut outcome = ContextFitOutcome::Fit;
        if request.body.len() > budget {
            let lines: Vec<String> = self
                .batch
                .questions
                .first()
                .map(|selection| split_lines(&selection.instructions))
                .unwrap_or_default();
            let mut best: Option<(String, Request)> = None;
            let (mut low, mut high) = (1, lines.len().saturating_sub(1));
            while low <= high {
                let count = low + (high - low) / 2;
                let instructions = lines.get(..count).unwrap_or_default().join("\n");
                let mut candidate = self.batch.clone();
                let built = instructed(&mut candidate, instructions.clone());
                if built.body.len() <= budget {
                    best = Some((instructions, built));
                    low = count + 1;
                } else {
                    high = count - 1;
                }
            }
            let (instructions, built) = best.unwrap_or_else(|| {
                let mut candidate = self.batch.clone();
                let default = SELECTION_INSTRUCTIONS.to_owned();
                let built = instructed(&mut candidate, default.clone());
                (default, built)
            });
            outcome = if built.body.len() <= budget {
                ContextFitOutcome::Cut
            } else {
                ContextFitOutcome::Overflow
            };
            if let Some(selection) = self.batch.questions.first_mut() {
                selection.instructions = instructions;
            }
            request = built;
        }
        self.sent = Some((request.body.len(), Some(outcome)));
        request
    }

    /// The done data of the request last built, once `answered`: its body
    /// bytes (`request_bytes`), its fit (`context_fit`, with a
    /// [`ContextFit`]) and the tokens a successful answer says the backend
    /// counted (`usage`), from which the invoking document learns its
    /// bytes per token (decisions D143, D150); `None`
    /// before a request was built.
    #[must_use]
    pub fn done_data<B: Backend>(
        &self,
        backend: &B,
        answered: &Result<Response, TransportFailure>,
    ) -> Option<String> {
        let (request_bytes, outcome) = self.sent?;
        let mut data = Map::new();
        data.insert("request_bytes".into(), request_bytes.into());
        if let Some(outcome) = outcome {
            data.insert("context_fit".into(), outcome.name().into());
        }
        if let Some(usage) = answered
            .as_ref()
            .ok()
            .filter(|response| (200..300).contains(&response.status))
            .and_then(|response| backend.usage(&response.body))
        {
            let mut tokens = Map::new();
            tokens.insert("input_tokens".into(), usage.input_tokens.into());
            if let Some(output_tokens) = usage.output_tokens {
                tokens.insert("output_tokens".into(), output_tokens.into());
            }
            data.insert("usage".into(), Json::Object(tokens));
        }
        Some(canonical_json(&Json::Object(data)))
    }

    /// The event the answer decides (see the crate documentation).
    #[must_use]
    pub fn decide<B: Backend>(
        &self,
        backend: &B,
        answered: &Result<Response, TransportFailure>,
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
        decided_events(
            &self.choices,
            &self.offered,
            &self.hand_ons,
            question,
            distribution,
            distributions,
        )
    }
}

/// The event an answer decides (see the crate documentation).
fn decided_events(
    choices: &[Candidate],
    offered: &[Grounded],
    hand_ons: &[HandOn],
    question: &Question,
    distribution: &Distribution,
    distributions: &[Distribution],
) -> Result<Returned, FailureCode> {
    let chosen = backend::argmax(&question.criteria, distribution).ok_or(NO_ANSWER)?;
    let probability = |index: usize| {
        distribution
            .probabilities
            .get(index)
            .copied()
            .unwrap_or(0.0)
    };
    if offered.get(chosen).is_none() {
        // The pass criteria follow the offered events, one per tier, in
        // `hand_ons` order.
        let hand_to = chosen
            .checked_sub(offered.len())
            .and_then(|index| hand_ons.get(index))
            .copied();
        return Ok(Returned::abstain(
            "declined",
            Some(probability(chosen)),
            hand_to,
        ));
    }
    // The event at `index` with its filled data, validated
    // against its schema, and the candidate it is.
    let event = |index: usize| -> Result<(&Candidate, Json), FailureCode> {
        let grounded = offered.get(index).ok_or(NO_ANSWER)?;
        let candidate = choices.get(grounded.candidate).ok_or(NO_ANSWER)?;
        let data = filled_data(offered, index, distributions)?;
        if !candidate
            .compiled
            .admits(&Data::JsonObject(canonical_json(&data)))
        {
            return Err(FailureCode::SchemaMismatch);
        }
        Ok((candidate, data))
    };
    let (candidate, data) = event(chosen)?;
    // Only the chosen option is returned (decision D150), with its
    // probability as its confidence and its only offerer as `target`
    // (decision D113), none when several offer it.
    let mut selection = serde_json::json!({
        "event": candidate.event,
        "data": data,
        "confidence": probability(chosen),
    });
    if let ([target], Some(selection)) = (candidate.targets.as_slice(), selection.as_object_mut()) {
        selection.insert("target".into(), Json::String(target.clone()));
    }
    Ok(Returned::new(
        SELECTED_EVENT,
        &serde_json::json!({ "selections": [selection] }),
    ))
}

/// The lines of `text` the instructions are cut by: split at `\n` or
/// `\r\n` ([`str::lines`]), no empty line after a final break.
fn split_lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
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
        let mut prepared = match plan {
            Plan::Return(returned) => return Ok(Next::Return(returned)),
            Plan::Ask(prepared) => prepared,
        };
        // A held key goes to the run's base URL, else the package's origin;
        // without a key, only a base URL the run names is asked, keyless.
        let api_key = slot(&request.references, super::API_KEY_SLOT);
        let base = match (slot(&request.references, super::BASE_URL_SLOT), api_key) {
            (Some(reference), _) => super::Part::Reference(reference),
            (None, Some(_)) => match super::package_origin() {
                Some(origin) => super::Part::Literal(origin),
                None => return Ok(Next::Return(Returned::failed(FailureCode::Unconfigured))),
            },
            (None, None) => return Ok(Next::Return(Returned::failed(FailureCode::Unconfigured))),
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
                let (decided, done_data) = match next {
                    Next::Return(decided) => (decided, None),
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
                        (
                            prepared.decide(&TypeSafe, &answered),
                            prepared.done_data(&TypeSafe, &answered),
                        )
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
                let _ = done.write(Ok(done_data.map(DataValue::JsonObject))).await;
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
