//! The `generate` invoked service: one model generation per `<invoke>`, as a
//! `stateforward:scxml/invoked-service` Wasm component.
//!
//! A model call is slow I/O, so it is a service, never executable content:
//! the invoking session keeps processing events and timers while the call is
//! in flight, a `<send delay>` timeout in the invoking state preempts it, and
//! leaving that state cancels it. The wire protocol is the
//! `OpenAI`-compatible Chat Completions API (`POST {base}/chat/completions`),
//! which `OpenAI`, `NVIDIA`'s `integrate.api.nvidia.com/v1`, and local
//! servers all speak; the backend is chosen by the run through the `baseurl`
//! slot, never by the document or the component. Documents name
//! [`SERVICE_URI`]:
//!
//! ```xml
//! <invoke idlocation="gen" type="https://github.com/stateforward/nemobot/services/generate@v0.1.0"
//!         env:apikey="OPENAI_API_KEY" env:baseurl="OPENAI_BASE_URL">
//!   <param name="model" expr="'gpt-6-luna'"/>
//!   <param name="prompt" expr="'Say hello.'"/>
//! </invoke>
//! ```
//!
//! # Parameters (`<param>`)
//!
//! * `model` (required, a non-empty string): sent as `model`.
//! * the conversation, exactly one of `prompt` (a non-empty string: one
//!   `user` message, used exactly) or `messages` (a non-empty JSON array of
//!   objects with exactly `role`, one of `system`, `user`, `assistant`, and
//!   `content`, a string).
//! * `system` (optional, a non-empty string): a `system` message before the
//!   conversation.
//! * `schema` (optional): a JSON Schema (a JSON object, or a string holding
//!   one) that satisfies its dialect's metaschema (2020-12 unless `$schema`
//!   names another supported dialect) and references nothing outside itself.
//!   With it the request asks for structured output (`response_format` of
//!   type `json_schema`), and the reply must parse as JSON and satisfy the
//!   schema **before** anything is emitted. `schemaname` (a string matching
//!   `[A-Za-z0-9_-]{1,64}`, [`DEFAULT_SCHEMA_NAME`] without it) and `strict`
//!   (a boolean, `false` without it) are sent with it and require it.
//!
//! Any other parameter, a parameter given twice, or a malformed value
//! rejects the start: the `<invoke>` raises `error.execution` and nothing is
//! sent.
//!
//! The `env` slots ([`API_KEY_SLOT`], [`BASE_URL_SLOT`]): `env:apikey="NAME"`
//! (sent as a bearer token) and `env:baseurl="NAME"` (the API base URL, for
//! example `https://api.openai.com/v1`; it must not end with `/`). Both are
//! required at run time and no backend URL is compiled in. The component
//! holds opaque references only; the host substitutes the values into the
//! one admitted request.
//!
//! # Events
//!
//! The service emits exactly one event, then completes (the invoking session
//! receives `done.invoke.<id>` after it):
//!
//! * [`RESULT_EVENT`] `{"text": T}` (no schema) or `{"json": V}` (with a
//!   schema; `V` satisfies it).
//! * [`FAILED_EVENT`] `{"code": C}` with one closed [`FailureCode`].
//!
//! Its package declares both with JSON Schemas (`package/events.json`), so
//! the host checks every emit against them, and declares that it accepts no
//! event (a `<send>` to it raises `error.execution`).
//!
//! # The request
//!
//! `POST {base}/chat/completions` with `authorization: Bearer <apikey>`,
//! `content-type: application/json`, `accept: application/json`, and a body
//! that is canonical JSON ([`scxml_event_schema::canonical_json`]: compact,
//! members sorted by key), so one request always produces the same bytes
//! ([`build_request`]):
//!
//! ```json
//! {"messages":[{"content":"…","role":"system"},{"content":"…","role":"user"}],
//!  "model":"…",
//!  "response_format":{"json_schema":{"name":"result","schema":{…},"strict":false},"type":"json_schema"}}
//! ```
//!
//! The request never blocks the session: `start` hands it to the host with
//! `outbound-http.begin`, and the `wake` the host calls when it settled takes
//! the outcome (`outbound-http.take`). `cancel` drops the service's state;
//! the host abandons the request.
//!
//! # The reply
//!
//! The reply is `choices[0].message.content`, which must be a string; a
//! `refusal`, a `finish_reason` of `length` or `content_filter`, or a body
//! without that shape is a failure ([`reply_text`]). With a schema the text
//! is parsed as JSON and checked against the compiled schema
//! ([`structured_value`]).
//!
//! # Bounds
//!
//! The conversation's bytes ([`MAX_CONVERSATION_BYTES`]), its message count
//! ([`MAX_MESSAGES`]), and the model name ([`MAX_MODEL_BYTES`]) are bounded on
//! wasm builds only ([`scxml_types::wasm_bound`]); the schema is bounded by
//! `scxml-event-schema`, and the reply by the host's response-body bound.
//!
//! # Telemetry
//!
//! Each export call (`type-uri`, `start`, `send`, `wake`, `cancel`) records
//! one observation through scxml.rs's `telemetry` WIT facade: its closed
//! operation, its closed outcome (`ok`, `rejected`, `failed`), and one closed
//! signal event. No attribute carries the prompt, the reply, the schema, the
//! model, or a URL, so the cardinality is the product of the three closed
//! enums. The host adds its own `service-*` and `service-http-*` spans with
//! only their closed outcomes.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

use scxml_event_schema::{CompiledSchema, canonical_json};
use scxml_types::{DataValue as Data, FiniteNumber, exceeds_wasm_bound, wasm_bound};
use serde_json::{Map, Value as Json};

/// The service's name: the last path segment of its repository service URI
/// before the tag.
pub const SERVICE_NAME: &str = "generate";

/// The service's repository URI without its `@<tag>` suffix.
pub const SERVICE_URI_BASE: &str = "https://github.com/stateforward/nemobot/services/generate";

/// The release tag the service URI names. nemobot has no release yet: this
/// is the placeholder of its first release (the workspace version), and the
/// manifest's `service-uri` moves with it before a tag is cut.
pub const SERVICE_TAG: &str = "v0.1.0";

/// The exact service type URI documents name: `{SERVICE_URI_BASE}@{SERVICE_TAG}`.
pub const SERVICE_URI: &str = "https://github.com/stateforward/nemobot/services/generate@v0.1.0";

/// The `env` slot of the API key (`env:apikey`).
pub const API_KEY_SLOT: &str = "apikey";
/// The `env` slot of the API base URL (`env:baseurl`).
pub const BASE_URL_SLOT: &str = "baseurl";
/// The path appended to the base URL.
pub const CHAT_COMPLETIONS_PATH: &str = "/chat/completions";
/// `json_schema.name` when the invocation names none.
pub const DEFAULT_SCHEMA_NAME: &str = "result";
/// Upper bound on the bytes of a schema name (the `OpenAI` limit).
pub const MAX_SCHEMA_NAME_BYTES: usize = 64;

/// The event that carries the reply.
pub const RESULT_EVENT: &str = "generate.result";
/// The event that reports a failed generation.
pub const FAILED_EVENT: &str = "generate.failed";

/// Upper bound on the conversation's bytes, the system message included
/// (wasm builds only). Its JSON encoding stays inside the host's request-body
/// bound.
pub const MAX_CONVERSATION_BYTES: usize = wasm_bound(512 * 1024);
/// Upper bound on the number of messages (wasm builds only).
pub const MAX_MESSAGES: usize = wasm_bound(256);
/// Upper bound on the model name's bytes (wasm builds only).
pub const MAX_MODEL_BYTES: usize = wasm_bound(256);

/// One part of a URL or header value: literal text, or an opaque reference
/// the host substitutes with the value it holds for one slot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Part {
    /// Literal text.
    Literal(String),
    /// A slot's reference.
    Reference(u32),
}

/// One `POST` request to the model service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    /// The absolute target URL, in parts.
    pub url: Vec<Part>,
    /// Header fields, in order, their values in parts.
    pub headers: Vec<(String, Vec<Part>)>,
    /// The canonical JSON body.
    pub body: Vec<u8>,
}

/// The model service's answer.
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
    /// No response: refused, unreachable, timed out, or failed.
    Unavailable,
    /// The response body exceeds the host's bound.
    TooLarge,
}

/// A chat message role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    /// `system`.
    System,
    /// `user`.
    User,
    /// `assistant`.
    Assistant,
}

impl Role {
    /// The wire name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "system" => Some(Self::System),
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            _ => None,
        }
    }
}

/// One chat message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    /// Its role.
    pub role: Role,
    /// Its text.
    pub content: String,
}

/// The structured-output request of one invocation.
#[derive(Debug)]
pub struct ResponseSchema {
    /// `json_schema.name`.
    pub name: String,
    /// `json_schema.strict`.
    pub strict: bool,
    /// The canonical resolved schema.
    pub schema: Json,
    /// The schema compiled for checking the reply.
    pub compiled: CompiledSchema,
}

/// One validated generation request: what an invocation's parameters ask.
#[derive(Debug)]
pub struct Generation {
    /// The model name.
    pub model: String,
    /// The conversation, the system message first.
    pub messages: Vec<Message>,
    /// The structured-output request, when the invocation gave a schema.
    pub schema: Option<ResponseSchema>,
}

/// The closed failure codes of [`FAILED_EVENT`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureCode {
    /// The `apikey` or `baseurl` slot names no variable the run holds.
    Unconfigured,
    /// The host refused the request before sending it (an origin the
    /// package does not declare, no transport, or a run that delivers no
    /// deferred outcome).
    Refused,
    /// The transport failed: no response.
    Transport,
    /// The model service answered with a non-2xx status.
    Status,
    /// The answer is not a chat completion with text content, or exceeds
    /// the host's response bound.
    Malformed,
    /// The reply was cut short by its token limit.
    Truncated,
    /// A content filter withheld the reply.
    Filtered,
    /// The model refused the request.
    Refusal,
    /// The structured reply is not JSON.
    NotJson,
    /// The structured reply does not satisfy the requested schema.
    SchemaMismatch,
}

impl FailureCode {
    /// The wire name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unconfigured => "unconfigured",
            Self::Refused => "refused",
            Self::Transport => "transport",
            Self::Status => "status",
            Self::Malformed => "malformed",
            Self::Truncated => "truncated",
            Self::Filtered => "filtered",
            Self::Refusal => "refusal",
            Self::NotJson => "not-json",
            Self::SchemaMismatch => "schema-mismatch",
        }
    }
}

/// The one event an invocation emits before it completes: its name and its
/// data as a canonical JSON object.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Emitted {
    /// [`RESULT_EVENT`] or [`FAILED_EVENT`].
    pub name: &'static str,
    /// Canonical JSON object text.
    pub data: String,
}

impl Emitted {
    /// [`FAILED_EVENT`] with `code`.
    #[must_use]
    pub fn failed(code: FailureCode) -> Self {
        Self {
            name: FAILED_EVENT,
            data: canonical_json(&serde_json::json!({ "code": code.name() })),
        }
    }
}

fn text_parameter(value: &Data, invalid: &'static str) -> Result<String, &'static str> {
    match value {
        Data::String(text) if !text.trim().is_empty() => Ok(text.clone()),
        _ => Err(invalid),
    }
}

/// The messages of a JSON array: objects with exactly `role` and `content`.
///
/// # Errors
///
/// A static detail for anything else, or an empty array.
pub fn messages_from_json(text: &str) -> Result<Vec<Message>, &'static str> {
    const INVALID: &str = "generate messages must be objects with exactly a role (system, user, assistant) and a string content";
    let Ok(Json::Array(items)) = serde_json::from_str::<Json>(text) else {
        return Err("generate messages must be a JSON array of messages");
    };
    if items.is_empty() {
        return Err("generate messages must hold at least one message");
    }
    items
        .iter()
        .map(|item| {
            let Json::Object(members) = item else {
                return Err(INVALID);
            };
            if members.len() != 2 {
                return Err(INVALID);
            }
            let role = members
                .get("role")
                .and_then(Json::as_str)
                .and_then(Role::parse)
                .ok_or(INVALID)?;
            let content = members
                .get("content")
                .and_then(Json::as_str)
                .ok_or(INVALID)?;
            Ok(Message {
                role,
                content: content.to_owned(),
            })
        })
        .collect()
}

/// Resolves and compiles one response schema from its source text.
///
/// # Errors
///
/// A static detail when the text is not a JSON Schema its dialect's
/// metaschema admits, references anything outside itself, or exceeds the
/// schema bound.
pub fn response_schema(
    source: &str,
    name: Option<&str>,
    strict: bool,
) -> Result<ResponseSchema, &'static str> {
    const INVALID: &str = "generate schema is not a self-contained JSON Schema";
    let resolved =
        scxml_event_schema::resolve(source.trim().as_bytes(), None).map_err(|_| INVALID)?;
    let schema: Json = serde_json::from_str(&resolved.schema).map_err(|_| INVALID)?;
    let compiled = CompiledSchema::new(&resolved.schema).map_err(|_| INVALID)?;
    Ok(ResponseSchema {
        name: name.unwrap_or(DEFAULT_SCHEMA_NAME).to_owned(),
        strict,
        schema,
        compiled,
    })
}

/// The validated generation request of an invocation's parameters (see the
/// crate documentation, "Parameters").
///
/// # Errors
///
/// A static detail for an unknown or repeated parameter, a missing or
/// malformed one, or a conversation or model outside its bound.
pub fn parse_parameters<'a>(
    parameters: impl IntoIterator<Item = (&'a str, &'a Data)>,
) -> Result<Generation, &'static str> {
    let mut model = None;
    let mut prompt = None;
    let mut messages = None;
    let mut system = None;
    let mut schema = None;
    let mut schema_name = None;
    let mut strict = None;
    for (name, value) in parameters {
        let slot = match name {
            "model" => &mut model,
            "prompt" => &mut prompt,
            "messages" => &mut messages,
            "system" => &mut system,
            "schema" => &mut schema,
            "schemaname" => &mut schema_name,
            "strict" => &mut strict,
            _ => return Err("generate takes no parameter of that name"),
        };
        if slot.replace(value).is_some() {
            return Err("generate takes each parameter once");
        }
    }
    let model = text_parameter(
        model.ok_or("generate requires a model parameter")?,
        "generate model must be a non-empty string",
    )?
    .trim()
    .to_owned();
    if exceeds_wasm_bound(model.len(), MAX_MODEL_BYTES) {
        return Err("generate model exceeds its bound");
    }
    let mut conversation = Vec::new();
    if let Some(system) = system {
        conversation.push(Message {
            role: Role::System,
            content: text_parameter(system, "generate system must be a non-empty string")?,
        });
    }
    match (prompt, messages) {
        (Some(prompt), None) => conversation.push(Message {
            role: Role::User,
            content: text_parameter(prompt, "generate prompt must be a non-empty string")?,
        }),
        (None, Some(Data::JsonArray(text))) => conversation.extend(messages_from_json(text)?),
        (None, Some(_)) => return Err("generate messages must be a JSON array of messages"),
        _ => return Err("generate takes exactly one of prompt or messages"),
    }
    let bytes = conversation.iter().fold(0_usize, |total, message| {
        total.saturating_add(message.content.len())
    });
    if exceeds_wasm_bound(conversation.len(), MAX_MESSAGES)
        || exceeds_wasm_bound(bytes, MAX_CONVERSATION_BYTES)
    {
        return Err("generate conversation exceeds its bound");
    }
    let schema_name = match schema_name {
        None => None,
        Some(Data::String(name))
            if !name.is_empty()
                && name.len() <= MAX_SCHEMA_NAME_BYTES
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-') =>
        {
            Some(name.as_str())
        }
        Some(_) => return Err("generate schemaname must match [A-Za-z0-9_-]{1,64}"),
    };
    let strict = match strict {
        None => None,
        Some(Data::Bool(strict)) => Some(*strict),
        Some(_) => return Err("generate strict must be a boolean"),
    };
    let schema = match schema {
        None => {
            if schema_name.is_some() || strict.is_some() {
                return Err("generate schemaname and strict require a schema");
            }
            None
        }
        Some(Data::JsonObject(text) | Data::String(text)) => {
            Some(response_schema(text, schema_name, strict.unwrap_or(false))?)
        }
        Some(_) => return Err("generate schema must be a JSON object or a string holding one"),
    };
    Ok(Generation {
        model,
        messages: conversation,
        schema,
    })
}

/// The Chat Completions request: `POST {base_url}/chat/completions` (the
/// `baseurl` slot's reference) with the `apikey` slot's reference as a bearer
/// key and a canonical JSON body: `messages`, `model`, and, with a schema,
/// `response_format`.
#[must_use]
pub fn build_request(base_url: u32, api_key: u32, generation: &Generation) -> Request {
    let mut body = Map::new();
    body.insert(
        "messages".into(),
        generation
            .messages
            .iter()
            .map(|message| {
                serde_json::json!({ "role": message.role.name(), "content": message.content })
            })
            .collect(),
    );
    body.insert("model".into(), generation.model.clone().into());
    if let Some(schema) = &generation.schema {
        body.insert(
            "response_format".into(),
            serde_json::json!({
                "type": "json_schema",
                "json_schema": {
                    "name": schema.name,
                    "strict": schema.strict,
                    "schema": schema.schema,
                },
            }),
        );
    }
    Request {
        url: vec![
            Part::Reference(base_url),
            Part::Literal(CHAT_COMPLETIONS_PATH.into()),
        ],
        headers: vec![
            (
                "authorization".into(),
                vec![Part::Literal("Bearer ".into()), Part::Reference(api_key)],
            ),
            (
                "content-type".into(),
                vec![Part::Literal("application/json".into())],
            ),
            (
                "accept".into(),
                vec![Part::Literal("application/json".into())],
            ),
        ],
        body: canonical_json(&Json::Object(body)).into_bytes(),
    }
}

/// The reply text of a Chat Completions result: `choices[0].message.content`.
///
/// # Errors
///
/// The failure code when the body is not such a result
/// ([`FailureCode::Malformed`]), the first choice was cut short
/// ([`FailureCode::Truncated`]) or filtered ([`FailureCode::Filtered`]), or
/// the message carries a refusal ([`FailureCode::Refusal`]).
pub fn reply_text(body: &[u8]) -> Result<String, FailureCode> {
    let value: Json = serde_json::from_slice(body).map_err(|_| FailureCode::Malformed)?;
    let choice = value
        .get("choices")
        .and_then(Json::as_array)
        .and_then(|choices| choices.first())
        .ok_or(FailureCode::Malformed)?;
    match choice.get("finish_reason").and_then(Json::as_str) {
        Some("length") => return Err(FailureCode::Truncated),
        Some("content_filter") => return Err(FailureCode::Filtered),
        _ => {}
    }
    let message = choice.get("message").ok_or(FailureCode::Malformed)?;
    if message
        .get("refusal")
        .and_then(Json::as_str)
        .is_some_and(|refusal| !refusal.is_empty())
    {
        return Err(FailureCode::Refusal);
    }
    message
        .get("content")
        .and_then(Json::as_str)
        .map(str::to_owned)
        .ok_or(FailureCode::Malformed)
}

/// The structured reply: `text` parsed as JSON, admitted by `schema`.
///
/// # Errors
///
/// [`FailureCode::NotJson`] when `text` is not JSON (or holds a number
/// outside the data model's range), [`FailureCode::SchemaMismatch`] when the
/// value does not satisfy the schema.
pub fn structured_value(text: &str, schema: &CompiledSchema) -> Result<Json, FailureCode> {
    let value: Json = serde_json::from_str(text).map_err(|_| FailureCode::NotJson)?;
    if !schema.admits(&json_data(&value)?) {
        return Err(FailureCode::SchemaMismatch);
    }
    Ok(value)
}

/// The data value of one JSON value: objects and arrays as canonical JSON
/// text, scalars as themselves.
fn json_data(value: &Json) -> Result<Data, FailureCode> {
    Ok(match value {
        Json::Null => Data::Null,
        Json::Bool(value) => Data::Bool(*value),
        Json::Number(number) => Data::Number(
            number
                .as_f64()
                .and_then(|number| FiniteNumber::new(number).ok())
                .ok_or(FailureCode::NotJson)?,
        ),
        Json::String(text) => Data::String(text.clone()),
        Json::Array(_) => Data::JsonArray(canonical_json(value)),
        Json::Object(_) => Data::JsonObject(canonical_json(value)),
    })
}

/// What one settled request becomes: [`RESULT_EVENT`] with the reply
/// (`{"text": T}`, or `{"json": V}` with a schema, validated first), or
/// [`FAILED_EVENT`] with the failure's code.
#[must_use]
pub fn outcome(generation: &Generation, answered: Result<Response, TransportFailure>) -> Emitted {
    let response = match answered {
        Ok(response) => response,
        Err(TransportFailure::Unavailable) => return Emitted::failed(FailureCode::Transport),
        Err(TransportFailure::TooLarge) => return Emitted::failed(FailureCode::Malformed),
    };
    if !(200..300).contains(&response.status) {
        return Emitted::failed(FailureCode::Status);
    }
    let text = match reply_text(&response.body) {
        Ok(text) => text,
        Err(code) => return Emitted::failed(code),
    };
    let data = match &generation.schema {
        None => serde_json::json!({ "text": text }),
        Some(schema) => match structured_value(&text, &schema.compiled) {
            Ok(value) => serde_json::json!({ "json": value }),
            Err(code) => return Emitted::failed(code),
        },
    };
    Emitted {
        name: RESULT_EVENT,
        data: canonical_json(&data),
    }
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
    });

    use std::cell::RefCell;

    use exports::stateforward::scxml::invoked_service_api::Guest;
    use stateforward::scxml::invoked_service_host;
    use stateforward::scxml::outbound_http::{self, BodyPart, HttpHeader, HttpRequest, TextPart};
    use stateforward::scxml::telemetry::{self, Operation, Outcome, SignalEvent};
    use stateforward::scxml::types::{
        ComponentError, DataValue, ErrorCode, ErrorVersion, Event, EventType, ServiceInvocation,
        SlotReference,
    };

    use super::{Emitted, FailureCode, Generation, Part, Response, TransportFailure};

    /// One invocation's generation and the ticket of its request in flight.
    struct Pending {
        generation: Generation,
        ticket: u64,
    }

    thread_local! {
        // This instance's one invocation. The Component Model instance is
        // its single owner (the host instantiates one per invocation, on one
        // thread); each export borrows it for that call only, and the
        // imports it calls cannot re-enter the component.
        static PENDING: RefCell<Option<Pending>> = const { RefCell::new(None) };
    }

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

    /// Emits the invocation's one event, then completes.
    fn finish(emitted: &Emitted) -> Result<(), ComponentError> {
        invoked_service_host::emit(&Event {
            name: emitted.name.to_owned(),
            event_type: EventType::External,
            send_id: None,
            origin: None,
            origin_type: None,
            invoke_id: None,
            data: DataValue::JsonObject(emitted.data.clone()),
            raw: None,
            trace_context: None,
        })?;
        invoked_service_host::complete(None)
    }

    fn slot(references: &[SlotReference], name: &str) -> Option<u32> {
        references
            .iter()
            .find(|bound| bound.slot == name)
            .map(|bound| bound.reference)
    }

    impl Guest for Component {
        fn type_uri() -> String {
            telemetry::event(
                Operation::TypeUri,
                SignalEvent::TransformationCompleted,
                None,
            );
            super::SERVICE_URI.to_owned()
        }

        fn start(
            request: ServiceInvocation,
            references: Vec<SlotReference>,
        ) -> Result<(), ComponentError> {
            observed(Operation::Start, || {
                if request.type_uri != super::SERVICE_URI {
                    return Err(error(ErrorCode::InvalidInput, "service type URI differs"));
                }
                let parameters = request
                    .parameters
                    .into_iter()
                    .map(|entry| Ok((entry.name, native_value(entry.value)?)))
                    .collect::<Result<Vec<_>, ComponentError>>()?;
                let generation = super::parse_parameters(
                    parameters
                        .iter()
                        .map(|(name, value)| (name.as_str(), value)),
                )
                .map_err(|detail| error(ErrorCode::InvalidInput, detail))?;
                let (Some(api_key), Some(base_url)) = (
                    slot(&references, super::API_KEY_SLOT),
                    slot(&references, super::BASE_URL_SLOT),
                ) else {
                    return finish(&Emitted::failed(FailureCode::Unconfigured));
                };
                let built = super::build_request(base_url, api_key, &generation);
                let ticket = outbound_http::begin(&HttpRequest {
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
                });
                match ticket {
                    Ok(ticket) => {
                        PENDING.set(Some(Pending { generation, ticket }));
                        Ok(())
                    }
                    Err(_) => finish(&Emitted::failed(FailureCode::Refused)),
                }
            })
        }

        fn send(_event: Event) -> Result<(), ComponentError> {
            observed(Operation::Send, || {
                Err(error(
                    ErrorCode::InvalidInput,
                    "the generate service accepts no event",
                ))
            })
        }

        fn wake() -> Result<(), ComponentError> {
            observed(Operation::Wake, || {
                let settled = PENDING.with_borrow(|pending| {
                    pending
                        .as_ref()
                        .and_then(|pending| outbound_http::take(pending.ticket))
                });
                // A wake with nothing settled asks for nothing.
                let Some(settled) = settled else {
                    return Ok(());
                };
                let Some(pending) = PENDING.take() else {
                    return Ok(());
                };
                let answered = settled
                    .map(|response| Response {
                        status: response.status,
                        body: response.body,
                    })
                    .map_err(|failure| match failure.code {
                        ErrorCode::PayloadTooLarge => TransportFailure::TooLarge,
                        _ => TransportFailure::Unavailable,
                    });
                finish(&super::outcome(&pending.generation, answered))
            })
        }

        fn cancel() -> Result<(), ComponentError> {
            observed(Operation::Cancel, || {
                PENDING.set(None);
                Ok(())
            })
        }
    }

    /// One export call's closed telemetry: its outcome and one event (no
    /// attribute carries input).
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
