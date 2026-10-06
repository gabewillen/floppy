//! SCXML `BasicHTTP` Event I/O Processor (SCXML 1.0 appendix C.2).
//!
//! The processor owns every `BasicHTTP` decision: it encodes an evaluated
//! `<send>` as an `application/x-www-form-urlencoded` HTTP POST, interprets
//! the recipient's response, validates and decodes inbound messages into
//! external SCXML events, and chooses the status code answered for each
//! delivery outcome. It performs no I/O of its own: requests leave only
//! through the caller-supplied [`HttpTransport`] capability (the Wasm
//! component imports it as `stateforward:scxml-basichttp/http-transport`).
//! It reads no clock, randomness, environment, or network, and every input
//! is size-bounded before use.
//!
//! # Wire format
//!
//! * `_scxmleventname=<event>` first when the send names an event, then
//!   every `namelist`/`<param>` entry in document order as `name=value`.
//! * `<content>` (never combined with parameters, SCXML 6.2) becomes one
//!   bare percent-encoded body segment, so a receiver can tell it from a
//!   parameter: the encoder escapes every `=` and `&` it contains.
//! * Every byte outside RFC 3986 `unreserved` is percent-encoded with
//!   uppercase hexadecimal; a space is `%20`.
//!
//! Inbound form bodies decode `+` as a space (WHATWG form decoding) and
//! reject malformed percent escapes and non-UTF-8 text instead of guessing.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; scripts/verify-component-authored-unsafe.py rejects authored unsafe"
    )
)]

#[cfg(target_arch = "wasm32")]
mod guest;

use std::fmt;

use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};
use scxml_types::{DataValue, Event, EventName};

/// Canonical `BasicHTTP` Event I/O Processor type URI (SCXML C.2).
pub const BASICHTTP_EVENT_PROCESSOR_URI: &str =
    "http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor";
/// Aliases documents may use for this processor's type, in `_ioprocessors`
/// order (WIT `basichttp-api.aliases`).
pub const BASICHTTP_PROCESSOR_ALIASES: [&str; 1] = ["basichttp"];
/// Form parameter (or header) naming the SCXML event (SCXML C.2).
pub const EVENT_NAME_PARAMETER: &str = "_scxmleventname";
/// Event name used when a message carries no `_scxmleventname` (SCXML C.2:
/// the name of the HTTP method).
pub const DEFAULT_EVENT_NAME: &str = "HTTP.POST";
/// Media type of every request this processor sends.
pub const FORM_MEDIA_TYPE: &str = "application/x-www-form-urlencoded";
/// The only request method `BasicHTTP` accepts (SCXML C.2).
pub const POST_METHOD: &str = "POST";
/// Maximum bytes of a request method.
pub const MAX_METHOD_BYTES: usize = 16;
/// Maximum bytes of a request URI or send target.
pub const MAX_URI_BYTES: usize = 16_384;
/// Maximum header fields in one request or response.
pub const MAX_HEADERS: usize = 64;
/// Maximum bytes of one header name.
pub const MAX_HEADER_NAME_BYTES: usize = 256;
/// Maximum bytes of one header value.
pub const MAX_HEADER_VALUE_BYTES: usize = 8_192;
/// Maximum bytes of one request or response body.
pub const MAX_BODY_BYTES: usize = 65_536;
/// Maximum parameters in one send or inbound form body.
pub const MAX_PARAMS: usize = 256;
/// Maximum bytes of one parameter name or event name.
pub const MAX_NAME_BYTES: usize = 1_024;
/// Maximum bytes of the raw event text (`_event.raw`).
pub const MAX_RAW_BYTES: usize = scxml_types::MAX_EVENT_DATA_BYTES;

/// One HTTP header field. Names compare ASCII case-insensitively.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpHeader {
    /// Field name: 1..=[`MAX_HEADER_NAME_BYTES`] RFC 9110 token characters.
    pub name: String,
    /// Field value: at most [`MAX_HEADER_VALUE_BYTES`], no CR, LF, or NUL.
    pub value: String,
}

/// One HTTP request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpRequest {
    /// Request method token.
    pub method: String,
    /// Absolute request URI.
    pub uri: String,
    /// Header fields in order.
    pub headers: Vec<HttpHeader>,
    /// Body bytes.
    pub body: Vec<u8>,
}

/// One HTTP response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpResponse {
    /// Status code, 100..=599.
    pub status: u16,
    /// Header fields in order.
    pub headers: Vec<HttpHeader>,
    /// Body bytes.
    pub body: Vec<u8>,
}

/// No response was obtained from the transport (unreachable address,
/// refused connection, or transport failure).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransportFailure;

/// The narrow outbound HTTP capability the embedder supplies.
///
/// Implementations carry one request and return the recipient's response
/// synchronously. They never interpret SCXML. A deterministic in-process
/// loopback and a real HTTP client are both valid implementations.
pub trait HttpTransport {
    /// Performs one request.
    ///
    /// # Errors
    ///
    /// [`TransportFailure`] when no response was obtained.
    fn post(&mut self, request: &HttpRequest) -> Result<HttpResponse, TransportFailure>;
}

/// One HTTP POST parameter produced by `namelist` or `<param>`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Param {
    /// Parameter name: 1..=[`MAX_NAME_BYTES`] bytes.
    pub name: String,
    /// Evaluated value; [`DataValue::Absent`] and [`DataValue::Undefined`]
    /// are rejected.
    pub value: DataValue,
}

/// One evaluated `<send>` addressed to the `BasicHTTP` processor. The host
/// evaluates every expression; the processor never sees one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutboundSend {
    /// Evaluated `event`/`eventexpr`, sent as `_scxmleventname`.
    pub event_name: Option<String>,
    /// Evaluated `target`/`targetexpr`: an absolute `http`/`https` URI.
    pub target: Option<String>,
    /// `namelist` entries then `<param>` children, in document order.
    pub params: Vec<Param>,
    /// Evaluated `<content>`; mutually exclusive with `params`.
    pub content: Option<DataValue>,
}

/// How the sending session must react to one [`SendError`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendErrorKind {
    /// A malformed request: the session raises `error.execution`.
    InvalidInput,
    /// A byte or count bound was exceeded: the session raises
    /// `error.execution`.
    PayloadTooLarge,
    /// The message could not be delivered: the session raises
    /// `error.communication` (SCXML C.2).
    Unavailable,
}

/// Closed failure of one `BasicHTTP` send. Nothing is sent when validation
/// fails.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendError {
    /// Neither `target` nor `targetexpr` was given (SCXML C.2).
    MissingTarget,
    /// The target is not a bounded absolute `http`/`https` URI.
    InvalidTarget,
    /// The event name is empty or over [`MAX_NAME_BYTES`].
    InvalidEventName,
    /// A parameter name is empty or over [`MAX_NAME_BYTES`].
    InvalidParamName,
    /// A parameter or content value is absent or undefined.
    InvalidValue,
    /// `<content>` was combined with parameters (SCXML 6.2).
    ContentWithParams,
    /// Too many parameters, or the encoded body exceeds [`MAX_BODY_BYTES`].
    PayloadTooLarge,
    /// The transport returned no response.
    TransportFailed,
    /// The recipient answered with a non-2xx status.
    NotAccepted,
    /// The transport returned a response outside the documented bounds.
    InvalidResponse,
}

impl SendError {
    /// Returns the session-visible classification of this failure.
    #[must_use]
    pub fn kind(self) -> SendErrorKind {
        match self {
            Self::InvalidTarget
            | Self::InvalidEventName
            | Self::InvalidParamName
            | Self::InvalidValue
            | Self::ContentWithParams => SendErrorKind::InvalidInput,
            Self::PayloadTooLarge => SendErrorKind::PayloadTooLarge,
            Self::MissingTarget
            | Self::TransportFailed
            | Self::NotAccepted
            | Self::InvalidResponse => SendErrorKind::Unavailable,
        }
    }

    /// Static, payload-free description (at most 160 bytes).
    #[must_use]
    pub fn detail(self) -> &'static str {
        match self {
            Self::MissingTarget => "a BasicHTTP send requires a target",
            Self::InvalidTarget => "the send target is not a bounded absolute http or https URI",
            Self::InvalidEventName => "the event name is empty or exceeds its byte bound",
            Self::InvalidParamName => "a parameter name is empty or exceeds its byte bound",
            Self::InvalidValue => "a parameter or content value is absent or undefined",
            Self::ContentWithParams => "content cannot be combined with namelist or param",
            Self::PayloadTooLarge => "the encoded request exceeds its byte or count bound",
            Self::TransportFailed => "the transport returned no response",
            Self::NotAccepted => "the recipient did not answer with a 2xx status",
            Self::InvalidResponse => "the transport response violates its bounds",
        }
    }
}

impl fmt::Display for SendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.detail())
    }
}

impl std::error::Error for SendError {}

/// Why an inbound message was not turned into an event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rejection {
    /// Malformed request line, header, body encoding, or event name.
    InvalidMessage,
    /// The method is not `POST` (SCXML C.2).
    MethodNotAllowed,
    /// The message or its raw event text exceeds a byte or count bound.
    PayloadTooLarge,
}

/// Result of offering one inbound message to its target session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Delivery {
    /// The event was added to the session's external queue.
    Enqueued,
    /// [`BasicHttpIoProcessor::receive`] rejected the message.
    Rejected(Rejection),
    /// No live session answers the request URI.
    UnknownSession,
    /// The session cannot take the message now (the bundled browser host,
    /// while the target session is mid-call; the Wasmtime host never).
    QueueFull,
}

/// Stateless `BasicHTTP` Event I/O Processor.
#[derive(Clone, Copy, Debug, Default)]
pub struct BasicHttpIoProcessor;

impl BasicHttpIoProcessor {
    /// Returns the canonical processor type URI.
    #[must_use]
    pub fn type_uri(self) -> &'static str {
        BASICHTTP_EVENT_PROCESSOR_URI
    }

    /// Validates one send and encodes it as an HTTP POST request.
    ///
    /// # Errors
    ///
    /// Every [`SendError`] validation failure; transport failures cannot
    /// occur here.
    pub fn encode(self, send: &OutboundSend) -> Result<HttpRequest, SendError> {
        encode_request(send)
    }

    /// Validates and encodes one send exactly as [`Self::send`] does,
    /// without performing it (WIT `basichttp-api.check`): the validation
    /// failures [`Self::send`] reports before calling the transport, except
    /// [`SendError::MissingTarget`], a delivery failure reported when the
    /// message is dispatched. A host checks a delayed send before
    /// scheduling it (SCXML 6.2: "The Processor MUST evaluate all arguments
    /// to `<send>` when the `<send>` element is evaluated, and not when the
    /// message is actually dispatched."). Pure.
    ///
    /// # Errors
    ///
    /// Every [`SendError`] validation failure but
    /// [`SendError::MissingTarget`].
    pub fn check(self, send: &OutboundSend) -> Result<(), SendError> {
        if send.target.is_some() {
            return encode_request(send).map(|_| ());
        }
        // A placeholder target keeps every other check identical.
        let addressed = OutboundSend {
            target: Some(CHECK_PLACEHOLDER_TARGET.to_owned()),
            ..send.clone()
        };
        encode_request(&addressed).map(|_| ())
    }

    /// Encodes one send and performs it through `transport` (SCXML C.2).
    /// Succeeds only when the recipient answers with a 2xx status.
    ///
    /// # Errors
    ///
    /// Validation failures before anything is sent; then
    /// [`SendError::TransportFailed`], [`SendError::InvalidResponse`], or
    /// [`SendError::NotAccepted`].
    pub fn send<T: HttpTransport + ?Sized>(
        self,
        send: &OutboundSend,
        transport: &mut T,
    ) -> Result<(), SendError> {
        let span = observe(
            Module::IoProcessorBasicHttpComponent,
            Operation::BasicHttpSend,
        );
        let result = (|| {
            let request = self.encode(send)?;
            let response = transport
                .post(&request)
                .map_err(|TransportFailure| SendError::TransportFailed)?;
            validate_response(&response)?;
            if (200..=299).contains(&response.status) {
                Ok(())
            } else {
                Err(SendError::NotAccepted)
            }
        })();
        if result.is_ok() {
            span.event(SignalEvent::IoProcessorSendAccepted);
        } else {
            span.event(SignalEvent::IoProcessorSendRejected);
            span.fail(ErrorCategory::Execution);
        }
        result
    }

    /// Validates and decodes one inbound message into the external SCXML
    /// event to enqueue. Pure: nothing is enqueued.
    ///
    /// The event name is the first `_scxmleventname` form parameter, else
    /// the first `_scxmleventname` header, else `HTTP.POST`. Form
    /// parameters other than `_scxmleventname` become a canonical JSON
    /// object of strings (a repeated name becomes an array in message
    /// order); any other content (a non-form body, or a bare form segment)
    /// becomes JSON, an XML document, or text by its `Content-Type` and
    /// B.2.8.1's content rules (`content_data`); `_event.raw` is the message
    /// as received.
    ///
    /// # Errors
    ///
    /// The [`Rejection`] to answer with.
    pub fn receive(self, request: &HttpRequest) -> Result<Event, Rejection> {
        let span = observe(
            Module::IoProcessorBasicHttpComponent,
            Operation::BasicHttpReceive,
        );
        let result = decode_request(request);
        if result.is_ok() {
            span.event(SignalEvent::ScxmlEventReceived);
        } else {
            span.event(SignalEvent::InputRejected);
            span.fail(ErrorCategory::InvalidInput);
        }
        result
    }

    /// The response for one delivery outcome: 200 after the event was
    /// enqueued (SCXML C.2), 400/405/413 for rejected messages, 404 for an
    /// unknown session, and 503 for a full queue. Total and pure.
    #[must_use]
    pub fn respond(self, delivery: Delivery) -> HttpResponse {
        let (status, headers) = match delivery {
            Delivery::Enqueued => (200, Vec::new()),
            Delivery::Rejected(Rejection::InvalidMessage) => (400, Vec::new()),
            Delivery::Rejected(Rejection::MethodNotAllowed) => (
                405,
                vec![HttpHeader {
                    name: "Allow".into(),
                    value: POST_METHOD.into(),
                }],
            ),
            Delivery::Rejected(Rejection::PayloadTooLarge) => (413, Vec::new()),
            Delivery::UnknownSession => (404, Vec::new()),
            Delivery::QueueFull => (503, Vec::new()),
        };
        HttpResponse {
            status,
            headers,
            body: Vec::new(),
        }
    }
}

/// A valid target standing in for an absent one in [`BasicHttpIoProcessor::check`].
const CHECK_PLACEHOLDER_TARGET: &str = "http://check.invalid/";

fn encode_request(send: &OutboundSend) -> Result<HttpRequest, SendError> {
    let target = send.target.as_deref().ok_or(SendError::MissingTarget)?;
    if !is_http_target(target) {
        return Err(SendError::InvalidTarget);
    }
    if send.content.is_some() && !send.params.is_empty() {
        return Err(SendError::ContentWithParams);
    }
    if send.params.len() > MAX_PARAMS {
        return Err(SendError::PayloadTooLarge);
    }
    let mut body = BoundedBody::default();
    if let Some(name) = send.event_name.as_deref() {
        if name.is_empty() || name.len() > MAX_NAME_BYTES {
            return Err(SendError::InvalidEventName);
        }
        body.pair(EVENT_NAME_PARAMETER, name)?;
    }
    for param in &send.params {
        if param.name.is_empty() || param.name.len() > MAX_NAME_BYTES {
            return Err(SendError::InvalidParamName);
        }
        body.pair(&param.name, &value_text(&param.value)?)?;
    }
    if let Some(content) = &send.content {
        body.segment(&value_text(content)?)?;
    }
    Ok(HttpRequest {
        method: POST_METHOD.into(),
        uri: target.to_owned(),
        headers: vec![HttpHeader {
            name: "Content-Type".into(),
            value: FORM_MEDIA_TYPE.into(),
        }],
        body: body.bytes.into_bytes(),
    })
}

/// Form body under construction, bounded by [`MAX_BODY_BYTES`].
#[derive(Default)]
struct BoundedBody {
    bytes: String,
}

impl BoundedBody {
    fn pair(&mut self, name: &str, value: &str) -> Result<(), SendError> {
        self.separator().and_then(|()| {
            self.encoded(name)?;
            self.push("=")?;
            self.encoded(value)
        })
    }

    fn segment(&mut self, value: &str) -> Result<(), SendError> {
        self.separator().and_then(|()| self.encoded(value))
    }

    fn separator(&mut self) -> Result<(), SendError> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            self.push("&")
        }
    }

    fn encoded(&mut self, value: &str) -> Result<(), SendError> {
        {
            for byte in value.bytes() {
                if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                    self.push(char::from(byte).encode_utf8(&mut [0; 4]))?;
                } else {
                    let high = HEX_UPPER.get(usize::from(byte >> 4)).copied();
                    let low = HEX_UPPER.get(usize::from(byte & 0x0f)).copied();
                    let (Some(high), Some(low)) = (high, low) else {
                        return Err(SendError::InvalidValue);
                    };
                    let escaped = [b'%', high, low];
                    let escaped =
                        std::str::from_utf8(&escaped).map_err(|_| SendError::InvalidValue)?;
                    self.push(escaped)?;
                }
            }
            Ok(())
        }
    }

    fn push(&mut self, text: &str) -> Result<(), SendError> {
        if self
            .bytes
            .len()
            .checked_add(text.len())
            .is_some_and(|total| total <= MAX_BODY_BYTES)
        {
            self.bytes.push_str(text);
            Ok(())
        } else {
            Err(SendError::PayloadTooLarge)
        }
    }
}

const HEX_UPPER: [u8; 16] = *b"0123456789ABCDEF";

/// Text form of one parameter or content value. Numbers use Rust's
/// shortest round-trip decimal notation, never an exponent; `-0` is `0`.
fn value_text(value: &DataValue) -> Result<String, SendError> {
    match value {
        DataValue::Absent | DataValue::Undefined => Err(SendError::InvalidValue),
        DataValue::Null => Ok("null".to_owned()),
        DataValue::Bool(value) => Ok(value.to_string()),
        DataValue::Number(number) => {
            let number = number.get();
            Ok(if number == 0.0 {
                "0".to_owned()
            } else {
                number.to_string()
            })
        }
        DataValue::String(text)
        | DataValue::JsonArray(text)
        | DataValue::JsonObject(text)
        | DataValue::Xml(text) => Ok(text.clone()),
    }
}

/// Whether `target` is a bounded absolute `http`/`https` URI of visible
/// ASCII characters with a non-empty authority.
fn is_http_target(target: &str) -> bool {
    let rest = ["https://", "http://"].into_iter().find_map(|scheme| {
        target
            .get(..scheme.len())
            .filter(|prefix| prefix.eq_ignore_ascii_case(scheme))
            .and_then(|_| target.get(scheme.len()..))
    });
    is_visible_uri(target) && rest.is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
}

/// Whether `uri` is 1..=[`MAX_URI_BYTES`] visible ASCII characters.
fn is_visible_uri(uri: &str) -> bool {
    !uri.is_empty() && uri.len() <= MAX_URI_BYTES && uri.bytes().all(|byte| byte.is_ascii_graphic())
}

fn is_token(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// Why a header list failed [`validate_headers`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HeaderError {
    /// The header count or a name or value length is over its bound.
    OverBound,
    /// A name is not an HTTP token, or a value holds CR, LF, or NUL.
    Malformed,
}

/// Validates header count and fields.
fn validate_headers(headers: &[HttpHeader]) -> Result<(), HeaderError> {
    if headers.len() > MAX_HEADERS {
        return Err(HeaderError::OverBound);
    }
    for header in headers {
        if header.name.len() > MAX_HEADER_NAME_BYTES || header.value.len() > MAX_HEADER_VALUE_BYTES
        {
            return Err(HeaderError::OverBound);
        }
        if !is_token(&header.name, MAX_HEADER_NAME_BYTES)
            || header
                .value
                .bytes()
                .any(|byte| matches!(byte, b'\r' | b'\n' | 0))
        {
            return Err(HeaderError::Malformed);
        }
    }
    Ok(())
}

fn validate_response(response: &HttpResponse) -> Result<(), SendError> {
    let valid = (100..=599).contains(&response.status)
        && response.body.len() <= MAX_BODY_BYTES
        && validate_headers(&response.headers).is_ok();
    if valid {
        Ok(())
    } else {
        Err(SendError::InvalidResponse)
    }
}

fn header_value<'a>(headers: &'a [HttpHeader], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str())
}

/// Decoded form body: the first `_scxmleventname`, the other parameters in
/// message order, and bare (content) segments.
#[derive(Default)]
struct FormBody {
    event_name: Option<String>,
    params: Vec<(String, String)>,
    content: Vec<String>,
}

fn decode_form(body: &str) -> Result<FormBody, Rejection> {
    let mut form = FormBody::default();
    let mut segments = 0_usize;
    for segment in body.split('&').filter(|segment| !segment.is_empty()) {
        segments = segments.saturating_add(1);
        if segments > MAX_PARAMS.saturating_add(1) {
            return Err(Rejection::PayloadTooLarge);
        }
        if let Some((name, value)) = segment.split_once('=') {
            let name = percent_decode(name)?;
            let value = percent_decode(value)?;
            if name == EVENT_NAME_PARAMETER {
                // SCXML C.2: the first instance names the event; later
                // instances are not message content.
                if form.event_name.is_none() {
                    form.event_name = Some(value);
                }
            } else {
                form.params.push((name, value));
            }
        } else {
            form.content.push(percent_decode(segment)?);
        }
    }
    Ok(form)
}

fn percent_decode(text: &str) -> Result<String, Rejection> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut input = text.bytes();
    while let Some(byte) = input.next() {
        match byte {
            b'+' => bytes.push(b' '),
            b'%' => {
                let high = input.next().and_then(hex_value);
                let low = input.next().and_then(hex_value);
                let (Some(high), Some(low)) = (high, low) else {
                    return Err(Rejection::InvalidMessage);
                };
                bytes.push((high << 4) | low);
            }
            other => bytes.push(other),
        }
    }
    String::from_utf8(bytes).map_err(|_| Rejection::InvalidMessage)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Canonical JSON object of string parameters; a repeated name becomes an
/// array of strings in message order.
fn params_object(params: Vec<(String, String)>) -> Result<DataValue, Rejection> {
    let mut object = serde_json::Map::new();
    for (name, value) in params {
        match object.get_mut(&name) {
            None => {
                object.insert(name, serde_json::Value::String(value));
            }
            Some(serde_json::Value::Array(values)) => {
                values.push(serde_json::Value::String(value));
            }
            Some(existing) => {
                let first = std::mem::replace(existing, serde_json::Value::Null);
                *existing = serde_json::Value::Array(vec![first, serde_json::Value::String(value)]);
            }
        }
    }
    let text = serde_json::to_string(&serde_json::Value::Object(object))
        .map_err(|_| Rejection::InvalidMessage)?;
    if text.len() > scxml_types::MAX_EVENT_DATA_BYTES {
        return Err(Rejection::PayloadTooLarge);
    }
    DataValue::canonical_json_object(&text).map_err(|_| Rejection::InvalidMessage)
}

fn decode_request(request: &HttpRequest) -> Result<Event, Rejection> {
    if !is_token(&request.method, MAX_METHOD_BYTES) {
        return Err(Rejection::InvalidMessage);
    }
    if request.method != POST_METHOD {
        return Err(Rejection::MethodNotAllowed);
    }
    if request.uri.len() > MAX_URI_BYTES || request.body.len() > MAX_BODY_BYTES {
        return Err(Rejection::PayloadTooLarge);
    }
    if !is_visible_uri(&request.uri) {
        return Err(Rejection::InvalidMessage);
    }
    validate_headers(&request.headers).map_err(|error| match error {
        HeaderError::OverBound => Rejection::PayloadTooLarge,
        HeaderError::Malformed => Rejection::InvalidMessage,
    })?;
    let body = std::str::from_utf8(&request.body).map_err(|_| Rejection::InvalidMessage)?;
    // RFC 9110 §8.3.1: "The type and subtype tokens are case-insensitive."
    let media_type = header_value(&request.headers, "content-type").and_then(|value| {
        value
            .split(';')
            .next()
            .map(|media| media.trim().to_ascii_lowercase())
    });
    let is_form = media_type.as_deref() == Some(FORM_MEDIA_TYPE);
    let (form_name, data) = if is_form {
        let form = decode_form(body)?;
        let data = match (form.content.len(), form.params.is_empty()) {
            (0, true) => DataValue::Absent,
            (0, false) => params_object(form.params)?,
            // A bare segment is `<content>`, not a key-value pair: B.2.8.1's
            // content rules read it (`content_data`).
            (1, true) => form
                .content
                .into_iter()
                .next()
                .map_or(DataValue::Absent, |content| content_data(None, &content)),
            // A bare segment is `<content>`, which never travels with
            // parameters (SCXML 6.2); an ambiguous body is rejected.
            _ => return Err(Rejection::InvalidMessage),
        };
        (form.event_name, data)
    } else if body.is_empty() {
        (None, DataValue::Absent)
    } else {
        (None, content_data(media_type.as_deref(), body))
    };
    let name = form_name
        .or_else(|| header_value(&request.headers, EVENT_NAME_PARAMETER).map(str::to_owned))
        .unwrap_or_else(|| DEFAULT_EVENT_NAME.to_owned());
    let name = EventName::new(name).map_err(|_| Rejection::InvalidMessage)?;
    let raw = render_raw(request, body)?;
    let mut event = Event::external(name);
    event.data = data;
    event.raw = Some(raw);
    event.origin_type = Some(BASICHTTP_EVENT_PROCESSOR_URI.to_owned());
    event.validate().map_err(|error| match error {
        scxml_types::EventError::ValueTooLarge | scxml_types::EventError::AggregateTooLarge => {
            Rejection::PayloadTooLarge
        }
        scxml_types::EventError::InvalidValue | scxml_types::EventError::InvalidTraceContext => {
            Rejection::InvalidMessage
        }
    })?;
    Ok(event)
}

/// Content that is not key-value pairs (a non-form body, or a form body's
/// bare `<content>` segment) as `_event.data`, per SCXML B.2.8.1:
///
/// * "In some circumstances, the entity that generated the event may have
///   indicated how the data is to be interpreted. In such cases, the
///   Processor SHOULD try to format the data according to those
///   indications." A JSON media type (`application/json`, RFC 8259 §11, or
///   a `+json` suffix, RFC 6839 §3.1) is tried as JSON first, an XML one
///   (`application/xml` or `text/xml`, RFC 7303 §3, or a `+xml` suffix,
///   §4.2) as an XML document first.
/// * "Otherwise (i.e., if the content does not consist of key-value pairs),
///   if the Processor supports JSON and it can interpret the content as
///   JSON, it MUST create the corresponding ECMAScript object(s) as the
///   value of _event.data. Otherwise, if the Processor can interpret the
///   content as a valid XML document, it MUST create the corresponding DOM
///   structure and assign it as the value _event.data." Well-formed JSON is
///   its value; a well-formed XML document (one element, with only
///   comments, processing instructions, and white space beside it, under
///   the data models' own bounded parser) is an XML value.
/// * "Otherwise, the Processor MUST treat the content as a space-normalized
///   string literal and assign it as the value of _event.data."
///   ([`space_normalized`]). Any other media type, `text/plain` included,
///   indicates no structure and leaves the content to these rules.
fn content_data(media_type: Option<&str>, body: &str) -> DataValue {
    let subtype = media_type
        .and_then(|media| media.split_once('/'))
        .map_or("", |(_, subtype)| subtype);
    let json_type = media_type == Some("application/json") || subtype.ends_with("+json");
    let xml_type =
        matches!(media_type, Some("application/xml" | "text/xml")) || subtype.ends_with("+xml");
    if xml_type && xml_document(body) {
        return DataValue::Xml(body.to_owned());
    }
    if let Some(value) = json_data(body) {
        return value;
    }
    if !json_type && xml_document(body) {
        return DataValue::Xml(body.to_owned());
    }
    DataValue::String(space_normalized(body))
}

/// SCXML's "space-normalized string literal" as `XPath` 1.0
/// `normalize-space()` defines it (the definition the data models use):
/// leading and trailing white space stripped and every internal run of it
/// replaced by one space, where white space is "the same as those allowed
/// by the S production in XML" (space, tab, carriage return, line feed).
fn space_normalized(text: &str) -> String {
    text.split([' ', '\t', '\r', '\n'])
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The portable value of well-formed JSON text, if it has one.
fn json_data(body: &str) -> Option<DataValue> {
    let trimmed = body.trim_matches(scxml_types::is_xml_space);
    match serde_json::from_str::<serde_json::Value>(trimmed).ok()? {
        serde_json::Value::Null => Some(DataValue::Null),
        serde_json::Value::Bool(value) => Some(DataValue::Bool(value)),
        serde_json::Value::Number(number) => number
            .as_f64()
            .and_then(|number| scxml_types::FiniteNumber::new(number).ok())
            .map(DataValue::Number),
        serde_json::Value::String(value) => Some(DataValue::String(value)),
        serde_json::Value::Array(_) => DataValue::canonical_json_array(trimmed).ok(),
        serde_json::Value::Object(_) => DataValue::canonical_json_object(trimmed).ok(),
    }
}

/// Whether `body` is a well-formed XML document under the data models' own
/// bounded parser: exactly one element, plus only comments, processing
/// instructions, and XML white-space text.
fn xml_document(body: &str) -> bool {
    use scxml_xpath::{NodeKind, NodeRef, Store};
    let mut store = Store::new();
    let Ok(top) = store.parse_fragment(body) else {
        return false;
    };
    let mut elements = 0_usize;
    for id in top {
        let node = NodeRef::node(id);
        match store.kind(node) {
            Ok(NodeKind::Element) => elements = elements.saturating_add(1),
            Ok(NodeKind::Text) => {
                if !store
                    .string_value(node)
                    .is_ok_and(|value| value.chars().all(scxml_types::is_xml_space))
                {
                    return false;
                }
            }
            Ok(NodeKind::Comment | NodeKind::ProcessingInstruction) => {}
            _ => return false,
        }
    }
    elements == 1
}

/// `_event.raw`: request line, header fields, blank line, body.
fn render_raw(request: &HttpRequest, body: &str) -> Result<String, Rejection> {
    let mut raw = String::new();
    let mut push = |text: &str| {
        if raw
            .len()
            .checked_add(text.len())
            .is_some_and(|total| total <= MAX_RAW_BYTES)
        {
            raw.push_str(text);
            Ok(())
        } else {
            Err(Rejection::PayloadTooLarge)
        }
    };
    push(&request.method)?;
    push(" ")?;
    push(&request.uri)?;
    push(" HTTP/1.1\r\n")?;
    for header in &request.headers {
        push(&header.name)?;
        push(": ")?;
        push(&header.value)?;
        push("\r\n")?;
    }
    push("\r\n")?;
    push(body)?;
    Ok(raw)
}

#[cfg(test)]
mod tests;
