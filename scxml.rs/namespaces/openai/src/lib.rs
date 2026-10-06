//! The `openai` extension namespace: `<openai:generate>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! This is a rewrite, for the WIT extension component contract, modelled on
//! the `openai` namespace of agentml-go (`openai/namespace.go` at commit
//! 5d305e7 of <https://github.com/agentflare-ai/agentml.go>, MIT). It covers agentml-go's *content-generation
//! mode*: the element calls an OpenAI-compatible Responses API
//! (`POST {base}/responses`) and stores the model's output text at a data-model
//! location.
//!
//! | agentml-go                                 | this component                                        |
//! |--------------------------------------------|-------------------------------------------------------|
//! | `Namespace.Handle(ctx, el)`                | `executable-content-extension-api.execute`                       |
//! | `DataModel().EvaluateValue`                | `data-model-host.evaluate`                            |
//! | `DataModel().Assign`                       | `data-model-host.assign`                              |
//! | `os.Getenv("OPENAI_API_KEY")`, `..._BASE_URL` | the `apikey` and `baseurl` slots (opaque references) |
//! | `openai-go` client over `net/http`         | `outbound-http.fetch` (host-admitted, deny by default) |
//! | `interpreter.Snapshot(ctx, {ExcludeData})` | `session-snapshot-host.snapshot` (read-only)          |
//! | `interpreter.Send(ctx, ev)`                | `execution-host.send` (routed exactly as `<send>`)    |
//!
//! # `<openai:generate>`
//!
//! * `model="M"` or `modelexpr="E"` (required; `modelexpr` wins and must
//!   evaluate to a non-empty string);
//! * the prompt: the `prompt` attribute (literal text), then the string
//!   value of `promptexpr`, then the trimmed text of every
//!   `<openai:prompt>` child in document order; non-empty parts are joined
//!   with `\n`, exactly agentml-go's concatenation order. At least one part is
//!   required;
//! * `location="L"` (optional): the response text is assigned there as a
//!   string;
//! * `max-output-tokens="N"` (a positive integer) and `reasoning="R"` (the
//!   Responses API `reasoning.effort`, passed through) are optional.
//!
//! # The session snapshot and function calling
//!
//! As agentml-go does, every request carries the executing session's
//! read-only snapshot and the events the session can take:
//!
//! * the **system message** is the canonical `scxml.session-snapshot/v2`
//!   document taken with `datamodel` and `transitions` excluded (agentml-go's
//!   `SnapshotConfig{ExcludeData: true}` plus `PruneSnapshot`, which drops
//!   the transition listing because it becomes tools);
//! * the **tools** are agentml-go's send functions, derived from the
//!   `transitions` of a second document (only that section kept): one
//!   `send_<event>` function per event descriptor the listed transitions
//!   carry ([`send_functions`], [`sanitize_function_name`]), each with the
//!   optional string parameters `target` and `delay`. The snapshot carries
//!   no event data schemas (SCXML has none), so no tool declares `data`.
//!
//! Without `location` the element is in *function-calling mode*: the
//! request sets `tool_choice` to `required`, and the model must call at
//! least one function. With `location` it is `auto`. Every function call
//! the model returns is checked first ([`resolve_call`]: a name the tools
//! did not declare or malformed arguments is `error.execution`, and nothing
//! is sent); then each becomes one `execution-host.send`, routed exactly as
//! `<send>`, of the original event name. With `location`, the output text,
//! when the response has one, is assigned after the sends.
//!
//! When the host offers no snapshot (`unavailable`), the request carries
//! no system message and no tools, as agentml-go's does when its
//! `Snapshot` fails; function-calling mode then fails with
//! `error.execution`.
//!
//! The local name `generate` matches exactly, as in agentml-go; any other
//! local name is `not-handled` (the machine raises `error.execution`).
//!
//! # Configuration
//!
//! The element names its key and base URL by variable, never by value, with
//! the host's `env` attributes (`https://github.com/stateforward/scxml.rs/namespaces/env`):
//! `env:apikey="OPENAI_API_KEY"` (required, sent as a bearer token) and
//! `env:baseurl="OPENAI_BASE_URL"` (optional; [`DEFAULT_BASE_URL`] without it),
//! or `env:apikeyexpr`/`env:baseurlexpr` for an expression that evaluates to
//! the name. The component never sees either value: the host hands it an
//! opaque reference per slot ([`API_KEY_SLOT`], [`BASE_URL_SLOT`]) and
//! substitutes the value only inside the request. The request reaches the
//! network only through the host's `outbound-http` capability, which admits
//! the origin the package declares (`https://api.openai.com`) or the origin a
//! `baseurl` value supplies entirely (a local OpenAI-compatible service, for
//! example); the base URL must not end with `/` (the path `/responses` is
//! appended).
//!
//! # Failures
//!
//! Every document, evaluation, configuration, transport, status, and
//! response-shape failure is an `execution` error with a static detail
//! (SCXML `error.execution`); the prompt, the response, the model name, and
//! the key are never echoed.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; scripts/verify-component-authored-unsafe.py rejects authored unsafe"
    )
)]

/// Exact namespace URI of the `openai` namespace.
///
/// agentml-go uses the scheme-less `github.com/agentflare-ai/agentml-go/openai`;
/// this repository requires absolute namespace URIs
/// (`scxml_types::NamespaceUri`), so the URI is the package's path in this
/// repository at its release tag (a repository namespace).
pub const OPENAI_NAMESPACE_URI: &str =
    "https://github.com/stateforward/scxml.rs/namespaces/openai@v0.2.0";
/// The `env` slot of the API key (`env:apikey`).
pub const API_KEY_SLOT: &str = "apikey";
/// The `env` slot of the API base URL (`env:baseurl`).
pub const BASE_URL_SLOT: &str = "baseurl";
/// The API base URL when the element binds no [`BASE_URL_SLOT`].
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
/// Upper bound on the bytes of one function name (the `OpenAI` function-name
/// limit): an event whose sanitized name is longer gets no tool.
pub const MAX_FUNCTION_NAME_BYTES: usize = 64;
/// Upper bound on the assembled prompt, in bytes (wasm builds only; see
/// [`scxml_types::wasm_bound`]). Its JSON encoding stays inside the
/// host's request-body bound.
pub const MAX_PROMPT_BYTES: usize = scxml_types::wasm_bound(512 * 1024);

/// One attribute of a executed element.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attribute {
    /// Whether the attribute has a namespace (only unqualified attributes are
    /// element parameters).
    pub qualified: bool,
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
    },
    /// The end tag of the innermost open nested element.
    End,
}

/// A value the data model evaluated.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// A string.
    Text(String),
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
    /// The JSON body.
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

/// Optional generation parameters.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Options {
    /// `max-output-tokens`.
    pub max_output_tokens: Option<u64>,
    /// `reasoning` (the Responses API `reasoning.effort`).
    pub reasoning: Option<String>,
}

/// A section of the session snapshot a request can omit
/// (`session-snapshot-host.section`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Section {
    /// SCXML 5.10 system variables.
    System,
    /// User data-model variables.
    DataModel,
    /// The event queues.
    Queues,
    /// Pending delayed sends.
    PendingSends,
    /// Transitions an event could trigger.
    Transitions,
}

/// The snapshot of the system message: agentml-go's `ExcludeData` plus
/// `PruneSnapshot`'s removal of the transition listing, which becomes tools.
pub const SYSTEM_SNAPSHOT_EXCLUDE: [Section; 2] = [Section::DataModel, Section::Transitions];
/// The snapshot the tools are derived from: only its transitions are read.
pub const TOOLS_SNAPSHOT_EXCLUDE: [Section; 4] = [
    Section::System,
    Section::DataModel,
    Section::Queues,
    Section::PendingSends,
];

/// One `send_*` function offered to the model (agentml-go's `SendFunction`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SendFunction {
    /// The sanitized function name.
    pub name: String,
    /// The event name the function sends.
    pub event: String,
    /// The function description.
    pub description: String,
}

/// One function call the model returned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCall {
    /// The function name.
    pub name: String,
    /// The JSON arguments text.
    pub arguments: String,
}

/// One event to send through `execution-host.send`, as `<send>` would.
#[derive(Clone, Debug, PartialEq)]
pub struct SendCall {
    /// The event name.
    pub event: String,
    /// The `target`, when the model gave a non-empty one.
    pub target: Option<String>,
    /// The delay in milliseconds (0: immediate).
    pub delay_ms: u64,
    /// The event data, when the model gave any.
    pub data: Option<serde_json::Value>,
}

/// What the element did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Executed {
    /// `openai:generate` ran.
    Handled,
    /// The local name is not an `openai` element.
    NotHandled,
}

/// Why an element failed.
#[derive(Debug, Eq, PartialEq)]
pub enum Failure<E> {
    /// SCXML `error.execution`, with a static detail.
    Execution(&'static str),
    /// A non-recoverable host failure, passed through unchanged.
    Host(E),
}

/// The host capabilities one `execute` call uses.
pub trait OpenAiHost {
    /// A non-recoverable host failure.
    type Error;

    /// Evaluates one data-model expression; `Ok(Err(()))` is a recoverable
    /// evaluation failure.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn evaluate(&mut self, expression: &str) -> Result<Result<Value, ()>, Self::Error>;

    /// Assigns one string to one data-model location; `Ok(Err(()))` is a
    /// recoverable assignment failure.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn assign_text(&mut self, location: &str, value: &str) -> Result<Result<(), ()>, Self::Error>;

    /// The reference the host bound to one of the element's `env` slots,
    /// when the element names a variable for it that the host holds.
    fn slot(&mut self, slot: &str) -> Option<u32>;

    /// Performs one `POST`; `Ok(Err(()))` means the host refused it or the
    /// transport failed (no response).
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn post(&mut self, request: &Request) -> Result<Result<Response, ()>, Self::Error>;

    /// The executing session's snapshot document with `exclude` omitted;
    /// `Ok(Err(()))` when the host offers none or it could not be taken.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn snapshot(&mut self, exclude: &[Section]) -> Result<Result<String, ()>, Self::Error>;

    /// Sends one event exactly as `<send>` routes it; `Ok(Err(()))` is a
    /// recoverable send failure.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn send(&mut self, call: &SendCall) -> Result<Result<(), ()>, Self::Error>;
}

/// Executes one element with the given local name, attributes, and
/// flattened content.
///
/// # Errors
///
/// [`Failure::Execution`] for every document, evaluation, configuration,
/// transport, or response failure; [`Failure::Host`] for a non-recoverable
/// host failure.
pub fn execute<H: OpenAiHost>(
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

fn generate<H: OpenAiHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<(), Failure<H::Error>> {
    // agentml-go's `retry` re-asks the model after a streamed tool call fails
    // its event data schema, when earlier calls were already sent; the
    // snapshot carries no schemas and this port sends only after checking
    // every call, so there is nothing for it to correct.
    if attribute(attributes, "stream").is_some() || attribute(attributes, "retry").is_some() {
        return Err(Failure::Execution(
            "openai:generate stream and retry are not supported by this package version",
        ));
    }
    let location = attribute(attributes, "location")
        .map(str::trim)
        .filter(|location| !location.is_empty());
    let model = model(attributes, host)?;
    let prompt = prompt(attributes, content, host)?;
    let options = options(attributes)?;
    let api_key = host.slot(API_KEY_SLOT).ok_or(Failure::Execution(
        "openai:generate has no API key: env:apikey names no variable the host holds",
    ))?;
    let base_url = host.slot(BASE_URL_SLOT);
    let system = host
        .snapshot(&SYSTEM_SNAPSHOT_EXCLUDE)
        .map_err(Failure::Host)?
        .ok();
    let functions = match host
        .snapshot(&TOOLS_SNAPSHOT_EXCLUDE)
        .map_err(Failure::Host)?
    {
        Ok(document) => serde_json::from_str(&document)
            .map(|document| send_functions(&document))
            .map_err(|_| Failure::Execution("the session snapshot is not valid JSON"))?,
        Err(()) => Vec::new(),
    };
    if location.is_none() && functions.is_empty() {
        return Err(Failure::Execution(
            "openai:generate without location needs an event the active configuration accepts",
        ));
    }
    let request = build_request(
        base_url,
        api_key,
        &model,
        &Conversation {
            system: system.as_deref(),
            prompt: &prompt,
            functions: &functions,
            tool_choice: if location.is_some() {
                ToolChoice::Auto
            } else {
                ToolChoice::Required
            },
        },
        &options,
    )
    .map_err(Failure::Execution)?;
    let response = host
        .post(&request)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution("the model request was refused or failed"))?;
    if !(200..300).contains(&response.status) {
        return Err(Failure::Execution(
            "the model service answered with a non-success status",
        ));
    }
    let calls = tool_calls(&response.body).map_err(Failure::Execution)?;
    let text = match (location, calls.is_empty()) {
        (None, true) => return Err(Failure::Execution("the model called no function")),
        (Some(_), true) => Some(response_text(&response.body).map_err(Failure::Execution)?),
        (_, false) => response_text(&response.body).ok(),
    };
    // Every call is checked before the first event is sent.
    let sends = calls
        .iter()
        .map(|call| resolve_call(&functions, call))
        .collect::<Result<Vec<_>, _>>()
        .map_err(Failure::Execution)?;
    for send in &sends {
        host.send(send)
            .map_err(Failure::Host)?
            .map_err(|()| Failure::Execution("failed to send an event the model chose"))?;
    }
    match (location, text) {
        (Some(location), Some(text)) => host
            .assign_text(location, &text)
            .map_err(Failure::Host)?
            .map_err(|()| Failure::Execution("failed to store the model response")),
        _ => Ok(()),
    }
}

/// The unqualified attribute `local_name`, when present.
fn attribute<'a>(attributes: &'a [Attribute], local_name: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|attribute| !attribute.qualified && attribute.local_name == local_name)
        .map(|attribute| attribute.value.as_str())
}

/// `modelexpr`'s non-empty string value when present, else `model`.
fn model<H: OpenAiHost>(
    attributes: &[Attribute],
    host: &mut H,
) -> Result<String, Failure<H::Error>> {
    let expression = attribute(attributes, "modelexpr").map_or("", str::trim);
    if !expression.is_empty() {
        return match host.evaluate(expression).map_err(Failure::Host)? {
            Ok(Value::Text(text)) if !text.trim().is_empty() => Ok(text.trim().to_owned()),
            Ok(_) => Err(Failure::Execution(
                "modelexpr must evaluate to a non-empty string",
            )),
            Err(()) => Err(Failure::Execution("failed to evaluate modelexpr")),
        };
    }
    let model = attribute(attributes, "model").map_or("", str::trim);
    if model.is_empty() {
        return Err(Failure::Execution(
            "openai:generate requires a model or modelexpr attribute",
        ));
    }
    Ok(model.to_owned())
}

/// The `prompt` attribute, `promptexpr`'s string value, and every
/// `<openai:prompt>` child's trimmed text, non-empty parts joined by `\n`.
fn prompt<H: OpenAiHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<String, Failure<H::Error>> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(literal) = attribute(attributes, "prompt").filter(|text| !text.is_empty()) {
        parts.push(literal.to_owned());
    }
    let expression = attribute(attributes, "promptexpr").map_or("", str::trim);
    if !expression.is_empty() {
        match host.evaluate(expression).map_err(Failure::Host)? {
            Ok(Value::Text(text)) => {
                if !text.trim().is_empty() {
                    parts.push(text);
                }
            }
            Ok(Value::Other) => {
                return Err(Failure::Execution("promptexpr must evaluate to a string"));
            }
            Err(()) => return Err(Failure::Execution("failed to evaluate promptexpr")),
        }
    }
    parts.extend(child_prompts(content));
    if parts.is_empty() {
        return Err(Failure::Execution(
            "openai:generate requires a prompt, promptexpr, or openai:prompt child",
        ));
    }
    let prompt = parts.join("\n");
    if scxml_types::exceeds_wasm_bound(prompt.len(), MAX_PROMPT_BYTES) {
        return Err(Failure::Execution("the prompt exceeds its byte bound"));
    }
    Ok(prompt)
}

/// The trimmed text of every top-level `<openai:prompt>` child, in document
/// order, empty ones dropped (agentml-go's `processChildPrompts`).
#[must_use]
pub fn child_prompts(content: &[Content]) -> Vec<String> {
    let mut prompts = Vec::new();
    let mut depth = 0_usize;
    let mut current: Option<String> = None;
    for item in content {
        match item {
            Content::Start {
                namespace_uri,
                local_name,
            } => {
                if depth == 0
                    && namespace_uri.as_deref() == Some(OPENAI_NAMESPACE_URI)
                    && local_name == "prompt"
                {
                    current = Some(String::new());
                }
                depth = depth.saturating_add(1);
            }
            Content::End => {
                depth = depth.saturating_sub(1);
                if depth == 0
                    && let Some(text) = current.take()
                {
                    let text = text.trim();
                    if !text.is_empty() {
                        prompts.push(text.to_owned());
                    }
                }
            }
            Content::Text(text) => {
                if let Some(current) = current.as_mut() {
                    current.push_str(text);
                }
            }
        }
    }
    prompts
}

fn options<E>(attributes: &[Attribute]) -> Result<Options, Failure<E>> {
    let max_output_tokens = match attribute(attributes, "max-output-tokens").map(str::trim) {
        None => None,
        Some(text) => match text.parse::<u64>() {
            Ok(tokens) if tokens > 0 => Some(tokens),
            _ => {
                return Err(Failure::Execution(
                    "max-output-tokens must be a positive integer",
                ));
            }
        },
    };
    let reasoning = attribute(attributes, "reasoning")
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
        .map(str::to_owned);
    Ok(Options {
        max_output_tokens,
        reasoning,
    })
}

/// How the model may use the tools (`tool_choice`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolChoice {
    /// `auto` (with `location`).
    Auto,
    /// `required` (function-calling mode, without `location`).
    Required,
}

/// What one request asks the model.
#[derive(Clone, Copy, Debug)]
pub struct Conversation<'a> {
    /// The system message (the pruned session snapshot), when there is one.
    pub system: Option<&'a str>,
    /// The user message.
    pub prompt: &'a str,
    /// The tools; none omits `tools` and `tool_choice`.
    pub functions: &'a [SendFunction],
    /// The tool choice when there are tools.
    pub tool_choice: ToolChoice,
}

/// The Responses API request: `POST {base_url}/responses` (the `baseurl`
/// slot's reference, or [`DEFAULT_BASE_URL`]) with the `apikey` slot's
/// reference as a bearer key, the system message when there is one, the user
/// message, and the send functions as function tools.
///
/// # Errors
///
/// A static detail when the body cannot be encoded.
pub fn build_request(
    base_url: Option<u32>,
    api_key: u32,
    model: &str,
    conversation: &Conversation<'_>,
    options: &Options,
) -> Result<Request, &'static str> {
    let mut body = serde_json::Map::new();
    body.insert("model".into(), model.into());
    let mut input = Vec::new();
    if let Some(system) = conversation.system {
        input.push(serde_json::json!({ "role": "system", "content": system }));
    }
    input.push(serde_json::json!({ "role": "user", "content": conversation.prompt }));
    body.insert("input".into(), input.into());
    if !conversation.functions.is_empty() {
        body.insert("tools".into(), tools(conversation.functions));
        body.insert(
            "tool_choice".into(),
            match conversation.tool_choice {
                ToolChoice::Auto => "auto",
                ToolChoice::Required => "required",
            }
            .into(),
        );
    }
    if let Some(tokens) = options.max_output_tokens {
        body.insert("max_output_tokens".into(), tokens.into());
    }
    if let Some(effort) = &options.reasoning {
        body.insert("reasoning".into(), serde_json::json!({ "effort": effort }));
    }
    let body = serde_json::to_vec(&serde_json::Value::Object(body))
        .map_err(|_| "failed to encode the model request")?;
    Ok(Request {
        url: match base_url {
            Some(reference) => vec![
                Part::Reference(reference),
                Part::Literal("/responses".into()),
            ],
            None => vec![Part::Literal(format!("{DEFAULT_BASE_URL}/responses"))],
        },
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
        body,
    })
}

/// agentml-go's `sanitizeFunctionName`: every byte outside `[A-Za-z0-9_-]`
/// becomes `_`.
#[must_use]
pub fn sanitize_function_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The event a descriptor names (SCXML 3.12.1: `foo.*` and `foo.` match
/// exactly what `foo` matches), or `None` for `*`, which names no event.
fn descriptor_event(descriptor: &str) -> Option<&str> {
    let event = descriptor.strip_suffix(".*").unwrap_or(descriptor);
    let event = event.strip_suffix('.').unwrap_or(event);
    (!event.is_empty() && event != "*").then_some(event)
}

/// agentml-go's `BuildSendFunctions` over a `scxml.session-snapshot/v2`
/// document: one `send_<event>` function per distinct event descriptor of
/// the listed transitions (`snapshot.root.transitions`), in the
/// order of the transitions walk; `.` becomes `_` and the name is sanitized, so the
/// function maps back to the original event name. A descriptor that names
/// no event (`*`), a name over [`MAX_FUNCTION_NAME_BYTES`], or a name an
/// earlier event already took gets no function. A document without
/// transitions yields none.
#[must_use]
pub fn send_functions(document: &serde_json::Value) -> Vec<SendFunction> {
    let mut functions: Vec<SendFunction> = Vec::new();
    let transitions = document
        .pointer("/snapshot/root/transitions")
        .and_then(serde_json::Value::as_array);
    let descriptors = transitions
        .into_iter()
        .flatten()
        .filter_map(|transition| transition.get("events")?.as_array())
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter_map(descriptor_event);
    for event in descriptors {
        let name = sanitize_function_name(&format!("send_{}", event.replace('.', "_")));
        if name.len() > MAX_FUNCTION_NAME_BYTES
            || functions
                .iter()
                .any(|function| function.event == event || function.name == name)
        {
            continue;
        }
        functions.push(SendFunction {
            name,
            event: event.to_owned(),
            description: format!("Send event '{event}' through the SCXML interpreter"),
        });
    }
    functions
}

/// The Responses API function tools of agentml-go's
/// `convertToOpenAIToolsWithMapping`: optional string `target` and `delay`
/// parameters (no `data`: the snapshot carries no event data schema).
#[must_use]
pub fn tools(functions: &[SendFunction]) -> serde_json::Value {
    functions
        .iter()
        .map(|function| {
            serde_json::json!({
                "type": "function",
                "name": function.name,
                "description": function.description,
                "parameters": {
                    "type": "object",
                    "properties": {
                        "target": {
                            "type": "string",
                            "description": "Target destination for the event (optional)",
                        },
                        "delay": {
                            "type": "string",
                            "description": "Delay before sending the event in CSS2 format (optional)",
                        },
                    },
                },
            })
        })
        .collect()
}

/// The function calls of a Responses API result, in output order.
///
/// # Errors
///
/// A static detail when the body is not such a result or a call is
/// malformed.
pub fn tool_calls(body: &[u8]) -> Result<Vec<ToolCall>, &'static str> {
    const MALFORMED: &str = "the model response is not a valid Responses API result";
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|_| MALFORMED)?;
    let output = value
        .get("output")
        .and_then(serde_json::Value::as_array)
        .ok_or(MALFORMED)?;
    output
        .iter()
        .filter(|item| {
            item.get("type").and_then(serde_json::Value::as_str) == Some("function_call")
        })
        .map(|item| {
            let field = |key| item.get(key).and_then(serde_json::Value::as_str);
            match (field("name"), field("arguments")) {
                (Some(name), Some(arguments)) => Ok(ToolCall {
                    name: name.to_owned(),
                    arguments: arguments.to_owned(),
                }),
                _ => Err(MALFORMED),
            }
        })
        .collect()
}

/// One function call as the event it sends (agentml-go's `handleSendCall`):
/// the declared function's original event name; `target` (a string, empty
/// for none); `delay` (a CSS2 duration, [`delay_ms`]); and the data:
/// `data` when present, else every other argument as one object.
///
/// # Errors
///
/// A static detail for a function the tools did not declare or arguments
/// that are not a JSON object of that shape.
pub fn resolve_call(functions: &[SendFunction], call: &ToolCall) -> Result<SendCall, &'static str> {
    const INVALID: &str = "the model passed invalid function arguments";
    let function = functions
        .iter()
        .find(|function| function.name == call.name)
        .ok_or("the model called a function the active configuration does not accept")?;
    let serde_json::Value::Object(mut arguments) =
        serde_json::from_str(&call.arguments).map_err(|_| INVALID)?
    else {
        return Err(INVALID);
    };
    let text = |value: Option<serde_json::Value>| match value {
        None => Ok(None),
        Some(serde_json::Value::String(text)) => {
            Ok(Some(text.trim().to_owned()).filter(|text| !text.is_empty()))
        }
        Some(_) => Err(INVALID),
    };
    let target = text(arguments.remove("target"))?;
    let delay_ms = match text(arguments.remove("delay"))? {
        None => 0,
        Some(delay) => delay_ms(&delay).ok_or(INVALID)?,
    };
    let data = match arguments.remove("data") {
        Some(data) => Some(data),
        None if arguments.is_empty() => None,
        None => Some(serde_json::Value::Object(arguments)),
    };
    Ok(SendCall {
        event: function.event.clone(),
        target,
        delay_ms,
        data,
    })
}

/// A CSS2 duration (`<n>s`, `<n>.<f>s`, `<n>ms`) in whole milliseconds;
/// agentml-go's zero spellings (`0`, `0s`, `0ms`, `PT0S`, `P0D`) are 0.
#[must_use]
pub fn delay_ms(delay: &str) -> Option<u64> {
    let delay = delay.trim();
    if matches!(delay, "" | "0" | "0s" | "0ms" | "PT0S" | "P0D") {
        return Some(0);
    }
    if let Some(milliseconds) = delay.strip_suffix("ms") {
        return milliseconds
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| milliseconds.parse().ok())
            .flatten();
    }
    let seconds = delay.strip_suffix('s')?;
    let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > 3
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || (seconds.contains('.') && fraction.is_empty())
    {
        return None;
    }
    let fraction: u64 = format!("{fraction:0<3}").parse().ok()?;
    whole
        .parse::<u64>()
        .ok()?
        .checked_mul(1000)?
        .checked_add(fraction)
}

/// The output text of a Responses API result: the first `output_text` of
/// the first assistant `message` output item (agentml-go's extraction).
///
/// # Errors
///
/// A static detail when the body is not such a result.
pub fn response_text(body: &[u8]) -> Result<String, &'static str> {
    const MALFORMED: &str = "the model response is not a valid Responses API result";
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|_| MALFORMED)?;
    let output = value
        .get("output")
        .and_then(serde_json::Value::as_array)
        .ok_or(MALFORMED)?;
    let message = output
        .iter()
        .find(|item| item.get("type").and_then(serde_json::Value::as_str) == Some("message"))
        .ok_or("the model response carries no message")?;
    if message.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
        return Err("the model response carries no assistant message");
    }
    message
        .get("content")
        .and_then(serde_json::Value::as_array)
        .and_then(|content| {
            content.iter().find(|item| {
                item.get("type").and_then(serde_json::Value::as_str) == Some("output_text")
            })
        })
        .and_then(|item| item.get("text"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or("the model response carries no output text")
}

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    wit_bindgen::generate!({
        world: "executable-content-extension",
        path: "../../crates/scxml-wit/wit/scxml",
    });

    use std::cell::Cell;

    use exports::stateforward::scxml::executable_content_extension_api::Guest;
    use stateforward::scxml::telemetry::{self, Operation, Outcome, SignalEvent};
    use stateforward::scxml::types::{
        ComponentError, DataValue, ErrorCode, ErrorVersion, Event, EventType,
        ExecutableContentExtension, ExtensionContent, ExtensionOutcome, SendMessage, SendRequest,
        SlotReference, TraceContext,
    };
    use stateforward::scxml::{
        data_model_host, execution_host, outbound_http, session_snapshot_host,
    };

    use super::{
        Attribute, Content, Executed, Failure, OpenAiHost, Part, Request, Response, Section,
        SendCall, Value,
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
        /// Ordinal of the next generated send id of a delayed send.
        static NEXT_SEND: Cell<u64> = const { Cell::new(0) };
    }

    struct Component;

    /// The host capabilities of one `execute` call, with its slot references.
    struct Host {
        references: Vec<SlotReference>,
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

    impl OpenAiHost for Host {
        type Error = ComponentError;

        fn evaluate(&mut self, expression: &str) -> Result<Result<Value, ()>, ComponentError> {
            match data_model_host::evaluate(expression) {
                Ok(DataValue::Text(text)) => Ok(Ok(Value::Text(text))),
                Ok(_) => Ok(Ok(Value::Other)),
                Err(error) if recoverable(error.code) => Ok(Err(())),
                Err(error) => Err(error),
            }
        }

        fn assign_text(
            &mut self,
            location: &str,
            value: &str,
        ) -> Result<Result<(), ()>, ComponentError> {
            match data_model_host::assign(location, &DataValue::Text(value.to_owned())) {
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
                Err(error) if error.code == ErrorCode::Platform => Err(error),
                // No snapshot for this `execute` call, a bound, or an
                // unreadable component: the request goes without it, as
                // agentml-go's.
                Err(_) => Ok(Err(())),
            }
        }

        fn send(&mut self, call: &SendCall) -> Result<Result<(), ()>, ComponentError> {
            // A delayed send needs a caller-owned id; the instance lives
            // for one session, so its ordinal is unique in the session.
            let send_id = (call.delay_ms > 0).then(|| {
                let ordinal = NEXT_SEND.get();
                NEXT_SEND.set(ordinal.wrapping_add(1));
                format!("openai.send.{ordinal}")
            });
            let data = call.data.as_ref().map(data_value);
            let request = SendRequest {
                send_id,
                event: Event {
                    name: call.event.clone(),
                    event_type: EventType::External,
                    send_id: None,
                    origin: None,
                    origin_type: None,
                    invoke_id: None,
                    data: data.clone().unwrap_or(DataValue::Absent),
                    raw: None,
                    trace_context: None,
                },
                target: call.target.clone(),
                processor_type: None,
                delay_ms: call.delay_ms,
                message: if data.is_some() {
                    SendMessage::Content
                } else {
                    SendMessage::Empty
                },
            };
            match execution_host::send(&request) {
                Ok(()) => Ok(Ok(())),
                // As `<send>`: the host's own failure fails the session;
                // every other one is `error.execution` here.
                Err(error) if error.code == ErrorCode::Platform => Err(error),
                Err(_) => Ok(Err(())),
            }
        }
    }

    /// One JSON value as the event data `<send><content>` would carry.
    fn data_value(value: &serde_json::Value) -> DataValue {
        match value {
            serde_json::Value::Null => DataValue::Null,
            serde_json::Value::Bool(value) => DataValue::Boolean(*value),
            serde_json::Value::Number(number) => {
                number.as_f64().map_or(DataValue::Null, DataValue::Number)
            }
            serde_json::Value::String(text) => DataValue::Text(text.clone()),
            serde_json::Value::Array(_) => DataValue::JsonArray(value.to_string()),
            serde_json::Value::Object(_) => DataValue::JsonObject(value.to_string()),
        }
    }

    /// Evaluation and assignment failures (and a session with no data
    /// model) are document errors, as in agentml-go.
    fn recoverable(code: ErrorCode) -> bool {
        matches!(
            code,
            ErrorCode::InvalidInput | ErrorCode::Execution | ErrorCode::Unavailable
        )
    }

    fn content(items: Vec<ExtensionContent>) -> Vec<Content> {
        items
            .into_iter()
            .map(|item| match item {
                ExtensionContent::Text(text) => Content::Text(text),
                ExtensionContent::StartElement(start) => Content::Start {
                    namespace_uri: start.name.namespace_uri,
                    local_name: start.name.local_name,
                },
                ExtensionContent::EndElement(_) => Content::End,
            })
            .collect()
    }

    impl Guest for Component {
        fn namespace_uri() -> String {
            super::OPENAI_NAMESPACE_URI.into()
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
                if element.name.namespace_uri.as_deref() != Some(super::OPENAI_NAMESPACE_URI) {
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
                        qualified: attribute.name.namespace_uri.is_some(),
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
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    const OK_BODY: &str = r#"{"id":"r","output":[{"type":"reasoning","summary":[]},{"type":"message","role":"assistant","content":[{"type":"refusal","refusal":"x"},{"type":"output_text","text":"hello there"}]}]}"#;

    /// A scripted host: expressions evaluate from a table, assignments and
    /// requests are recorded, and the service answers with a fixed result.
    struct FakeHost {
        values: BTreeMap<String, Value>,
        slots: BTreeMap<String, u32>,
        assigned: Vec<(String, String)>,
        requests: Vec<Request>,
        answer: Result<Response, ()>,
        fail_assign: bool,
        /// The system-message and tools documents; `None`: no snapshot.
        documents: Option<(String, String)>,
        snapshot_requests: Vec<Vec<Section>>,
        sent: Vec<SendCall>,
        fail_send: bool,
    }

    impl Default for FakeHost {
        fn default() -> Self {
            Self {
                values: BTreeMap::new(),
                slots: BTreeMap::from([(API_KEY_SLOT.into(), 1)]),
                assigned: Vec::new(),
                requests: Vec::new(),
                answer: Ok(Response {
                    status: 200,
                    body: OK_BODY.into(),
                }),
                fail_assign: false,
                documents: None,
                snapshot_requests: Vec::new(),
                sent: Vec::new(),
                fail_send: false,
            }
        }
    }

    impl OpenAiHost for FakeHost {
        type Error = &'static str;

        fn evaluate(&mut self, expression: &str) -> Result<Result<Value, ()>, &'static str> {
            if expression == "fatal" {
                return Err("fatal");
            }
            Ok(self.values.get(expression).cloned().ok_or(()))
        }

        fn assign_text(
            &mut self,
            location: &str,
            value: &str,
        ) -> Result<Result<(), ()>, &'static str> {
            if self.fail_assign {
                return Ok(Err(()));
            }
            self.assigned.push((location.to_owned(), value.to_owned()));
            Ok(Ok(()))
        }

        fn slot(&mut self, slot: &str) -> Option<u32> {
            self.slots.get(slot).copied()
        }

        fn post(&mut self, request: &Request) -> Result<Result<Response, ()>, &'static str> {
            self.requests.push(request.clone());
            Ok(self.answer.clone())
        }

        fn snapshot(&mut self, exclude: &[Section]) -> Result<Result<String, ()>, &'static str> {
            self.snapshot_requests.push(exclude.to_vec());
            Ok(self
                .documents
                .clone()
                .map(|(system, tools)| {
                    if exclude.contains(&Section::Transitions) {
                        system
                    } else {
                        tools
                    }
                })
                .ok_or(()))
        }

        fn send(&mut self, call: &SendCall) -> Result<Result<(), ()>, &'static str> {
            if self.fail_send {
                return Ok(Err(()));
            }
            self.sent.push(call.clone());
            Ok(Ok(()))
        }
    }

    /// A tools document whose transitions list these transitions' events.
    fn transitions_document(events: &[&[&str]]) -> String {
        let transitions: Vec<serde_json::Value> = events
            .iter()
            .enumerate()
            .map(|(index, events)| {
                serde_json::json!({
                    "source": "ask", "index": index, "events": events, "targets": ["done"],
                    "type": "external", "has_cond": false,
                })
            })
            .collect();
        serde_json::json!({
            "version": "scxml.session-snapshot/v2",
            "snapshot": { "root": { "transitions": transitions } },
        })
        .to_string()
    }

    const SYSTEM_DOCUMENT: &str = r#"{"version":"scxml.session-snapshot/v2","snapshot":{"root":{"configuration":{"atomic":["ask"],"full":["ask"]}}}}"#;

    fn calling_host(answer_body: &str) -> FakeHost {
        FakeHost {
            documents: Some((
                SYSTEM_DOCUMENT.into(),
                transitions_document(&[&["answer.yes"], &["answer.no", "answer.*"]]),
            )),
            answer: Ok(Response {
                status: 200,
                body: answer_body.into(),
            }),
            ..FakeHost::default()
        }
    }

    fn function_call(name: &str, arguments: &str) -> String {
        serde_json::json!({
            "output": [{
                "type": "function_call", "call_id": "call_1", "name": name, "arguments": arguments,
            }],
        })
        .to_string()
    }

    fn send_call(event: &str) -> SendCall {
        SendCall {
            event: event.into(),
            target: None,
            delay_ms: 0,
            data: None,
        }
    }

    fn attributes(pairs: &[(&str, &str)]) -> Vec<Attribute> {
        pairs
            .iter()
            .map(|(name, value)| Attribute {
                qualified: false,
                local_name: (*name).to_owned(),
                value: (*value).to_owned(),
            })
            .collect()
    }

    fn start(namespace_uri: Option<&str>, local_name: &str) -> Content {
        Content::Start {
            namespace_uri: namespace_uri.map(str::to_owned),
            local_name: local_name.to_owned(),
        }
    }

    fn body(request: &Request) -> serde_json::Value {
        serde_json::from_slice(&request.body).expect("the request body is JSON")
    }

    #[test]
    fn generate_posts_one_user_message_and_stores_the_output_text() {
        let mut host = FakeHost::default();
        assert_eq!(
            execute(
                "generate",
                &attributes(&[
                    ("model", " gpt-4o "),
                    ("prompt", "Hi"),
                    ("location", " reply ")
                ]),
                &[],
                &mut host,
            ),
            Ok(Executed::Handled)
        );
        assert_eq!(host.assigned, [("reply".into(), "hello there".into())]);
        let [request] = host.requests.as_slice() else {
            panic!("exactly one request is sent");
        };
        assert_eq!(
            request.url,
            [Part::Literal("https://api.openai.com/v1/responses".into())]
        );
        assert_eq!(
            request.headers,
            [
                (
                    "authorization".into(),
                    vec![Part::Literal("Bearer ".into()), Part::Reference(1)]
                ),
                (
                    "content-type".into(),
                    vec![Part::Literal("application/json".into())]
                ),
                (
                    "accept".into(),
                    vec![Part::Literal("application/json".into())]
                ),
            ],
            "the key is a reference: the component never holds it"
        );
        assert_eq!(
            body(request),
            serde_json::json!({
                "model": "gpt-4o",
                "input": [{ "role": "user", "content": "Hi" }],
            })
        );
    }

    #[test]
    fn the_prompt_concatenates_attribute_expression_and_children_in_agentml_order() {
        let mut host = FakeHost::default();
        host.values
            .insert("p".into(), Value::Text("from expr".into()));
        let content = [
            Content::Text("\n  ".into()),
            start(Some(OPENAI_NAMESPACE_URI), "prompt"),
            Content::Text("  first ".into()),
            Content::Text("child  ".into()),
            Content::End,
            start(Some(OPENAI_NAMESPACE_URI), "prompt"),
            Content::Text("   ".into()),
            Content::End,
            start(Some("urn:other"), "prompt"),
            Content::Text("ignored".into()),
            Content::End,
            start(Some(OPENAI_NAMESPACE_URI), "prompt"),
            Content::Text("second".into()),
            Content::End,
        ];
        execute(
            "generate",
            &attributes(&[
                ("model", "m"),
                ("prompt", "from attribute"),
                ("promptexpr", "p"),
                ("location", "x"),
            ]),
            &content,
            &mut host,
        )
        .expect("generate succeeds");
        assert_eq!(
            body(&host.requests[0])["input"][0]["content"],
            "from attribute\nfrom expr\nfirst child\nsecond"
        );
    }

    #[test]
    fn modelexpr_wins_and_must_be_a_non_empty_string() {
        let mut host = FakeHost::default();
        host.values
            .insert("chosen".into(), Value::Text("gpt-4o-mini".into()));
        host.values.insert("blank".into(), Value::Text(" ".into()));
        host.values.insert("number".into(), Value::Other);
        execute(
            "generate",
            &attributes(&[
                ("model", "ignored"),
                ("modelexpr", "chosen"),
                ("prompt", "p"),
                ("location", "x"),
            ]),
            &[],
            &mut host,
        )
        .expect("generate succeeds");
        assert_eq!(body(&host.requests[0])["model"], "gpt-4o-mini");
        for (expression, detail) in [
            ("blank", "modelexpr must evaluate to a non-empty string"),
            ("number", "modelexpr must evaluate to a non-empty string"),
            ("missing", "failed to evaluate modelexpr"),
        ] {
            assert_eq!(
                execute(
                    "generate",
                    &attributes(&[
                        ("modelexpr", expression),
                        ("prompt", "p"),
                        ("location", "x")
                    ]),
                    &[],
                    &mut host,
                ),
                Err(Failure::Execution(detail))
            );
        }
        assert_eq!(
            execute(
                "generate",
                &attributes(&[("modelexpr", "fatal"), ("prompt", "p"), ("location", "x")]),
                &[],
                &mut host,
            ),
            Err(Failure::Host("fatal")),
            "non-recoverable host failures pass through"
        );
    }

    #[test]
    fn options_and_base_url_shape_the_request() {
        let mut host = FakeHost::default();
        host.slots.insert(BASE_URL_SLOT.into(), 2);
        execute(
            "generate",
            &attributes(&[
                ("model", "m"),
                ("prompt", "p"),
                ("location", "x"),
                ("max-output-tokens", " 64 "),
                ("reasoning", "low"),
            ]),
            &[],
            &mut host,
        )
        .expect("generate succeeds");
        assert_eq!(
            host.requests[0].url,
            [Part::Reference(2), Part::Literal("/responses".into())]
        );
        let body = body(&host.requests[0]);
        assert_eq!(body["max_output_tokens"], 64);
        assert_eq!(body["reasoning"], serde_json::json!({ "effort": "low" }));
    }

    #[test]
    fn document_errors_raise_error_execution_before_any_request() {
        for (pairs, detail) in [
            (
                &[("prompt", "p"), ("location", "x")][..],
                "openai:generate requires a model or modelexpr attribute",
            ),
            (
                &[("model", "m"), ("location", "x")][..],
                "openai:generate requires a prompt, promptexpr, or openai:prompt child",
            ),
            (
                &[("model", "m"), ("prompt", "p"), ("location", "  ")][..],
                "openai:generate without location needs an event the active configuration accepts",
            ),
            (
                &[
                    ("model", "m"),
                    ("prompt", "p"),
                    ("location", "x"),
                    ("stream", "true"),
                ][..],
                "openai:generate stream and retry are not supported by this package version",
            ),
            (
                &[
                    ("model", "m"),
                    ("prompt", "p"),
                    ("location", "x"),
                    ("retry", "2"),
                ][..],
                "openai:generate stream and retry are not supported by this package version",
            ),
            (
                &[
                    ("model", "m"),
                    ("prompt", "p"),
                    ("location", "x"),
                    ("max-output-tokens", "0"),
                ][..],
                "max-output-tokens must be a positive integer",
            ),
            (
                &[
                    ("model", "m"),
                    ("prompt", "p"),
                    ("location", "x"),
                    ("max-output-tokens", "many"),
                ][..],
                "max-output-tokens must be a positive integer",
            ),
            (
                &[("model", "m"), ("promptexpr", "number"), ("location", "x")][..],
                "promptexpr must evaluate to a string",
            ),
            (
                &[("model", "m"), ("promptexpr", "missing"), ("location", "x")][..],
                "failed to evaluate promptexpr",
            ),
        ] {
            let mut host = FakeHost::default();
            host.values.insert("number".into(), Value::Other);
            assert_eq!(
                execute("generate", &attributes(pairs), &[], &mut host),
                Err(Failure::Execution(detail)),
                "{pairs:?}"
            );
            assert!(host.requests.is_empty(), "{pairs:?} sends nothing");
        }
    }

    #[test]
    fn configuration_errors_raise_error_execution_before_any_request() {
        let generate = attributes(&[("model", "m"), ("prompt", "p"), ("location", "x")]);
        let mut host = FakeHost::default();
        host.slots.clear();
        assert_eq!(
            execute("generate", &generate, &[], &mut host),
            Err(Failure::Execution(
                "openai:generate has no API key: env:apikey names no variable the host holds"
            ))
        );
        assert!(host.requests.is_empty());
    }

    #[test]
    fn service_failures_raise_error_execution_and_store_nothing() {
        let generate = attributes(&[("model", "m"), ("prompt", "p"), ("location", "x")]);
        for (answer, detail) in [
            (Err(()), "the model request was refused or failed"),
            (
                Ok(Response {
                    status: 401,
                    body: OK_BODY.into(),
                }),
                "the model service answered with a non-success status",
            ),
            (
                Ok(Response {
                    status: 302,
                    body: Vec::new(),
                }),
                "the model service answered with a non-success status",
            ),
            (
                Ok(Response {
                    status: 200,
                    body: b"not json".to_vec(),
                }),
                "the model response is not a valid Responses API result",
            ),
            (
                Ok(Response {
                    status: 200,
                    body: br#"{"output":[{"type":"reasoning"}]}"#.to_vec(),
                }),
                "the model response carries no message",
            ),
        ] {
            let mut host = FakeHost {
                answer,
                ..FakeHost::default()
            };
            assert_eq!(
                execute("generate", &generate, &[], &mut host),
                Err(Failure::Execution(detail))
            );
            assert!(host.assigned.is_empty());
        }
        let mut host = FakeHost {
            fail_assign: true,
            ..FakeHost::default()
        };
        assert_eq!(
            execute("generate", &generate, &[], &mut host),
            Err(Failure::Execution("failed to store the model response"))
        );
    }

    #[test]
    fn response_text_follows_agentmls_extraction() {
        assert_eq!(response_text(OK_BODY.as_bytes()), Ok("hello there".into()));
        for (body, detail) in [
            (
                r#"{"output":{}}"#,
                "the model response is not a valid Responses API result",
            ),
            (
                "{}",
                "the model response is not a valid Responses API result",
            ),
            (
                r#"{"output":[{"type":"message","role":"user","content":[{"type":"output_text","text":"t"}]}]}"#,
                "the model response carries no assistant message",
            ),
            (
                r#"{"output":[{"type":"message","role":"assistant","content":[{"type":"refusal"}]}]}"#,
                "the model response carries no output text",
            ),
            (
                r#"{"output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":7}]}]}"#,
                "the model response carries no output text",
            ),
        ] {
            assert_eq!(response_text(body.as_bytes()), Err(detail), "{body}");
        }
    }

    #[test]
    fn only_generate_is_handled_and_nested_prompts_keep_their_own_text() {
        let mut host = FakeHost::default();
        assert_eq!(
            execute("prompt", &attributes(&[]), &[], &mut host),
            Ok(Executed::NotHandled)
        );
        assert_eq!(
            execute("Generate", &attributes(&[]), &[], &mut host),
            Ok(Executed::NotHandled),
            "local names match exactly, as in agentml-go"
        );
        let nested = [
            start(Some(OPENAI_NAMESPACE_URI), "prompt"),
            Content::Text("outer ".into()),
            start(Some(OPENAI_NAMESPACE_URI), "prompt"),
            Content::Text("inner".into()),
            Content::End,
            Content::End,
            Content::Text("trailing".into()),
        ];
        assert_eq!(child_prompts(&nested), ["outer inner"]);
    }

    /// Function-calling mode (no `location`): the pruned snapshot is the
    /// system message, the send functions of the transitions are the tools,
    /// `tool_choice` is `required`, and the call sends the original event.
    #[test]
    fn function_calling_sends_the_event_the_model_chose() {
        let mut host = calling_host(&function_call(
            "send_answer_no",
            r#"{"target":" ","delay":"1.5s","data":{"why":"unsure"}}"#,
        ));
        assert_eq!(
            execute(
                "generate",
                &attributes(&[("model", "m"), ("prompt", "Decide")]),
                &[],
                &mut host,
            ),
            Ok(Executed::Handled)
        );
        assert_eq!(
            host.snapshot_requests,
            [
                vec![Section::DataModel, Section::Transitions],
                vec![
                    Section::System,
                    Section::DataModel,
                    Section::Queues,
                    Section::PendingSends
                ],
            ],
            "the system message excludes the data and the transitions (agentml-go's ExcludeData and PruneSnapshot)"
        );
        let body = body(&host.requests[0]);
        assert_eq!(
            body["input"],
            serde_json::json!([
                { "role": "system", "content": SYSTEM_DOCUMENT },
                { "role": "user", "content": "Decide" },
            ])
        );
        assert_eq!(body["tool_choice"], "required");
        let names: Vec<&str> = body["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|tool| tool["name"].as_str().expect("name"))
            .collect();
        assert_eq!(names, ["send_answer_yes", "send_answer_no", "send_answer"]);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(
            body["tools"][0]["parameters"]["properties"]
                .as_object()
                .expect("properties")
                .keys()
                .collect::<Vec<_>>(),
            ["delay", "target"]
        );
        assert_eq!(
            host.sent,
            [SendCall {
                event: "answer.no".into(),
                target: None,
                delay_ms: 1500,
                data: Some(serde_json::json!({ "why": "unsure" })),
            }]
        );
        assert!(host.assigned.is_empty());
    }

    /// With `location`, `tool_choice` is `auto`: calls are sent, then the
    /// output text is assigned; a text-only answer is content generation.
    #[test]
    fn with_a_location_tools_are_optional_and_the_text_is_stored() {
        let both = serde_json::json!({
            "output": [
                { "type": "function_call", "name": "send_answer_yes", "arguments": "{}" },
                { "type": "message", "role": "assistant",
                  "content": [{ "type": "output_text", "text": "done" }] },
            ],
        })
        .to_string();
        let mut host = calling_host(&both);
        execute(
            "generate",
            &attributes(&[("model", "m"), ("prompt", "p"), ("location", "reply")]),
            &[],
            &mut host,
        )
        .expect("generate succeeds");
        assert_eq!(body(&host.requests[0])["tool_choice"], "auto");
        assert_eq!(host.sent, [send_call("answer.yes")]);
        assert_eq!(host.assigned, [("reply".into(), "done".into())]);

        let mut host = calling_host(OK_BODY);
        execute(
            "generate",
            &attributes(&[("model", "m"), ("prompt", "p"), ("location", "reply")]),
            &[],
            &mut host,
        )
        .expect("generate succeeds");
        assert!(host.sent.is_empty());
        assert_eq!(host.assigned, [("reply".into(), "hello there".into())]);
    }

    /// Every call is checked before the first event is sent: an undeclared
    /// function or malformed arguments sends nothing.
    #[test]
    fn invalid_calls_raise_error_execution_and_send_nothing() {
        let unknown = "the model called a function the active configuration does not accept";
        let invalid = "the model passed invalid function arguments";
        let valid_then = |second: &str| {
            serde_json::json!({
                "output": [
                    { "type": "function_call", "name": "send_answer_yes", "arguments": "{}" },
                    serde_json::from_str::<serde_json::Value>(second).expect("item"),
                ],
            })
            .to_string()
        };
        for (body, detail) in [
            (function_call("send_elsewhere", "{}"), unknown),
            (function_call("send_answer_yes", "not json"), invalid),
            (function_call("send_answer_yes", "[1]"), invalid),
            (function_call("send_answer_yes", r#"{"target":7}"#), invalid),
            (
                function_call("send_answer_yes", r#"{"delay":"soon"}"#),
                invalid,
            ),
            (
                valid_then(r#"{"type":"function_call","name":"send_nope","arguments":"{}"}"#),
                unknown,
            ),
            (
                valid_then(r#"{"type":"function_call","name":"send_answer_no"}"#),
                "the model response is not a valid Responses API result",
            ),
            (OK_BODY.to_owned(), "the model called no function"),
        ] {
            let mut host = calling_host(&body);
            assert_eq!(
                execute(
                    "generate",
                    &attributes(&[("model", "m"), ("prompt", "p")]),
                    &[],
                    &mut host,
                ),
                Err(Failure::Execution(detail)),
                "{body}"
            );
            assert!(host.sent.is_empty(), "{body}");
        }
        let mut host = calling_host(&function_call("send_answer_yes", "{}"));
        host.fail_send = true;
        assert_eq!(
            execute(
                "generate",
                &attributes(&[("model", "m"), ("prompt", "p")]),
                &[],
                &mut host,
            ),
            Err(Failure::Execution(
                "failed to send an event the model chose"
            ))
        );
    }

    /// Without a snapshot there is no system message and no tool, so
    /// function-calling mode fails before any request.
    #[test]
    fn function_calling_needs_an_accepted_event() {
        let mut host = FakeHost::default();
        assert_eq!(
            execute(
                "generate",
                &attributes(&[("model", "m"), ("prompt", "p")]),
                &[],
                &mut host,
            ),
            Err(Failure::Execution(
                "openai:generate without location needs an event the active configuration accepts"
            ))
        );
        assert!(host.requests.is_empty());
        let mut host = FakeHost {
            documents: Some((SYSTEM_DOCUMENT.into(), transitions_document(&[&["*"]]))),
            ..FakeHost::default()
        };
        assert_eq!(
            execute(
                "generate",
                &attributes(&[("model", "m"), ("prompt", "p")]),
                &[],
                &mut host,
            ),
            Err(Failure::Execution(
                "openai:generate without location needs an event the active configuration accepts"
            )),
            "`*` names no event"
        );
    }

    #[test]
    fn send_functions_follow_agentmls_naming_and_mapping() {
        let long = "e".repeat(MAX_FUNCTION_NAME_BYTES);
        let document: serde_json::Value = serde_json::from_str(&transitions_document(&[
            &["user.reply", "user.reply.*"],
            &["*", "error.*", "done.invoke."],
            &["a-b:c"],
            &["a_b.c", "a.b.c"],
            &[long.as_str()],
        ]))
        .expect("JSON");
        let functions = send_functions(&document);
        let pairs: Vec<(&str, &str)> = functions
            .iter()
            .map(|function| (function.name.as_str(), function.event.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("send_user_reply", "user.reply"),
                ("send_error", "error"),
                ("send_done_invoke", "done.invoke"),
                ("send_a-b_c", "a-b:c"),
                ("send_a_b_c", "a_b.c"),
            ],
            "descriptors normalize (SCXML 3.12.1), `*` and over-long names get none, and a taken name keeps its first event"
        );
        assert_eq!(
            functions[0].description,
            "Send event 'user.reply' through the SCXML interpreter"
        );
        assert_eq!(sanitize_function_name("send_a.b/c d"), "send_a_b_c_d");
        assert!(send_functions(&serde_json::json!({ "snapshot": {} })).is_empty());
    }

    #[test]
    fn calls_resolve_to_sends_like_agentmls_handle_send_call() {
        let functions = [SendFunction {
            name: "send_go".into(),
            event: "go".into(),
            description: String::new(),
        }];
        let call = |arguments: &str| {
            resolve_call(
                &functions,
                &ToolCall {
                    name: "send_go".into(),
                    arguments: arguments.into(),
                },
            )
        };
        assert_eq!(call("{}"), Ok(send_call("go")));
        assert_eq!(
            call(r##"{"target":"#_internal","delay":"0","x":1,"y":"z"}"##),
            Ok(SendCall {
                event: "go".into(),
                target: Some("#_internal".into()),
                delay_ms: 0,
                data: Some(serde_json::json!({ "x": 1, "y": "z" })),
            }),
            "without `data`, the other arguments are the data"
        );
        assert_eq!(
            call(r#"{"data":[1],"x":1}"#).map(|send| send.data),
            Ok(Some(serde_json::json!([1])))
        );
        for (delay, expected) in [
            ("250ms", Some(250)),
            ("2s", Some(2000)),
            ("0.25s", Some(250)),
            ("1.5s", Some(1500)),
            ("PT0S", Some(0)),
            (" 0ms ", Some(0)),
            ("1.2345s", None),
            ("1.s", None),
            (".5s", None),
            ("1.5ms", None),
            ("-1s", None),
            ("5m", None),
            ("PT5S", None),
            ("99999999999999999999s", None),
        ] {
            assert_eq!(delay_ms(delay), expected, "{delay}");
        }
    }

    /// `MAX_PROMPT_BYTES` bounds the prompt of a wasm build only: natively
    /// any prompt is sent.
    #[test]
    fn the_prompt_is_unbounded_natively() {
        assert_eq!(MAX_PROMPT_BYTES, usize::MAX);
        let mut host = FakeHost::default();
        let long = "x".repeat(600 * 1024);
        execute(
            "generate",
            &attributes(&[("model", "m"), ("prompt", &long), ("location", "x")]),
            &[],
            &mut host,
        )
        .expect("a native build sets no prompt bound");
    }
}
