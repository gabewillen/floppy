//! The `generate` extension namespace: `<gen:generate>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! Provider-neutral model generation: one element asks a model for one reply
//! (text, or JSON that satisfies a JSON Schema) and assigns it to a data-model
//! location. The wire protocol is the `OpenAI`-compatible Chat Completions API
//! (`POST {base}/chat/completions`), which `OpenAI`, `NVIDIA`'s
//! `integrate.api.nvidia.com/v1`, and local servers all speak; the backend is
//! chosen by the run, through the `baseurl` slot, never by the document or the
//! component. Documents bind [`NAMESPACE_URI`] (conventionally with the prefix
//! `gen`).
//!
//! # `<gen:generate>`
//!
//! * `location="L"` (required): receives the reply: a string, or, with a
//!   response schema, the JSON value the model returned.
//! * `model="M"` or `modelexpr="E"` (exactly one; `modelexpr` must evaluate
//!   to a non-empty string): sent as `model`.
//! * the conversation, exactly one of:
//!   * one `<gen:content>` child: the prompt as text, leading and trailing
//!     white space removed, sent as one `user` message; or
//!   * `expr="E"`: an expression that evaluates to a string (one `user`
//!     message, used exactly) or to a JSON array of messages, each an object
//!     with exactly the members `role` (`system`, `user`, or `assistant`) and
//!     `content` (a string).
//! * `system="S"` or `systemexpr="E"` (optional, at most one; a string):
//!   sent as a `system` message before the conversation.
//! * the response schema (optional, at most one): one `<gen:schema>` child
//!   holding a JSON Schema as text, or `schemaexpr="E"` evaluating to one (a
//!   JSON object, or a string holding it). It must satisfy its dialect's
//!   metaschema (JSON Schema 2020-12 unless `$schema` names another
//!   supported dialect) and reference nothing outside itself. With it the
//!   request asks for structured output (`response_format` of type
//!   `json_schema`), and the reply must parse as JSON and satisfy the schema
//!   **before** anything is assigned.
//!   * `schemaname="N"` (optional, [`DEFAULT_SCHEMA_NAME`] without it;
//!     `[A-Za-z0-9_-]{1,64}`) and `strict="true|false"` (optional, `false`
//!     without it): sent as `json_schema.name` and `json_schema.strict`.
//!     Both require a schema.
//! * the host's `env` slots (`https://github.com/stateforward/scxml.rs/namespaces/env`):
//!   `env:apikey="NAME"` (required at run time; sent as a bearer token) and
//!   `env:baseurl="NAME"` (required at run time: the API base URL, for
//!   example `https://api.openai.com/v1`; must not end with `/`), or their
//!   `expr` forms. No backend URL is compiled in: an element whose
//!   `baseurl` slot holds no value fails like one with no key. The component holds opaque
//!   references only; the host substitutes the values into the one admitted
//!   request.
//!
//! Every other attribute (unqualified, or in a namespace other than `env`),
//! child element, or non-white-space text is refused.
//!
//! # The request
//!
//! `POST {base}/chat/completions` with `authorization: Bearer <apikey>`,
//! `content-type: application/json`, `accept: application/json`, and a body
//! that is canonical JSON ([`scxml_event_schema::canonical_json`]: compact,
//! members sorted by key), so one element and one data-model state always
//! produce the same bytes ([`build_request`]):
//!
//! ```json
//! {"messages":[{"content":"…","role":"system"},{"content":"…","role":"user"}],
//!  "model":"…",
//!  "response_format":{"json_schema":{"name":"result","schema":{…},"strict":false},"type":"json_schema"}}
//! ```
//!
//! (`response_format` only with a schema.)
//!
//! # The reply
//!
//! The reply is `choices[0].message.content`, which must be a string; a
//! `refusal`, a `finish_reason` of `length` or `content_filter`, or a body
//! without that shape is a failure ([`reply_text`]). Without a schema the
//! text is assigned as a string. With one it is parsed as JSON and checked
//! against the compiled schema; a reply that is not JSON or does not satisfy
//! the schema is a failure and nothing is assigned ([`structured_value`]).
//!
//! # Failures
//!
//! Every malformed element, evaluation, missing key, refused or failed
//! request, non-2xx status (a redirect is never followed), malformed reply,
//! schema mismatch, or assignment failure is an `execution` error with a
//! static detail (SCXML `error.execution`); the session keeps running. The
//! prompt, the reply, the schema, the model name, and the key are never
//! echoed.
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
//! Each entry point (`initialize`, `execute`, `shutdown`) records one
//! observation through scxml.rs's `telemetry` WIT facade: its closed
//! operation, its closed outcome (`ok`, `rejected`, `failed`), and one closed
//! signal event. No attribute carries the prompt, the reply, the schema, the
//! model, a location, or a URL, so the cardinality is the product of the
//! three closed enums. The host adds one `extension-http-fetch` span per
//! request with only its closed outcome.

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

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "generate";

/// The namespace's repository namespace URI without its `@<tag>` suffix.
pub const NAMESPACE_URI_BASE: &str = "https://github.com/stateforward/nemobot/namespaces/generate";

/// The release tag the namespace URI names. nemobot has no release yet: this
/// is the placeholder of its first release (the workspace version), and the
/// manifest's `namespace-uri` and the schema's `targetNamespace` move with it
/// before a tag is cut.
pub const NAMESPACE_TAG: &str = "v0.1.0";

/// The exact namespace URI documents bind: `{NAMESPACE_URI_BASE}@{NAMESPACE_TAG}`.
pub const NAMESPACE_URI: &str =
    "https://github.com/stateforward/nemobot/namespaces/generate@v0.1.0";

/// The `env` slot of the API key (`env:apikey`).
pub const API_KEY_SLOT: &str = "apikey";
/// The `env` slot of the API base URL (`env:baseurl`).
pub const BASE_URL_SLOT: &str = "baseurl";
/// The path appended to the base URL.
pub const CHAT_COMPLETIONS_PATH: &str = "/chat/completions";
/// `json_schema.name` when the element names none.
pub const DEFAULT_SCHEMA_NAME: &str = "result";
/// Upper bound on the bytes of a schema name (the `OpenAI` limit).
pub const MAX_SCHEMA_NAME_BYTES: usize = 64;

/// Upper bound on the conversation's bytes, the system message included
/// (wasm builds only). Its JSON encoding stays inside the host's request-body
/// bound.
pub const MAX_CONVERSATION_BYTES: usize = wasm_bound(512 * 1024);
/// Upper bound on the number of messages (wasm builds only).
pub const MAX_MESSAGES: usize = wasm_bound(256);
/// Upper bound on the model name's bytes (wasm builds only).
pub const MAX_MODEL_BYTES: usize = wasm_bound(256);

/// One attribute of an element.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attribute {
    /// Its namespace URI, when it has one (only unqualified attributes are
    /// element parameters; `env` attributes are the host's).
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
    Start {
        /// Its namespace URI, when it has one.
        namespace_uri: Option<String>,
        /// Its local name.
        local_name: String,
        /// Whether it carries any attribute.
        has_attributes: bool,
    },
    /// The end tag of the innermost open nested element.
    End,
}

/// A value the data model evaluated.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
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

/// The structured-output request of one element.
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
pub trait GenerateHost {
    /// A non-recoverable host failure.
    type Error;

    /// Evaluates one data-model expression; `Ok(Err(()))` is a recoverable
    /// evaluation failure.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn evaluate(&mut self, expression: &str) -> Result<Result<Value, ()>, Self::Error>;

    /// Assigns one value (a string, or a JSON value) to one data-model
    /// location; `Ok(Err(()))` is a recoverable assignment failure.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn assign(&mut self, location: &str, value: &Data) -> Result<Result<(), ()>, Self::Error>;

    /// The reference the host bound to one of the element's `env` slots,
    /// when the element names a variable for it that the run holds.
    fn slot(&mut self, slot: &str) -> Option<u32>;

    /// Performs one `POST`; `Ok(Err(()))` means the host refused it or the
    /// transport failed (no response).
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn post(&mut self, request: &Request) -> Result<Result<Response, ()>, Self::Error>;
}

/// Executes one element with the given local name, attributes, and
/// flattened content.
///
/// # Errors
///
/// [`Failure::Execution`] for every malformed element, evaluation,
/// configuration, transport, reply, schema, or assignment failure;
/// [`Failure::Host`] for a non-recoverable host failure.
pub fn execute<H: GenerateHost>(
    local_name: &str,
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    match local_name {
        "generate" => generate(attributes, content, host).map(|()| Executed::Handled),
        _ => Ok(Executed::NotHandled),
    }
}

/// The parsed attributes of one `<gen:generate>`.
#[derive(Debug, Default)]
struct Parameters<'a> {
    location: &'a str,
    model: Option<&'a str>,
    model_expr: Option<&'a str>,
    expr: Option<&'a str>,
    system: Option<&'a str>,
    system_expr: Option<&'a str>,
    schema_expr: Option<&'a str>,
    schema_name: Option<&'a str>,
    strict: Option<bool>,
}

/// The parsed children of one `<gen:generate>`.
#[derive(Debug, Default)]
struct Children {
    content: Option<String>,
    schema: Option<String>,
}

fn parameters(attributes: &[Attribute]) -> Result<Parameters<'_>, &'static str> {
    let mut location = None;
    let mut parameters = Parameters::default();
    for attribute in attributes {
        match attribute.namespace_uri.as_deref() {
            // The host's slot vocabulary: the validator checks it against
            // the manifest and the host binds it; it is no parameter here.
            Some(scxml_types::ENV_NAMESPACE_URI) => continue,
            Some(_) => {
                return Err("gen:generate takes no attributes in another namespace than env");
            }
            None => {}
        }
        let value = attribute.value.as_str();
        let slot = match attribute.local_name.as_str() {
            "location" => &mut location,
            "model" => &mut parameters.model,
            "modelexpr" => &mut parameters.model_expr,
            "expr" => &mut parameters.expr,
            "system" => &mut parameters.system,
            "systemexpr" => &mut parameters.system_expr,
            "schemaexpr" => &mut parameters.schema_expr,
            "schemaname" => &mut parameters.schema_name,
            "strict" => {
                parameters.strict = Some(match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err("gen:generate strict must be true or false"),
                });
                continue;
            }
            _ => {
                return Err("gen:generate has an attribute this package version does not define");
            }
        };
        *slot = Some(value);
    }
    parameters.location = location
        .map(str::trim)
        .filter(|location| !location.is_empty())
        .ok_or("gen:generate requires a non-empty location")?;
    if parameters.system.is_some() && parameters.system_expr.is_some() {
        return Err("gen:generate takes at most one of system or systemexpr");
    }
    if let Some(name) = parameters.schema_name
        && (name.is_empty()
            || name.len() > MAX_SCHEMA_NAME_BYTES
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'))
    {
        return Err("gen:generate schemaname must match [A-Za-z0-9_-]{1,64}");
    }
    Ok(parameters)
}

fn children(content: &[Content]) -> Result<Children, &'static str> {
    const ONLY: &str = "gen:generate holds only gen:content and gen:schema children";
    let mut children = Children::default();
    let mut items = content.iter();
    while let Some(item) = items.next() {
        let (namespace_uri, local_name, has_attributes) = match item {
            Content::Text(text) if text.trim().is_empty() => continue,
            Content::Text(_) => return Err(ONLY),
            Content::End => return Err("gen:generate content is unbalanced"),
            Content::Start {
                namespace_uri,
                local_name,
                has_attributes,
            } => (namespace_uri, local_name, *has_attributes),
        };
        if namespace_uri.as_deref() != Some(NAMESPACE_URI) {
            return Err(ONLY);
        }
        if has_attributes {
            return Err("gen:content and gen:schema take no attributes");
        }
        let mut text = String::new();
        loop {
            match items.next() {
                Some(Content::Text(part)) => text.push_str(part),
                Some(Content::End) => break,
                Some(Content::Start { .. }) => {
                    return Err("gen:content and gen:schema hold text only, not elements");
                }
                None => return Err("gen:generate content is unbalanced"),
            }
        }
        let slot = match local_name.as_str() {
            "content" => &mut children.content,
            "schema" => &mut children.schema,
            _ => return Err(ONLY),
        };
        if slot.is_some() {
            return Err("gen:generate holds at most one gen:content and one gen:schema");
        }
        *slot = Some(text);
    }
    Ok(children)
}

fn evaluate<H: GenerateHost>(
    host: &mut H,
    expression: &str,
    failed: &'static str,
) -> Result<Value, Failure<H::Error>> {
    host.evaluate(expression)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(failed))
}

fn model<H: GenerateHost>(
    parameters: &Parameters<'_>,
    host: &mut H,
) -> Result<String, Failure<H::Error>> {
    let model = match (parameters.model, parameters.model_expr) {
        (Some(model), None) => model.trim().to_owned(),
        (None, Some(expression)) => {
            match evaluate(
                host,
                expression,
                "gen:generate modelexpr could not be evaluated",
            )? {
                Value::Text(text) => text.trim().to_owned(),
                _ => {
                    return Err(Failure::Execution(
                        "gen:generate modelexpr must evaluate to a string",
                    ));
                }
            }
        }
        _ => {
            return Err(Failure::Execution(
                "gen:generate takes exactly one of model or modelexpr",
            ));
        }
    };
    if model.is_empty() || exceeds_wasm_bound(model.len(), MAX_MODEL_BYTES) {
        return Err(Failure::Execution(
            "gen:generate model must be non-empty and within the model bound",
        ));
    }
    Ok(model)
}

/// The messages of a JSON array: objects with exactly `role` and `content`.
fn messages_from_json(text: &str) -> Result<Vec<Message>, &'static str> {
    const INVALID: &str = "gen:generate expr messages must be objects with exactly a role (system, user, assistant) and a string content";
    let Ok(Json::Array(items)) = serde_json::from_str::<Json>(text) else {
        return Err("gen:generate expr must evaluate to a string or an array of messages");
    };
    if items.is_empty() {
        return Err("gen:generate expr must evaluate to at least one message");
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

fn conversation<H: GenerateHost>(
    parameters: &Parameters<'_>,
    content: Option<String>,
    host: &mut H,
) -> Result<Vec<Message>, Failure<H::Error>> {
    let mut messages = Vec::new();
    let system = match (parameters.system, parameters.system_expr) {
        (Some(system), _) => Some(system.to_owned()),
        (None, Some(expression)) => {
            match evaluate(
                host,
                expression,
                "gen:generate systemexpr could not be evaluated",
            )? {
                Value::Text(text) => Some(text),
                _ => {
                    return Err(Failure::Execution(
                        "gen:generate systemexpr must evaluate to a string",
                    ));
                }
            }
        }
        (None, None) => None,
    };
    if let Some(system) = system {
        if system.trim().is_empty() {
            return Err(Failure::Execution("gen:generate system must not be empty"));
        }
        messages.push(Message {
            role: Role::System,
            content: system,
        });
    }
    match (parameters.expr, content) {
        (None, Some(text)) => messages.push(Message {
            role: Role::User,
            content: text.trim().to_owned(),
        }),
        (Some(expression), None) => {
            match evaluate(host, expression, "gen:generate expr could not be evaluated")? {
                Value::Text(text) => messages.push(Message {
                    role: Role::User,
                    content: text,
                }),
                Value::Json(text) => {
                    messages.extend(messages_from_json(&text).map_err(Failure::Execution)?);
                }
                Value::Other => {
                    return Err(Failure::Execution(
                        "gen:generate expr must evaluate to a string or an array of messages",
                    ));
                }
            }
        }
        _ => {
            return Err(Failure::Execution(
                "gen:generate takes exactly one of expr or gen:content",
            ));
        }
    }
    if messages
        .last()
        .is_some_and(|message| message.role == Role::User && message.content.trim().is_empty())
    {
        return Err(Failure::Execution("gen:generate prompt must not be empty"));
    }
    let bytes = messages.iter().fold(0_usize, |total, message| {
        total.saturating_add(message.content.len())
    });
    if exceeds_wasm_bound(messages.len(), MAX_MESSAGES)
        || exceeds_wasm_bound(bytes, MAX_CONVERSATION_BYTES)
    {
        return Err(Failure::Execution(
            "gen:generate conversation exceeds its bound",
        ));
    }
    Ok(messages)
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
    const INVALID: &str = "gen:generate schema is not a self-contained JSON Schema";
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

fn schema<H: GenerateHost>(
    parameters: &Parameters<'_>,
    child: Option<String>,
    host: &mut H,
) -> Result<Option<ResponseSchema>, Failure<H::Error>> {
    let source = match (parameters.schema_expr, child) {
        (None, None) => {
            if parameters.schema_name.is_some() || parameters.strict.is_some() {
                return Err(Failure::Execution(
                    "gen:generate schemaname and strict require a response schema",
                ));
            }
            return Ok(None);
        }
        (None, Some(text)) => text,
        (Some(expression), None) => {
            match evaluate(
                host,
                expression,
                "gen:generate schemaexpr could not be evaluated",
            )? {
                Value::Text(text) | Value::Json(text) => text,
                Value::Other => {
                    return Err(Failure::Execution(
                        "gen:generate schemaexpr must evaluate to a JSON object or a string",
                    ));
                }
            }
        }
        (Some(_), Some(_)) => {
            return Err(Failure::Execution(
                "gen:generate takes at most one of schemaexpr or gen:schema",
            ));
        }
    };
    response_schema(
        &source,
        parameters.schema_name,
        parameters.strict.unwrap_or(false),
    )
    .map(Some)
    .map_err(Failure::Execution)
}

fn generate<H: GenerateHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<(), Failure<H::Error>> {
    let parameters = parameters(attributes).map_err(Failure::Execution)?;
    let children = children(content).map_err(Failure::Execution)?;
    let model = model(&parameters, host)?;
    let messages = conversation(&parameters, children.content, host)?;
    let schema = schema(&parameters, children.schema, host)?;
    let api_key = host.slot(API_KEY_SLOT).ok_or(Failure::Execution(
        "gen:generate has no API key: env:apikey names no variable the run holds",
    ))?;
    let base_url = host.slot(BASE_URL_SLOT).ok_or(Failure::Execution(
        "gen:generate has no base URL: env:baseurl names no variable the run holds",
    ))?;
    let request = build_request(base_url, api_key, &model, &messages, schema.as_ref());
    let response = host
        .post(&request)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution("the model request was refused or failed"))?;
    if !(200..300).contains(&response.status) {
        return Err(Failure::Execution(
            "the model service answered with a non-success status",
        ));
    }
    let text = reply_text(&response.body).map_err(Failure::Execution)?;
    let value = match &schema {
        None => Data::String(text),
        Some(schema) => structured_value(&text, &schema.compiled).map_err(Failure::Execution)?,
    };
    host.assign(parameters.location, &value)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution("gen:generate could not assign the reply to location"))
}

/// The Chat Completions request: `POST {base_url}/chat/completions` (the
/// `baseurl` slot's reference) with the `apikey`
/// slot's reference as a bearer key and a canonical JSON body: `messages`,
/// `model`, and, with a schema, `response_format`.
#[must_use]
pub fn build_request(
    base_url: u32,
    api_key: u32,
    model: &str,
    messages: &[Message],
    schema: Option<&ResponseSchema>,
) -> Request {
    let mut body = Map::new();
    body.insert(
        "messages".into(),
        messages
            .iter()
            .map(|message| {
                serde_json::json!({ "role": message.role.name(), "content": message.content })
            })
            .collect(),
    );
    body.insert("model".into(), model.into());
    if let Some(schema) = schema {
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
/// A static detail when the body is not such a result, the first choice
/// was cut short (`finish_reason` `length`) or filtered (`content_filter`),
/// the message carries a refusal, or its content is not a string.
pub fn reply_text(body: &[u8]) -> Result<String, &'static str> {
    const MALFORMED: &str = "the model response is not a valid chat completion";
    let value: Json = serde_json::from_slice(body).map_err(|_| MALFORMED)?;
    let choice = value
        .get("choices")
        .and_then(Json::as_array)
        .and_then(|choices| choices.first())
        .ok_or(MALFORMED)?;
    match choice.get("finish_reason").and_then(Json::as_str) {
        Some("length") => return Err("the model reply was cut short by its token limit"),
        Some("content_filter") => return Err("the model reply was withheld by a content filter"),
        _ => {}
    }
    let message = choice.get("message").ok_or(MALFORMED)?;
    if message
        .get("refusal")
        .and_then(Json::as_str)
        .is_some_and(|refusal| !refusal.is_empty())
    {
        return Err("the model refused the request");
    }
    message
        .get("content")
        .and_then(Json::as_str)
        .map(str::to_owned)
        .ok_or("the model reply carries no text content")
}

/// The data value of one JSON value: objects and arrays as canonical JSON
/// text, scalars as themselves.
///
/// # Errors
///
/// A static detail for a number that is not finite as a 64-bit float.
pub fn json_data(value: &Json) -> Result<Data, &'static str> {
    Ok(match value {
        Json::Null => Data::Null,
        Json::Bool(value) => Data::Bool(*value),
        Json::Number(number) => Data::Number(
            number
                .as_f64()
                .and_then(|number| FiniteNumber::new(number).ok())
                .ok_or("the model output holds a number outside the data model's range")?,
        ),
        Json::String(text) => Data::String(text.clone()),
        Json::Array(_) => Data::JsonArray(canonical_json(value)),
        Json::Object(_) => Data::JsonObject(canonical_json(value)),
    })
}

/// The structured reply: `text` parsed as JSON, admitted by `schema`.
///
/// # Errors
///
/// A static detail when `text` is not JSON or the value does not satisfy
/// the schema.
pub fn structured_value(text: &str, schema: &CompiledSchema) -> Result<Data, &'static str> {
    let value: Json = serde_json::from_str(text).map_err(|_| "the model output is not JSON")?;
    let data = json_data(&value)?;
    if !schema.admits(&data) {
        return Err("the model output does not satisfy the response schema");
    }
    Ok(data)
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
        ExtensionContent, ExtensionOutcome, SlotReference, TraceContext,
    };
    use stateforward::scxml::{data_model_host, outbound_http};

    use super::{
        Attribute, Content, Data, Executed, Failure, GenerateHost, Part, Request, Response, Value,
    };

    /// The lifecycle of this instance (`initialize`, `execute` calls,
    /// `shutdown`).
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

    fn text_parts(parts: &[Part]) -> Vec<outbound_http::TextPart> {
        parts
            .iter()
            .map(|part| match part {
                Part::Literal(text) => outbound_http::TextPart::Literal(text.clone()),
                Part::Reference(reference) => outbound_http::TextPart::Reference(*reference),
            })
            .collect()
    }

    /// One reply value as the WIT data value it is assigned as; the core
    /// never produces absent, undefined, or XML data.
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

    impl GenerateHost for Host {
        type Error = ComponentError;

        fn evaluate(&mut self, expression: &str) -> Result<Result<Value, ()>, ComponentError> {
            match data_model_host::evaluate(expression) {
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
                Err(error)
                    if matches!(
                        error.code,
                        ErrorCode::Unavailable
                            | ErrorCode::InvalidInput
                            | ErrorCode::PayloadTooLarge
                            | ErrorCode::Execution
                    ) =>
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
                ExtensionContent::StartElement(start) => Content::Start {
                    namespace_uri: start.name.namespace_uri,
                    local_name: start.name.local_name,
                    has_attributes: !start.attributes.is_empty(),
                },
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
